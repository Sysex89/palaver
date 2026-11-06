//! A remote control for the running editor, so a script can put it into a given state and
//! screenshot it. Development tooling, not a feature: compiled only with
//! `--features automation`, and `tools/shots.py` is the only thing meant to talk to it.
//!
//! **Every command goes through a handler the UI already uses** — a menu item's `fn`
//! pointer, `Editor::select`, `jump_to`, `switch_pane`, `show_flow`, `play_advance`, or a
//! field's `set_content`, which the `sync_fields` observer then writes into the node exactly
//! as typing does. Nothing here touches `Doc` directly, so a script can reach no state a
//! person could not, and a screenshot never shows a state the UI cannot get into.
//!
//! The protocol is one line in, one line out, over a Unix socket at
//! `temp_dir()/palaver-automation.sock`: `ok`, `ok <data>`, or `err <why>`. The socket is
//! read on a thread of its own, and commands reach the editor through a channel drained on
//! the foreground by a timer — the same shape as the autosave timer in `Editor::new`.
//!
//! **Isolation is the launcher's job**, and the reason is `temp_dir()`: the crash copy lives
//! there, and `replace_doc` deletes it. An automated instance sharing the real temp
//! directory would delete a real crash copy the first time it opened a file. The script
//! therefore sets `TMPDIR` to a private directory, which moves the socket and the recovery
//! file together. Autosave is also switched off here, so a long run cannot write over one.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use gpui::{Context, Focusable, Window, px, size};

use crate::doc::{Doc, NodeKind, clear_backup};
use crate::{
    Editor, MENUS, PaletteNext, PalettePrev, PaletteRun, Selection, SidePane, node_number,
};

/// How often the foreground looks for a command. Plenty for a script that sleeps between
/// every step anyway.
const POLL: Duration = Duration::from_millis(20);

struct Command {
    line: String,
    reply: mpsc::Sender<String>,
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join("palaver-automation.sock")
}

/// Starts listening. Failing to bind is reported and otherwise ignored: the editor works
/// exactly as it would without the feature.
pub fn start(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    editor.autosave_off = true;

    let path = socket_path();
    // A socket file left by a run that was killed would make the bind fail.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("automation: cannot listen on {}: {error}", path.display());
            return;
        }
    };
    eprintln!("automation: listening on {}", path.display());

    let (commands, inbox) = mpsc::channel::<Command>();
    std::thread::spawn(move || {
        // One client at a time, which is all a script is.
        for stream in listener.incoming().flatten() {
            serve(stream, &commands);
        }
    });

    cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            while let Ok(command) = inbox.try_recv() {
                let reply = this.update_in(cx, |editor, window, cx| {
                    run(editor, &command.line, window, cx)
                });
                let Ok(reply) = reply else {
                    return; // The window closed.
                };
                let _ = command.reply.send(match reply {
                    Ok(data) if data.is_empty() => "ok".to_string(),
                    Ok(data) => format!("ok {data}"),
                    Err(why) => format!("err {why}"),
                });
            }
        }
    })
    .detach();
}

/// Relays one client's lines to the foreground and its replies back.
fn serve(stream: UnixStream, commands: &mpsc::Sender<Command>) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            return;
        };
        let (reply, answer) = mpsc::channel();
        if commands.send(Command { line, reply }).is_err() {
            return;
        }
        let Ok(answer) = answer.recv() else {
            return;
        };
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}

/// Runs one command. `Ok` carries data for the reply, often none.
fn run(
    editor: &mut Editor,
    line: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<String, String> {
    let line = line.trim();
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();

    match verb {
        "ping" => Ok(String::new()),

        // The same refusal a person gets as a prompt, minus the prompt: a script cannot
        // answer one, and silently discarding the document is the one thing
        // `confirm_discard` exists to prevent.
        "open" => {
            if editor.doc.is_dirty() {
                return Err("document has unsaved changes".into());
            }
            let path = PathBuf::from(rest);
            let doc = Doc::load(&path)?;
            editor.replace_doc(doc, Some(path), cx);
            Ok(String::new())
        }

        // `menu Play/Play conversation` — the label as the dropdown shows it, with or
        // without its trailing ellipsis. Runs the very `fn` pointer the click does.
        "menu" => {
            let (menu, item) = rest.split_once('/').ok_or("expected Menu/Item")?;
            let (_, items) = MENUS
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(menu.trim()))
                .ok_or_else(|| format!("no menu {menu:?}"))?;
            let (_, _, action) = items
                .iter()
                .find(|(label, _, _)| {
                    label
                        .trim_end_matches('…')
                        .eq_ignore_ascii_case(item.trim().trim_end_matches('…'))
                })
                .ok_or_else(|| format!("no item {item:?} in {menu}"))?;
            action(editor, window, cx);
            Ok(String::new())
        }

        "select" => select(editor, rest, window, cx),

        "pane" => {
            let pane = match rest.to_ascii_lowercase().as_str() {
                "variables" => SidePane::Variables,
                "cast" => SidePane::Cast,
                "flows" => SidePane::Flows,
                "tags" => SidePane::Tags,
                _ => return Err(format!("no pane {rest:?}")),
            };
            editor.switch_pane(pane, cx);
            Ok(String::new())
        }

        // `flow main` for the unnamed one, since an empty argument reads as a mistake.
        "flow" => {
            // The main flow is always there and never listed, so it needs no check.
            let flow = if rest == "main" { "" } else { rest };
            if !flow.is_empty() && !editor.doc.all_flows().contains(&flow) {
                return Err(format!("no flow {rest:?}"));
            }
            editor.show_flow(flow, cx);
            Ok(String::new())
        }

        // The palette's keys, which a script cannot press into a field it has no focus on:
        // `palette next`, `palette prev`, `palette run`. Opening it and typing a query are
        // `menu View/Command palette` and `field palette <text>`.
        "palette" => {
            if !editor.palette_open {
                return Err("the palette is not open".into());
            }
            match rest {
                "next" => editor.on_palette_next(&PaletteNext, window, cx),
                "prev" => editor.on_palette_prev(&PalettePrev, window, cx),
                "run" => editor.on_palette_run(&PaletteRun, window, cx),
                _ => return Err(format!("expected next, prev or run, not {rest:?}")),
            }
            Ok(String::new())
        }

        // Sets a field's whole content, which the field's observer handles exactly as it
        // handles a keystroke.
        "field" => {
            let (name, text) = rest.split_once(' ').unwrap_or((rest, ""));
            let input = field(editor, name)?.clone();
            input.update(cx, |input, cx| input.set_content(text, cx));
            Ok(String::new())
        }

        "focus" => {
            let handle = match rest {
                "canvas" => editor.focus_handle.clone(),
                name => field(editor, name)?.read(cx).focus_handle(cx),
            };
            window.focus(&handle);
            Ok(String::new())
        }

        // `choose 0` — the first choice in the play panel, counting spent ones, as the
        // panel lists them.
        "choose" => {
            let path = editor.play.as_ref().ok_or("not playing")?;
            let current = *path.last().ok_or("empty play path")?;
            let index: usize = rest.parse().map_err(|_| "expected a choice number")?;
            let choices = editor.doc.choices(current);
            let &(to, _) = choices
                .get(index)
                .ok_or_else(|| format!("only {} choices", choices.len()))?;
            if editor.doc.spent(path, current, to) {
                return Err("that choice is spent".into());
            }
            editor.play_advance(to, window, cx);
            Ok(String::new())
        }

        "resize" => {
            let (w, h) = rest.split_once(' ').ok_or("expected width height")?;
            let w: f32 = w.parse().map_err(|_| "bad width")?;
            let h: f32 = h.parse().map_err(|_| "bad height")?;
            window.resize(size(px(w), px(h)));
            Ok(String::new())
        }

        // Screenshots go through the compositor, which captures the *active* window.
        "activate" => {
            window.activate_window();
            Ok(String::new())
        }

        "state" => Ok(state(editor, window, cx)),

        // `quit` refuses a dirty document for `open`'s reason; `quit discard` is the
        // explicit "Discard", and drops the crash copy the way the confirmed close does.
        "quit" => {
            if editor.doc.is_dirty() && rest != "discard" {
                return Err("document has unsaved changes; use `quit discard`".into());
            }
            clear_backup();
            cx.quit();
            Ok(String::new())
        }

        _ => Err(format!("unknown command {verb:?}")),
    }
}

/// `select none`, `select node 3` (the 1-based position the inspector shows),
/// `select alias intro`, `select kind hub [2]`, `select nodes 1 2 3`, `select edge 0`.
fn select(
    editor: &mut Editor,
    rest: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<String, String> {
    let mut words = rest.split_whitespace();
    let how = words.next().ok_or("select what?")?;
    let doc = &editor.doc;

    // Through `jump_to`, which also switches flow and centres the node, as a search hit or
    // a problem row does.
    let id = match how {
        "none" => {
            editor.select(Selection::None, cx);
            return Ok(String::new());
        }
        "edge" => {
            let index: usize = words
                .next()
                .and_then(|w| w.parse().ok())
                .ok_or("edge index")?;
            if index >= doc.edges.len() {
                return Err(format!("only {} edges", doc.edges.len()));
            }
            editor.select_edge(index, window, cx);
            return Ok(String::new());
        }
        "nodes" => {
            let ids = words
                .map(|w| position(doc, w))
                .collect::<Result<Vec<_>, _>>()?;
            editor.select(Selection::of(ids), cx);
            return Ok(String::new());
        }
        "node" => position(doc, words.next().ok_or("node position")?)?,
        "alias" => {
            let alias = words.next().ok_or("alias")?;
            doc.nodes
                .iter()
                .find(|n| n.alias == alias)
                .map(|n| n.id)
                .ok_or_else(|| format!("no alias {alias:?}"))?
        }
        "kind" => {
            let kind = match words.next().map(str::to_ascii_lowercase).as_deref() {
                Some("speech") => NodeKind::Speech,
                Some("hub") => NodeKind::Hub,
                Some("jump") => NodeKind::Jump,
                Some("instruction") => NodeKind::Instruction,
                Some("condition") => NodeKind::Condition,
                Some("note") => NodeKind::Note,
                other => return Err(format!("no kind {other:?}")),
            };
            let nth: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(1);
            doc.nodes
                .iter()
                .filter(|n| n.kind == kind)
                .nth(nth.saturating_sub(1))
                .map(|n| n.id)
                .ok_or_else(|| format!("no {kind:?} number {nth}"))?
        }
        _ => return Err(format!("cannot select by {how:?}")),
    };
    editor.jump_to(id, window, cx);
    Ok(String::new())
}

/// A node by the 1-based position `node_number` prints.
fn position(doc: &Doc, word: &str) -> Result<crate::NodeId, String> {
    let n: usize = word.parse().map_err(|_| format!("bad position {word:?}"))?;
    n.checked_sub(1)
        .and_then(|i| doc.nodes.get(i))
        .map(|node| node.id)
        .ok_or_else(|| format!("no node {n}"))
}

fn field<'a>(editor: &'a Editor, name: &str) -> Result<&'a gpui::Entity<crate::TextInput>, String> {
    Ok(match name {
        "speaker" => &editor.speaker_input,
        "text" => &editor.text_input,
        "alias" => &editor.alias_input,
        "label" => &editor.label_input,
        "search" => &editor.search_input,
        "palette" => &editor.palette_input,
        "var" => &editor.var_input,
        "value" => &editor.value_input,
        _ => return Err(format!("no field {name:?}")),
    })
}

/// One line a script can assert on. Positions, not ids, for `node_number`'s reason.
fn state(editor: &Editor, window: &Window, cx: &gpui::App) -> String {
    let doc = &editor.doc;
    let selection = match &editor.selection {
        Selection::None => "none".to_string(),
        Selection::Edge(index) => format!("edge:{index}"),
        Selection::Nodes(ids) => format!(
            "nodes:{}",
            ids.iter()
                .map(|&id| node_number(doc, id).trim_start_matches("Node ").to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    let playing = match editor.play.as_ref().and_then(|path| path.last()) {
        Some(&id) => node_number(doc, id).trim_start_matches("Node ").to_string(),
        None => "no".into(),
    };
    let focused = [
        ("speaker", &editor.speaker_input),
        ("text", &editor.text_input),
        ("alias", &editor.alias_input),
        ("label", &editor.label_input),
        ("search", &editor.search_input),
        ("palette", &editor.palette_input),
        ("var", &editor.var_input),
        ("value", &editor.value_input),
    ]
    .into_iter()
    .find(|(_, input)| input.read(cx).focus_handle(cx).is_focused(window))
    .map_or("-", |(name, _)| name);
    // Which overlays are up, so a script can tell a shot of the find card from a shot of the
    // canvas it failed to open over.
    let overlays: Vec<&str> = [
        ("search", editor.searching),
        ("problems", editor.problems_open),
        ("help", editor.help_open),
        ("palette", editor.palette_open),
        ("map", editor.flow_map),
    ]
    .into_iter()
    .filter_map(|(name, open)| open.then_some(name))
    .collect();
    let overlays = if overlays.is_empty() {
        "-".to_string()
    } else {
        overlays.join(",")
    };
    format!(
        "flow={} zoom={} selection={selection} playing={playing} dirty={} problems={} nodes={} field={focused} overlays={overlays} path={}",
        if editor.flow.is_empty() {
            "main"
        } else {
            &editor.flow
        },
        (editor.view.zoom * 100.0).round(),
        doc.is_dirty(),
        doc.problems().len(),
        doc.nodes.len(),
        editor
            .path
            .as_deref()
            .map_or("-".to_string(), |p| p.display().to_string()),
    )
}
