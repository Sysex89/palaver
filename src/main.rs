#[cfg(all(feature = "automation", unix))]
mod automation;
mod doc;
mod text_input;
mod theme;

use doc::{
    Cmp, Doc, FILE_EXT, FIT_MARGIN, FTL_EXT, NODE_H, NODE_W, Node, NodeId, NodeKind,
    OPEN_FIT_MIN_ZOOM, PORT_R, Problem, SetOp, Severity, Value, VarState, View, clear_backup,
    in_port, join_set, node_center, out_port, read_backup, split_set, with_default_extension,
};
use gpui::{
    AnyElement, App, Application, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder,
    PathPromptOptions, Pixels, Point, PromptLevel, Render, Rgba, ScrollHandle, ScrollWheelEvent,
    SharedString, Window, WindowBounds, WindowHandle, WindowOptions, actions, canvas, deferred,
    div, point, prelude::*, px, rgb, size,
};
use std::path::PathBuf;
use std::time::Duration;
use text_input::TextInput;
use theme::Typography;

actions!(
    palaver,
    [
        DeleteNode,
        Undo,
        Redo,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Find,
        Branch,
        NewHub,
        NewJump,
        NewInstruction,
        NewCondition,
        NewNote,
        FocusCanvas,
        New,
        Save,
        SaveAs,
        ExportFluent,
        Open,
        Play,
        PlayFromHere,
        StopPlay,
        ToggleProblems,
        ShowHelp,
        FitView,
        ShowPalette,
        PaletteNext,
        PalettePrev,
        PaletteRun
    ]
);

/// Height of the menu bar. Mouse positions are window-relative, so this is also the
/// offset between window coordinates and the canvas pane below the bar.
const MENU_H: f32 = 30.0;
/// Height of the status bar under the three panes. It makes the canvas pane shorter at the
/// *bottom*, and hit tests measure from the top-left, so — unlike `MENU_H` — it is not
/// subtracted in `window_pos`. Only `pane_center` has to know, since that is the one place
/// the pane's height is read.
const STATUS_H: f32 = 26.0;
/// Radius of the ring marking a node nothing links to. Smaller than `PORT_R`, so the handle
/// you drag from stays the larger and the only interactive-looking one.
const ROOT_R: f32 = 5.0;
/// Edge stroke widths. A walked edge is heavier, not just greener: a condition's pins already
/// use the trail's hue for `true`, so weight is the one cue that is not colour.
const EDGE_W: f32 = 1.5;
const TRAIL_W: f32 = 3.0;
/// How long a status message stays up before it clears itself.
const STATUS_FOR: Duration = Duration::from_secs(6);
/// Width of the inspector on the right.
const SIDEBAR_W: f32 = 280.0;
/// Width of the global-variables pane on the left. With [`SIDEBAR_W`] and [`MENU_H`], this
/// is what bounds the canvas — and what `window_pos` has to subtract off every mouse event.
const VARS_W: f32 = 220.0;

/// How much one pixel of scroll changes the zoom. Multiplicative, so a scroll up and an
/// equal scroll down cancel out exactly.
const ZOOM_PER_PIXEL: f32 = 1.0015;
/// Zoom step for the keyboard commands. `1 / 1.25 == 0.8`, so in-then-out is a round trip.
const ZOOM_STEP: f32 = 1.25;
/// What `var_input` is asking for, which is the placeholder it shows: the pane's `+` declares
/// into one list or the other, and the field is shared. One function so `Editor::new` and the
/// tab handler cannot disagree.
fn declare_hint(pane: SidePane) -> &'static str {
    match pane {
        SidePane::Variables => "New flag",
        SidePane::Cast => "New character",
        SidePane::Flows => "New flow",
    }
}

/// The colours a character can be tinted, cycled by clicking the swatch. The one set of hex
/// literals outside `theme.rs`, deliberately: these values are written into `.dln` files, so
/// they are document data, and retuning one would orphan every saved character using it.
/// It lives here rather than in `doc.rs` for the same
/// reason `problem_text` does: the model stores the colour, the view chooses it. There is no
/// colour-picker widget in the crate, and a click-cycle is the idiom everywhere else in it.
const CAST_COLORS: [u32; 8] = [
    0x9ecbff, 0xf7c96a, 0x8ce99a, 0xc3a6f7, 0xff7a7a, 0x7aa2f7, 0xb8b8c4, 0x9a9aa6,
];

/// How often a dirty document gets a crash copy. Also the most work a crash can cost.
const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

type MenuAction = fn(&mut Editor, &mut Window, &mut Context<Editor>);
/// Label, shortcut hint, what it runs.
type MenuItem = (&'static str, &'static str, MenuAction);
type MenuSpec = (&'static str, &'static [MenuItem]);

/// The palette's own menu entry. Named once because [`palette_entries`] leaves it out: a
/// command palette that offers to open the command palette is a list with a dead row in it.
const PALETTE_LABEL: &str = "Command palette…";

/// The commands the palette offers for `query`, as `(menu, item)` pairs read straight off
/// `MENUS` — so the palette, the menus and the shortcut sheet are one table and cannot
/// disagree about what a command is called or what runs.
///
/// Every word of the query has to appear somewhere in "Menu Label", in any order, so
/// `node hub` and `hub node` both find Hub and `fit` finds Fit to view. Ranked by where the
/// first word lands: the start of a label, then inside one, then only in its menu's name
/// (`play` lists the whole Play menu). The sort is stable, so ties keep the order of the bar.
fn palette_entries(query: &str) -> Vec<(&'static str, &'static MenuItem)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(u8, &'static str, &'static MenuItem)> = Vec::new();
    for (menu, items) in MENUS {
        for item in *items {
            let (label, _, _) = item;
            if *label == PALETTE_LABEL {
                continue;
            }
            let label_lc = label.to_lowercase();
            let haystack = format!("{} {label_lc}", menu.to_lowercase());
            if !words.iter().all(|word| haystack.contains(word.as_str())) {
                continue;
            }
            let rank = match words.first() {
                None => 0,
                Some(word) if label_lc.starts_with(word.as_str()) => 0,
                Some(word) if label_lc.contains(word.as_str()) => 1,
                Some(_) => 2,
            };
            found.push((rank, menu, item));
        }
    }
    found.sort_by_key(|&(rank, _, _)| rank);
    found
        .into_iter()
        .map(|(_, menu, item)| (menu, item))
        .collect()
}

/// Every action the editor has, in the order they appear in the bar.
const MENUS: &[MenuSpec] = &[
    (
        "File",
        &[
            ("New", "Ctrl+N", |e, w, cx| e.on_new(&New, w, cx)),
            ("Open…", "Ctrl+O", |e, w, cx| e.on_open(&Open, w, cx)),
            ("Save", "Ctrl+S", |e, w, cx| e.on_save(&Save, w, cx)),
            ("Save As…", "Ctrl+Shift+S", |e, w, cx| {
                e.on_save_as(&SaveAs, w, cx)
            }),
            ("Export Fluent…", "Ctrl+E", |e, w, cx| {
                e.on_export_fluent(&ExportFluent, w, cx)
            }),
        ],
    ),
    (
        "Edit",
        &[
            ("Undo", "Ctrl+Z", |e, w, cx| e.on_undo(&Undo, w, cx)),
            ("Redo", "Ctrl+Shift+Z", |e, w, cx| e.on_redo(&Redo, w, cx)),
            ("Find…", "Ctrl+F", |e, w, cx| e.on_find(&Find, w, cx)),
            ("Delete selection", "Del", |e, w, cx| {
                e.on_delete(&DeleteNode, w, cx)
            }),
        ],
    ),
    // Every kind is one keystroke, hung off the selected node. Without this a condition
    // took a double-click and a trip through the inspector's kind picker to reach.
    (
        "Node",
        &[
            ("Speech", "Tab", |e, w, cx| e.on_branch(&Branch, w, cx)),
            ("Hub", "Ctrl+2", |e, w, cx| e.on_new_hub(&NewHub, w, cx)),
            ("Jump", "Ctrl+3", |e, w, cx| e.on_new_jump(&NewJump, w, cx)),
            ("Instruction", "Ctrl+4", |e, w, cx| {
                e.on_new_instruction(&NewInstruction, w, cx)
            }),
            ("Condition", "Ctrl+5", |e, w, cx| {
                e.on_new_condition(&NewCondition, w, cx)
            }),
            ("Note", "Ctrl+6", |e, w, cx| e.on_new_note(&NewNote, w, cx)),
        ],
    ),
    (
        "View",
        &[
            ("Zoom in", "Ctrl+=", |e, w, cx| e.on_zoom_in(&ZoomIn, w, cx)),
            ("Zoom out", "Ctrl+-", |e, w, cx| {
                e.on_zoom_out(&ZoomOut, w, cx)
            }),
            ("Actual size", "Ctrl+0", |e, w, cx| {
                e.on_zoom_reset(&ZoomReset, w, cx)
            }),
            ("Fit to view", "F", |e, w, cx| {
                e.on_fit_view(&FitView, w, cx)
            }),
            ("Problems", "Ctrl+Shift+M", |e, w, cx| {
                e.on_toggle_problems(&ToggleProblems, w, cx)
            }),
            (PALETTE_LABEL, "Ctrl+K", |e, w, cx| {
                e.on_show_palette(&ShowPalette, w, cx)
            }),
            ("Keyboard shortcuts", "F1", |e, w, cx| {
                e.on_show_help(&ShowHelp, w, cx)
            }),
        ],
    ),
    (
        "Play",
        &[
            ("Play conversation", "F5", |e, w, cx| {
                e.on_play(&Play, w, cx)
            }),
            ("Play from here", "Shift+F5", |e, w, cx| {
                e.on_play_from_here(&PlayFromHere, w, cx)
            }),
            ("Stop", "Esc", |e, w, cx| e.on_stop_play(&StopPlay, w, cx)),
        ],
    ),
];

/// A document's display name, or a placeholder while it has no file yet.
fn document_name(path: Option<&std::path::Path>) -> String {
    match path {
        Some(path) => path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        None => format!("untitled.{FILE_EXT}"),
    }
}

struct Editor {
    doc: Doc,
    /// Where the canvas is looking. Applied to everything drawn.
    view: View,
    selection: Selection,
    drag: Option<Drag>,
    focus_handle: FocusHandle,
    speaker_input: Entity<TextInput>,
    text_input: Entity<TextInput>,
    /// The node's external handle. Node content like the two above it, so it belongs in the
    /// `sync_fields` loop — unlike `search_input` and `var_input` below.
    alias_input: Entity<TextInput>,
    label_input: Entity<TextInput>,
    /// Search query. Deliberately outside the `sync_fields` loop — it is not document content.
    search_input: Entity<TextInput>,
    /// Name for a variable about to be declared. Outside the `sync_fields` loop for the
    /// same reason `search_input` is: it is not content of the selected node.
    var_input: Entity<TextInput>,
    /// What `var_input` is renaming, or `None` when it is declaring a new one. The panel
    /// has one field and several jobs; this and `Editor::pane` are which. Never cleared on
    /// a document swap — `vars_pane` filters a name the document no longer has, which
    /// covers the swap, an undo and a delete in one place instead of three that can be
    /// forgotten.
    renaming: Option<Rename>,
    /// The set the declare field is aimed at, `""` for none. It holds only the *set*, so
    /// the field holds only the rest of the name and `doc::join_set` puts them back
    /// together — which is the whole point of the picker: an existing set is clicked
    /// rather than retyped. Filtered against the document at render like `renaming` is.
    set: String,
    /// The number editor, shared by the variables pane and the inspector's Sets/Checks rows
    /// so there is only ever one half-typed number in the crate. Outside the `sync_fields`
    /// loop for the same reason `var_input` is — a number being typed is not the selected
    /// node's content, and syncing it would write every keystroke into the document.
    value_input: Entity<TextInput>,
    /// Whose value `value_input` holds, or `None` when nothing is being typed. Filtered
    /// against the document at render like `renaming` is, rather than cleared at every site
    /// that could invalidate it.
    editing: Option<ValueTarget>,
    /// What is currently loaded into the sidebar fields.
    edited: Selection,
    /// Whether this editing session already pushed an undo checkpoint.
    text_dirty: bool,
    /// Where the document was last saved or loaded from.
    path: Option<PathBuf>,
    /// Outcome of the last save or load, shown in the status bar until it clears itself.
    status: Option<String>,
    /// Bumped by every `set_status`, so a message's expiry timer can tell it is still the
    /// message that started it and not clear a newer one.
    status_gen: u32,
    /// The find overlay is open. The query lives in `search_input`, which is not rendered
    /// while this is `false`.
    searching: bool,
    /// The problems drawer, opened from the status bar's badge.
    problems_open: bool,
    /// The keyboard-and-mouse cheat sheet.
    help_open: bool,
    /// The command palette is open. Its query lives in `palette_input`, which like
    /// `search_input` is not document content and stays out of the `sync_fields` loop.
    palette_open: bool,
    palette_input: Entity<TextInput>,
    /// Which row Enter would run. Reset to the top whenever the query changes, so the best
    /// match is always the one under the cursor.
    palette_sel: usize,
    /// The query `palette_sel` was last reset for, so a cursor move in the field — which
    /// notifies too — does not throw the selection back to the top.
    palette_seen: String,
    /// Scrolls the list to the selected row when the arrow keys walk past the edge of it.
    palette_scroll: ScrollHandle,
    /// Last title pushed to the platform, so `render` only sets it on change.
    title: String,
    /// Name of the menu whose dropdown is open.
    open_menu: Option<&'static str>,
    /// Set once discarding has been confirmed, so the close hook stops asking.
    closing: bool,
    /// Set after a backup fails, so a read-only temp directory reports once rather than
    /// every tick for the rest of the session.
    autosave_off: bool,
    /// The play-through walked so far, last entry first — `None` when not playing. One vec
    /// carries the current node, Back and Restart.
    play: Option<Vec<NodeId>>,
    /// Which list the left pane is showing. Its only new state: the cast rides the container
    /// and the text field the variables list already proved out, and a *third* pane was
    /// rejected because `window_pos` is the one place a new one breaks every hit test.
    pane: SidePane,
    /// The view wants fitting to the document before the next frame is drawn. Set by anything
    /// that installs a document; consumed in `render`, because the pane's size is only known
    /// where there is a window — and so the very first frame, whose size is the real one, is
    /// fitted rather than guessed at in `new`.
    fit_pending: bool,
    /// Which canvas is on screen. A flow is a *view* onto the one flat node list, so this
    /// is editor state and never serialized — the whole feature is this filter plus a tab.
    flow: String,
}

/// What the shared `var_input` is renaming. `Editor::pane` already says which *list* an
/// `Item` belongs to; the one ambiguity a pane cannot resolve is inside the Variables tab,
/// where the field can hold a variable's name or the name of the set they group under.
#[derive(Clone, PartialEq)]
enum Rename {
    Item(String),
    Set(String),
}

/// Which document-global list the left pane shows. With `Editor::renaming`, this is what
/// says which of `var_input`'s jobs it is doing.
#[derive(Clone, Copy, Default, PartialEq)]
enum SidePane {
    #[default]
    Variables,
    Cast,
    Flows,
}

/// Node ids are stable within a history step, so a node selection can be a set.
/// Edge indices are not — they shift when edges are removed, so an edge selection
/// stays singular and is cleared on every structural change.
#[derive(Clone, Default, PartialEq)]
enum Selection {
    #[default]
    None,
    Nodes(Vec<NodeId>),
    Edge(usize),
}

/// Which value the shared number editor is aimed at: a variable's starting value, what an
/// instruction writes, or what a condition compares against.
#[derive(Clone, PartialEq)]
enum ValueTarget {
    Default(String),
    Effect(NodeId, String),
    Check(NodeId, String),
}

/// Which side of a variable a `var_rows` list is editing: what an instruction writes, or
/// what a condition checks.
#[derive(Clone, Copy, PartialEq)]
enum VarRow {
    Sets,
    Checks,
}

#[derive(Clone, Copy)]
enum Drag {
    /// `grab` is the cursor's offset inside the node, in document coordinates. The rest
    /// of the selection rides along, so this drags a whole group.
    Node {
        id: NodeId,
        grab: (f32, f32),
        moved: bool,
    },
    /// Rubber band from a node's out-port to the cursor.
    Edge { from: NodeId, cursor: (f32, f32) },
    Pan {
        start_mouse: (f32, f32),
        start_pan: (f32, f32),
    },
    /// Selection band over empty canvas, in document coordinates.
    Marquee {
        start: (f32, f32),
        cursor: (f32, f32),
    },
}

impl Selection {
    /// The selected node ids. Empty for an edge or an empty selection.
    fn nodes(&self) -> &[NodeId] {
        match self {
            Selection::Nodes(ids) => ids,
            _ => &[],
        }
    }

    /// The one selected node, when the selection is exactly one node. Everything that
    /// edits a single node goes through this, so a multi-selection simply edits nothing.
    fn only_node(&self) -> Option<NodeId> {
        match self.nodes() {
            [id] => Some(*id),
            _ => None,
        }
    }

    /// Adds the id when absent, removes it when present — what shift-click does.
    fn toggled(&self, id: NodeId) -> Selection {
        let mut ids = self.nodes().to_vec();
        match ids.iter().position(|n| *n == id) {
            Some(at) => {
                ids.remove(at);
            }
            None => ids.push(id),
        }
        Selection::of(ids)
    }

    /// A node selection, or `None` when the set came out empty.
    fn of(ids: Vec<NodeId>) -> Selection {
        if ids.is_empty() {
            Selection::None
        } else {
            Selection::Nodes(ids)
        }
    }
}

impl Editor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        let speaker_input = cx.new(|cx| TextInput::new("Speaker", cx));
        let text_input = cx.new(|cx| TextInput::new("Line of dialog", cx));
        let alias_input = cx.new(|cx| TextInput::new("Name for the game to call", cx));
        let label_input = cx.new(|cx| TextInput::new("Choice", cx));
        for input in [&speaker_input, &text_input, &alias_input, &label_input] {
            cx.observe(input, |this, _, cx| this.sync_fields(cx))
                .detach();
        }

        // Neither of these is document content, so they stay out of the loop above and only
        // ask for a re-render — without this, typing would not refresh what they drive.
        let search_input = cx.new(|cx| TextInput::new("Find a node", cx));
        let var_input = cx.new(|cx| TextInput::new(declare_hint(SidePane::default()), cx));
        let value_input = cx.new(|cx| TextInput::new("0", cx));
        for input in [&search_input, &var_input, &value_input] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        let palette_input = cx.new(|cx| TextInput::new("Type a command", cx));
        cx.observe(&palette_input, |this, input, cx| {
            let query = input.read(cx).content().to_string();
            if query != this.palette_seen {
                this.palette_seen = query;
                this.palette_sel = 0;
            }
            cx.notify();
        })
        .detach();

        // Crash copies, for as long as the window lives. Spawning during construction is
        // fine: the entity slot exists before this closure runs, and the task cannot be
        // polled until the window is open.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSAVE_EVERY).await;
                if this.update(cx, |this, cx| this.autosave(cx)).is_err() {
                    return; // The entity is gone — the window closed.
                }
            }
        })
        .detach();

        Self {
            doc: Doc::sample(),
            view: View::default(),
            selection: Selection::None,
            drag: None,
            focus_handle,
            speaker_input,
            text_input,
            alias_input,
            label_input,
            search_input,
            var_input,
            value_input,
            renaming: None,
            set: String::new(),
            editing: None,
            edited: Selection::None,
            text_dirty: false,
            path: None,
            status: None,
            status_gen: 0,
            searching: false,
            problems_open: false,
            help_open: false,
            palette_open: false,
            palette_input,
            palette_sel: 0,
            palette_seen: String::new(),
            palette_scroll: ScrollHandle::new(),
            title: String::new(),
            open_menu: None,
            closing: false,
            autosave_off: false,
            play: None,
            pane: SidePane::default(),
            flow: String::new(),
            fit_pending: true,
        }
    }

    /// Writes a crash copy, when there is something to lose and it is safe to take one.
    /// Silent on success — the point of an autosave is that nobody notices it.
    fn autosave(&mut self, cx: &mut Context<Self>) {
        if self.autosave_off || !self.doc.is_dirty() || self.drag.is_some() {
            return;
        }
        if let Err(error) = self.doc.write_backup(self.path.as_deref()) {
            self.autosave_off = true;
            self.set_status(format!("Autosave failed: {error}"), cx);
        }
    }

    fn file_name(&self) -> String {
        document_name(self.path.as_deref())
    }

    /// Offers back a crash copy left by a previous run. Called once, at startup.
    fn offer_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backup) = read_backup() else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Recover unsaved work?",
            Some(&format!(
                "{} was not saved before the last session ended.",
                document_name(backup.path.as_deref())
            )),
            &["Recover", "Discard"],
            cx,
        );

        cx.spawn(async move |this, cx| {
            match answer.await {
                Ok(0) => {
                    this.update(cx, |this, cx| {
                        // The prompt does not hold the mouse on Linux, so the document may
                        // have been edited while it was up. Recovering over it would discard
                        // those edits without asking, which is the one thing `confirm_discard`
                        // exists to prevent.
                        if this.doc.is_dirty() {
                            return;
                        }
                        let path = backup.path;
                        this.replace_doc(backup.doc, path, cx);
                        // Recovered work is still unsaved work, and `checkpoint` is the only
                        // thing that says so. The price is a first undo that restores the
                        // state it was already in.
                        this.doc.checkpoint();
                    })
                    .ok();
                }
                // Only an explicit Discard deletes it — a prompt that was cancelled resolves
                // to `Err`, and must not take the user's only copy with it.
                Ok(_) => clear_backup(),
                Err(_) => {}
            }
        })
        .detach();
    }

    /// Asks before unsaved edits are thrown away. Resolves to true on "Discard".
    ///
    // ponytail: two buttons, not Save/Discard/Cancel. A Save button would have to chain
    // into the Save As dialog when there is no path yet — a second async prompt inside
    // this one. Add it if discarding by accident turns out to bite.
    fn confirm_discard(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> impl Future<Output = bool> + use<> {
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some(&format!("{} has unsaved edits.", self.file_name())),
            &["Discard", "Cancel"],
            cx,
        );
        async move { answer.await == Ok(0) }
    }

    fn on_new(&mut self, _: &New, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc.is_dirty() {
            return self.reset(cx);
        }

        let confirmed = self.confirm_discard(window, cx);
        cx.spawn(async move |this, cx| {
            if confirmed.await {
                this.update(cx, |this, cx| this.reset(cx)).ok();
            }
        })
        .detach();
    }

    /// Vetoed close: ask, and only then take the window down. Setting `closing` is what
    /// lets the second pass through the hook go straight to true.
    fn confirm_close(
        &mut self,
        handle: WindowHandle<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let confirmed = self.confirm_discard(window, cx);
        cx.spawn(async move |_, cx| {
            if confirmed.await {
                handle
                    .update(cx, |this, window, _| {
                        this.closing = true;
                        // Discarding on purpose is not something to offer back next launch.
                        clear_backup();
                        window.remove_window();
                    })
                    .ok();
            }
        })
        .detach();
    }

    /// Starts an empty, unnamed document.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.replace_doc(Doc::default(), None, cx);
    }

    /// Installs a different document. The one on screen is gone after this, so its crash
    /// copy goes too — this is the only place a backup is dropped on a document swap.
    fn replace_doc(&mut self, doc: Doc, path: Option<PathBuf>, cx: &mut Context<Self>) {
        clear_backup();
        self.doc = doc;
        self.view = View::default();
        self.fit_pending = true;
        self.drag = None;
        // Node ids — and flow names — belong to the document that is going away.
        self.play = None;
        self.flow = String::new();
        self.path = path;
        self.status = None;
        self.select(Selection::None, cx);
        cx.notify();
    }

    fn on_save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return self.on_save_as(&SaveAs, window, cx);
        };
        self.save_to(path, cx);
    }

    fn on_save_as(&mut self, _: &SaveAs, _: &mut Window, cx: &mut Context<Self>) {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let suggested = self.file_name();
        let response = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn(async move |this, cx| {
            match response.await {
                Ok(Ok(Some(path))) => {
                    this.update(cx, |this, cx| this.save_to(path, cx)).ok();
                }
                // Cancelled — nothing to report.
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.set_status(format!("Save dialog failed: {error}"), cx)
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Writes the translatable strings out for a game build. Deliberately not a save: it
    /// leaves `self.path`, the dirty flag and the crash copy alone, so exporting never
    /// re-points the editor at the `.ftl`.
    fn on_export_fluent(&mut self, _: &ExportFluent, _: &mut Window, cx: &mut Context<Self>) {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let suggested = PathBuf::from(self.file_name())
            .with_extension(FTL_EXT)
            .to_string_lossy()
            .into_owned();
        let response = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn(async move |this, cx| {
            match response.await {
                Ok(Ok(Some(path))) => {
                    this.update(cx, |this, cx| {
                        let path = with_default_extension(path, FTL_EXT);
                        let status = match this.doc.export_ftl(&path) {
                            Ok(()) => format!("Exported {}", document_name(Some(&path))),
                            Err(error) => format!("Export failed: {error}"),
                        };
                        this.set_status(status, cx);
                    })
                    .ok();
                }
                // Cancelled — nothing to report.
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.set_status(format!("Export dialog failed: {error}"), cx)
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn on_open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc.is_dirty() {
            return self.open_dialog(cx);
        }

        // Opening replaces the document, so it discards unsaved edits just like New does.
        let confirmed = self.confirm_discard(window, cx);
        cx.spawn(async move |this, cx| {
            if confirmed.await {
                this.update(cx, |this, cx| this.open_dialog(cx)).ok();
            }
        })
        .detach();
    }

    fn open_dialog(&mut self, cx: &mut Context<Self>) {
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        cx.spawn(async move |this, cx| {
            let paths = match response.await {
                Ok(Ok(Some(paths))) => paths,
                // Cancelled — nothing to report.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.set_status(format!("Open dialog failed: {error}"), cx)
                    })
                    .ok();
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| match Doc::load(&path) {
                Ok(doc) => this.replace_doc(doc, Some(path), cx),
                // The current document is left untouched on a bad file.
                Err(error) => this.set_status(format!("Open failed: {error}"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // A path typed without an extension in the dialog still becomes a .dln file.
        let path = with_default_extension(path, FILE_EXT);
        match self.doc.save(&path) {
            Ok(()) => {
                // The work is on disk under its own name now; the crash copy is redundant.
                clear_backup();
                self.path = Some(path);
                let status = format!("Saved {}", self.file_name());
                self.set_status(status, cx);
            }
            Err(error) => self.set_status(format!("Save failed: {error}"), cx),
        }
    }

    /// Shows a message in the status bar and clears it after [`STATUS_FOR`]. A message that
    /// stayed forever would be stale the moment the next edit made it untrue.
    fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = Some(status);
        self.status_gen = self.status_gen.wrapping_add(1);
        let generation = self.status_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STATUS_FOR).await;
            this.update(cx, |this, cx| {
                // A newer message has its own timer; this one is not its to clear.
                if this.status_gen == generation {
                    this.status = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Loads the selection's content into the sidebar fields, starting a fresh editing session.
    fn load_fields(&mut self, selection: Selection, cx: &mut Context<Self>) {
        let (speaker, text, alias, label) = match (selection.only_node(), &selection) {
            (Some(id), _) => match self.doc.node(id) {
                Some(node) => (
                    node.speaker.clone(),
                    node.text.clone(),
                    node.alias.clone(),
                    String::new(),
                ),
                None => Default::default(),
            },
            (None, Selection::Edge(index)) => match self.doc.edges.get(*index) {
                Some(edge) => (
                    String::new(),
                    String::new(),
                    String::new(),
                    edge.label.clone(),
                ),
                None => Default::default(),
            },
            _ => Default::default(),
        };

        self.speaker_input
            .update(cx, |input, cx| input.set_content(&speaker, cx));
        self.text_input
            .update(cx, |input, cx| input.set_content(&text, cx));
        self.alias_input
            .update(cx, |input, cx| input.set_content(&alias, cx));
        self.label_input
            .update(cx, |input, cx| input.set_content(&label, cx));
        let kind = selection
            .only_node()
            .and_then(|id| self.doc.node(id))
            .map_or(NodeKind::Speech, |node| node.kind);
        self.text_input
            .update(cx, |input, cx| input.set_placeholder(text_prompt(kind), cx));
        self.edited = selection;
        self.text_dirty = false;
    }

    /// Writes the fields back into the edited node or edge. Runs on every input
    /// notification, so it must be a no-op when nothing actually changed.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        match (self.edited.only_node(), &self.edited) {
            (Some(id), _) => {
                let speaker = self.speaker_input.read(cx).content().to_string();
                let text = self.text_input.read(cx).content().to_string();
                let alias = self.alias_input.read(cx).content().to_string();

                let Some(node) = self.doc.node(id) else {
                    return;
                };
                if node.speaker == speaker && node.text == text && node.alias == alias {
                    return;
                }
                self.checkpoint_once();
                if let Some(node) = self.doc.node_mut(id) {
                    node.speaker = speaker;
                    node.text = text;
                    node.alias = alias;
                }
            }
            (None, Selection::Edge(index)) => {
                let index = *index;
                let label = self.label_input.read(cx).content().to_string();

                let Some(edge) = self.doc.edges.get(index) else {
                    return;
                };
                if edge.label == label {
                    return;
                }
                self.checkpoint_once();
                self.doc.edges[index].label = label;
            }
            _ => return,
        }
        cx.notify();
    }

    // ponytail: one undo checkpoint per editing session, not per keystroke.
    fn checkpoint_once(&mut self) {
        if !self.text_dirty {
            self.doc.checkpoint();
            self.text_dirty = true;
        }
    }

    /// Mouse positions arrive in window coordinates; the canvas pane starts below the menu
    /// bar and to the right of the variables pane. Miss either offset and every hit test in
    /// the editor is wrong by that much.
    fn window_pos(pos: Point<Pixels>) -> (f32, f32) {
        (f32::from(pos.x) - VARS_W, f32::from(pos.y) - MENU_H)
    }

    fn doc_pos(&self, pos: Point<Pixels>) -> (f32, f32) {
        self.view.to_doc(Self::window_pos(pos))
    }

    /// Screen position of a document point.
    fn screen_pos(&self, doc: (f32, f32)) -> Point<Pixels> {
        let (x, y) = self.view.to_screen(doc);
        point(px(x), px(y))
    }

    /// Centre of the canvas pane, which is what the keyboard zoom commands anchor on.
    fn pane_center(window: &Window) -> (f32, f32) {
        let (w, h) = Self::pane_size(window);
        (w / 2.0, h / 2.0)
    }

    /// The canvas pane's size: the window less the chrome round it. The one place the pane's
    /// *height* is read, which is why `STATUS_H` is subtracted here and nowhere else.
    fn pane_size(window: &Window) -> (f32, f32) {
        let size = window.viewport_size();
        (
            f32::from(size.width) - SIDEBAR_W - VARS_W,
            f32::from(size.height) - MENU_H - STATUS_H,
        )
    }

    /// Fits the view to the canvas on screen, no further out than `min_zoom`. An empty canvas
    /// has nothing to fit and gets the default view. Pan and zoom only — never an edit, so no
    /// checkpoint.
    fn fit_view(&mut self, min_zoom: f32, window: &Window) {
        self.view = match self.doc.extent(&self.flow) {
            Some(extent) => View::fit(extent, Self::pane_size(window), FIT_MARGIN, min_zoom),
            None => View::default(),
        };
    }

    /// F, or View → Fit to view: everything on this canvas, however small that makes it. Open
    /// is gentler (`OPEN_FIT_MIN_ZOOM`) because it has to leave text readable; asking for it
    /// by name is asking for the overview.
    fn on_fit_view(&mut self, _: &FitView, window: &mut Window, cx: &mut Context<Self>) {
        self.fit_view(0.0, window);
        cx.notify();
    }

    /// One entry per edge: the line from the source's out-port to the target's in-port, and
    /// the colour to draw it in — the selection outranks a condition's branch, which
    /// outranks the ordinary edge colour. An edge leaving a jump is not drawn at all: the
    /// node itself names its target, which is the whole point of the kind.
    fn edge_lines(&self) -> Vec<(Point<Pixels>, Point<Pixels>, Rgba, f32)> {
        let path = self.play.as_deref().unwrap_or(&[]);
        self.doc
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.doc.is_jump(e.from))
            .filter_map(|(index, e)| {
                let from = self.doc.node(e.from)?;
                let to = self.doc.node(e.to)?;
                // An edge belongs to the canvas both its ends stand on — the same test
                // `Doc::edge_at` clicks it by, so a line is never drawn where it cannot be
                // hit, nor hit where it is not drawn.
                if from.flow != self.flow || to.flow != self.flow {
                    return None;
                }
                // The trail is read off the play path, the way `Doc::spent` is: nothing is
                // stored beside it. It is the weight of the line that says "walked", so a
                // condition's pin keeps its own colour — the trail hue is the true pin's.
                let walked = path.windows(2).any(|w| w[0] == e.from && w[1] == e.to);
                let spent = self.doc.spent(path, e.from, e.to);
                let color = match (self.selection == Selection::Edge(index), e.branch) {
                    (true, _) => theme::ACCENT,
                    _ if spent => theme::SPENT,
                    _ if !self.doc.is_condition(e.from) => {
                        if walked {
                            theme::TRAIL
                        } else {
                            theme::EDGE
                        }
                    }
                    (false, true) => theme::TRUE,
                    (false, false) => theme::FALSE,
                };
                Some((
                    self.screen_pos(out_port(from)),
                    self.screen_pos(in_port(to)),
                    color,
                    if walked { TRAIL_W } else { EDGE_W },
                ))
            })
            .collect()
    }

    /// The rubber band drawn while dragging a new edge out of a port.
    fn pending_edge(&self) -> Option<(Point<Pixels>, Point<Pixels>)> {
        let Some(Drag::Edge { from, cursor }) = self.drag else {
            return None;
        };
        let node = self.doc.node(from)?;
        Some((self.screen_pos(out_port(node)), self.screen_pos(cursor)))
    }

    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.selection = selection.clone();
        if self.edited != selection {
            self.load_fields(selection, cx);
        }
    }

    /// The rectangle a marquee drag currently covers: top-left and size, in screen pixels.
    fn marquee_rect(&self) -> Option<(Point<Pixels>, Point<Pixels>)> {
        let Some(Drag::Marquee { start, cursor }) = self.drag else {
            return None;
        };
        let top_left = self.screen_pos((start.0.min(cursor.0), start.1.min(cursor.1)));
        let bottom_right = self.screen_pos((start.0.max(cursor.0), start.1.max(cursor.1)));
        Some((top_left, bottom_right - top_left))
    }

    /// Starts a pan. Bound to the middle button, and to Alt+left for pointers without one.
    fn on_pan_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.open_menu = None;
        self.drag = Some(Drag::Pan {
            start_mouse: Self::window_pos(ev.position),
            start_pan: self.view.pan,
        });
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.modifiers.alt {
            return self.on_pan_down(ev, window, cx);
        }
        window.focus(&self.focus_handle);
        self.open_menu = None;
        let (x, y) = self.doc_pos(ev.position);

        // The port sits on top of the node box, so it is tested first.
        if let Some(from) = self.doc.port_hit_test(&self.flow, x, y) {
            self.drag = Some(Drag::Edge {
                from,
                cursor: (x, y),
            });
        } else if let Some(id) = self.doc.hit_test(&self.flow, x, y) {
            if ev.modifiers.shift {
                // Toggling is an edit to the set, not a grab, so no drag starts.
                self.select(self.selection.toggled(id), cx);
                self.drag = None;
                return cx.notify();
            }
            let node = self.doc.node(id).expect("hit_test returned a live node");
            let grab = (x - node.x, y - node.y);
            // Grabbing a node that is already selected keeps the set, which is what
            // makes a group drag possible; grabbing any other node replaces it.
            if !self.selection.nodes().contains(&id) {
                self.select(Selection::Nodes(vec![id]), cx);
            }
            self.drag = Some(Drag::Node {
                id,
                grab,
                moved: false,
            });
            if ev.click_count == 2 {
                window.focus(&self.text_input.read(cx).focus_handle(cx));
            }
        } else if let Some(index) = self.doc.edge_at(&self.flow, x, y) {
            self.select(Selection::Edge(index), cx);
            self.drag = None;
        } else if ev.click_count == 2 {
            self.doc.checkpoint();
            let id = self.doc.add_node(x, y);
            self.select(Selection::Nodes(vec![id]), cx);
            self.drag = None;
            window.focus(&self.speaker_input.read(cx).focus_handle(cx));
        } else {
            // Shift keeps the current set so the band can extend it; a plain drag replaces it.
            if !ev.modifiers.shift {
                self.select(Selection::None, cx);
            }
            self.drag = Some(Drag::Marquee {
                start: (x, y),
                cursor: (x, y),
            });
        }
        cx.notify();
    }

    /// Completes an edge drag when released over a node and commits a marquee to the
    /// selection; every other drag just ends.
    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Edge { from, .. }) => {
                let (x, y) = self.doc_pos(ev.position);
                if let Some(to) = self.doc.hit_test(&self.flow, x, y)
                    && from != to
                    && !self.doc.has_edge(from, to)
                {
                    self.doc.checkpoint();
                    self.doc.add_edge(from, to);
                    self.select(Selection::Edge(self.doc.edges.len() - 1), cx);
                }
            }
            Some(Drag::Marquee { start, cursor }) => {
                let mut ids = self.selection.nodes().to_vec();
                for id in self.doc.nodes_in_rect(&self.flow, start, cursor) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                // A plain click is a zero-size band that touches nothing, so this is
                // also what clears the selection.
                self.select(Selection::of(ids), cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };

        match drag {
            Drag::Node { id, grab, moved } => {
                // Checkpoint on the first movement, so a plain click leaves no empty undo entry.
                if !moved {
                    self.doc.checkpoint();
                    self.drag = Some(Drag::Node {
                        id,
                        grab,
                        moved: true,
                    });
                }
                // The grabbed node is anchored to the cursor and the rest of the selection
                // rides along by the same delta, so a group never drifts apart.
                let (x, y) = self.doc_pos(ev.position);
                let Some(node) = self.doc.node(id) else {
                    return;
                };
                let (dx, dy) = (x - grab.0 - node.x, y - grab.1 - node.y);
                let ids = self.selection.nodes().to_vec();
                self.doc.move_nodes(&ids, dx, dy);
            }
            Drag::Edge { from, .. } => {
                self.drag = Some(Drag::Edge {
                    from,
                    cursor: self.doc_pos(ev.position),
                });
            }
            Drag::Pan {
                start_mouse,
                start_pan,
            } => {
                // Pan is in screen pixels, so this is the same arithmetic at every zoom.
                let (x, y) = Self::window_pos(ev.position);
                self.view.pan = (
                    start_pan.0 + x - start_mouse.0,
                    start_pan.1 + y - start_mouse.1,
                );
            }
            Drag::Marquee { start, .. } => {
                self.drag = Some(Drag::Marquee {
                    start,
                    cursor: self.doc_pos(ev.position),
                });
            }
        }
        cx.notify();
    }

    fn on_delete(&mut self, _: &DeleteNode, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection == Selection::None {
            return;
        }
        // One checkpoint for the whole set, so a group deletion is a single undo step.
        self.doc.checkpoint();
        match self.selection.clone() {
            Selection::Nodes(ids) => ids.iter().for_each(|id| self.doc.remove_node(*id)),
            Selection::Edge(index) => self.doc.remove_edge(index),
            Selection::None => {}
        }
        self.select(Selection::None, cx);
        cx.notify();
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.undo() {
            self.after_history(cx);
        }
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.redo() {
            self.after_history(cx);
        }
    }

    /// Stepping through history can restore or drop whatever was selected, and it moves
    /// the document out from under the sidebar, so the fields always reload.
    fn after_history(&mut self, cx: &mut Context<Self>) {
        let live: Vec<NodeId> = self
            .selection
            .nodes()
            .iter()
            .copied()
            .filter(|id| self.doc.node(*id).is_some())
            .collect();
        let selection = Selection::of(live);
        self.selection = selection.clone();
        // A flow an undo removed would leave the author on a canvas nothing lists.
        if !self.flow.is_empty() && !self.doc.all_flows().contains(&self.flow.as_str()) {
            self.flow.clear();
        }
        self.load_fields(selection, cx);
        cx.notify();
    }

    /// Wheel zooms about the cursor, so whatever you are pointing at stays put.
    fn on_scroll(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        if dy == 0.0 {
            return;
        }
        self.view
            .zoom_at(ZOOM_PER_PIXEL.powf(dy), Self::window_pos(ev.position));
        cx.notify();
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(ZOOM_STEP, window, cx);
    }

    fn on_zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(1.0 / ZOOM_STEP, window, cx);
    }

    /// Back to 1:1, expressed as the factor that gets there — so there is no second
    /// code path that could disagree with [`View::zoom_at`] about where pan lands.
    fn on_zoom_reset(&mut self, _: &ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(1.0 / self.view.zoom, window, cx);
    }

    fn zoom_by(&mut self, factor: f32, window: &Window, cx: &mut Context<Self>) {
        self.view.zoom_at(factor, Self::pane_center(window));
        cx.notify();
    }

    /// Opens the find overlay with the field focused, ready to type. The field is only in
    /// the tree while `searching` is set; focusing it in the same turn works because the
    /// handle exists before the element does, the way `insert_node` focuses the speaker field.
    fn on_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        self.searching = true;
        window.focus(&self.search_input.read(cx).focus_handle(cx));
        cx.notify();
    }

    fn on_toggle_problems(&mut self, _: &ToggleProblems, _: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        self.problems_open = !self.problems_open;
        cx.notify();
    }

    fn on_show_help(&mut self, _: &ShowHelp, _: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        self.help_open = !self.help_open;
        cx.notify();
    }

    /// Ctrl+K. Toggles, and opens on a clean slate with the field focused. It closes every
    /// other overlay first: the palette is modal, and a find card left open underneath it
    /// would take the focus back the moment it was clicked.
    fn on_show_palette(&mut self, _: &ShowPalette, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = self.palette_open;
        self.dismiss_overlays(cx);
        if was_open {
            window.focus(&self.focus_handle);
        } else {
            self.palette_open = true;
            self.palette_sel = 0;
            window.focus(&self.palette_input.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    /// Closes the palette alone, for the handlers that open another overlay over it.
    fn close_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            self.palette_input
                .update(cx, |input, cx| input.set_content("", cx));
        }
    }

    /// Moves the highlighted row, wrapping at both ends so Up from the top reaches the last
    /// command instead of stopping dead.
    fn palette_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let query = self.palette_input.read(cx).content().to_string();
        let rows = palette_entries(&query).len();
        if rows == 0 {
            return;
        }
        let from = self.palette_sel.min(rows - 1) as isize;
        self.palette_sel = (from + delta).rem_euclid(rows as isize) as usize;
        self.palette_scroll.scroll_to_item(self.palette_sel);
        cx.notify();
    }

    fn on_palette_next(&mut self, _: &PaletteNext, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_step(1, cx);
    }

    fn on_palette_prev(&mut self, _: &PalettePrev, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_step(-1, cx);
    }

    /// Enter: runs the highlighted command.
    fn on_palette_run(&mut self, _: &PaletteRun, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.palette_input.read(cx).content().to_string();
        let entries = palette_entries(&query);
        let row = self.palette_sel.min(entries.len().saturating_sub(1));
        if let Some((_, (_, _, action))) = entries.get(row) {
            self.run_command(*action, window, cx);
        }
    }

    /// Runs a command picked from the palette. The palette closes and the canvas takes the
    /// focus *first*, so a command that wants somewhere else — Find focuses its own field —
    /// can have it, and one that closes overlays (Stop) finds none left to close and does
    /// its real job instead.
    fn run_command(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_overlays(cx);
        window.focus(&self.focus_handle);
        action(self, window, cx);
    }

    /// Closes the find overlay, the problems drawer, the cheat sheet and the palette. True when there was
    /// something to close, which is how Escape knows it has already done its job. The query
    /// is cleared with the overlay, so find always opens on a clean slate.
    fn dismiss_overlays(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.searching || self.problems_open || self.help_open || self.palette_open;
        if self.searching {
            self.search_input
                .update(cx, |input, cx| input.set_content("", cx));
        }
        self.close_palette(cx);
        self.searching = false;
        self.problems_open = false;
        self.help_open = false;
        if any {
            cx.notify();
        }
        any
    }

    /// Creates a node already connected to the selected one and drops the cursor in its
    /// Speaker field, so a branch is one keystroke instead of a double-click and a drag.
    fn on_branch(&mut self, _: &Branch, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_node(NodeKind::Speech, window, cx);
    }

    fn on_new_hub(&mut self, _: &NewHub, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_node(NodeKind::Hub, window, cx);
    }

    fn on_new_jump(&mut self, _: &NewJump, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_node(NodeKind::Jump, window, cx);
    }

    fn on_new_instruction(
        &mut self,
        _: &NewInstruction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_node(NodeKind::Instruction, window, cx);
    }

    fn on_new_condition(&mut self, _: &NewCondition, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_node(NodeKind::Condition, window, cx);
    }

    /// Adds a node of `kind`, hung off the selected one when there is exactly one — a
    /// branch needs a single parent — and dropped in the middle of the view otherwise, so
    /// the command always does something rather than silently refusing.
    fn on_new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_node(NodeKind::Note, window, cx);
    }

    fn insert_node(&mut self, kind: NodeKind, window: &mut Window, cx: &mut Context<Self>) {
        // Guard before the checkpoint: one whose mutation then fails is an empty undo
        // step and a false dirty flag.
        // A note takes no edges, so it can never branch off the selection: it always lands
        // in the middle of the view. Without this the branch would be created and the edge
        // silently refused, leaving the note parked where a child would have gone.
        let parent = self
            .selection
            .only_node()
            .filter(|_| kind != NodeKind::Note)
            .filter(|id| self.doc.node(*id).is_some());

        self.doc.checkpoint();
        let id = match parent {
            Some(from) => match self.doc.branch_from(from) {
                Some(id) => id,
                None => return,
            },
            None => {
                // Centred on the view rather than parked at its origin, so the node lands
                // where you are already looking.
                let (x, y) = self.view.to_doc(Self::pane_center(window));
                // Slid down past anything already standing there: dropped dead on top of
                // another node, the new one hid it, and a second Ctrl+6 hid the first.
                let (x, y) = self
                    .doc
                    .free_spot(&self.flow, x - NODE_W / 2.0, y - NODE_H / 2.0);
                let id = self.doc.add_node(x, y);
                // Onto the canvas being looked at. The branch arm needs no equivalent:
                // `branch_from` inherits the parent's, and switching canvas drops the
                // selection, so a parent is always on this one.
                let flow = self.flow.clone();
                if let Some(node) = self.doc.node_mut(id) {
                    node.flow = flow;
                }
                id
            }
        };
        if let Some(node) = self.doc.node_mut(id) {
            node.kind = kind;
        }
        self.select(Selection::Nodes(vec![id]), cx);
        // Only a speech node has a field worth landing in; every other kind is edited with
        // pickers, so the keyboard stays on the canvas.
        if kind == NodeKind::Speech {
            window.focus(&self.speaker_input.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    /// Plays the conversation the selection is standing in, from its top.
    fn on_play(&mut self, _: &Play, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(seed) = self.play_seed() {
            self.start_play(self.doc.start_of(seed), window, cx);
        }
    }

    /// Plays from the selected node itself, for testing one branch without its run-up.
    fn on_play_from_here(&mut self, _: &PlayFromHere, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(seed) = self.play_seed() {
            self.start_play(seed, window, cx);
        }
    }

    /// Escape. Closes an overlay if one is open and only otherwise stops the play-through,
    /// so Escape peels back one layer at a time instead of doing both at once.
    fn on_stop_play(&mut self, _: &StopPlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_overlays(cx) {
            return;
        }
        self.play = None;
        cx.notify();
    }

    /// The node play starts from: the selected one, or the first on the current canvas so
    /// that a stray F5 plays something rather than nagging. The walk itself crosses flows
    /// freely — only the seed is scoped.
    fn play_seed(&self) -> Option<NodeId> {
        // A note is not in anybody's conversation, so seeding play from one would only ever
        // report running out of graph.
        self.selection
            .only_node()
            .filter(|id| !self.doc.is_note(*id))
            .or_else(|| {
                self.doc
                    .nodes_in(&self.flow)
                    .find(|n| n.kind != NodeKind::Note)
                    .map(|n| n.id)
            })
    }

    /// Both entry points walk on through everything the player does not see, so the path
    /// always ends on a node worth showing. The nodes walked *through* stay in it — that
    /// is what lets `Doc::vars_along` recompute the variables from the path alone.
    fn start_play(&mut self, at: NodeId, window: &Window, cx: &mut Context<Self>) {
        let path = self.doc.start_walk(at);
        self.follow(&path, window);
        self.play = Some(path);
        cx.notify();
    }

    /// Advances the play-through, or ends it if the choice leads nowhere real.
    fn play_advance(&mut self, to: NodeId, window: &Window, cx: &mut Context<Self>) {
        let Some(mut path) = self.play.take() else {
            return;
        };
        self.doc.advance(&mut path, to);
        self.follow(&path, window);
        self.play = Some(path);
        cx.notify();
    }

    /// Brings the node a play-through has arrived at into view.
    fn follow(&mut self, path: &[NodeId], window: &Window) {
        if let Some(&at) = path.last() {
            self.show(at, window);
        }
    }

    /// Brings a node into view without touching the selection — which is what separates
    /// following a play-through from [`Editor::jump_to`].
    /// Switching the canvas lives here rather than in each caller: `jump_to` and `follow`
    /// both come through, so a search hit, a problem row and a play-through that walks into
    /// another flow all cross canvases without an arm of their own.
    fn show(&mut self, id: NodeId, window: &Window) {
        if let Some(node) = self.doc.node(id) {
            self.flow.clone_from(&node.flow);
            self.view
                .center_on(node_center(node), Self::pane_center(window));
        }
    }

    /// Selects a node and brings it into view without changing zoom.
    fn jump_to(&mut self, id: NodeId, window: &Window, cx: &mut Context<Self>) {
        if self.doc.node(id).is_none() {
            return;
        }
        self.show(id, window);
        self.select(Selection::Nodes(vec![id]), cx);
        cx.notify();
    }

    /// Selects an edge by index and shows whichever end of it still exists — the source
    /// first, which for a cross-flow edge is the canvas whose node needs to become a jump.
    /// Focuses the canvas so Delete reaches it straight from the problem row.
    fn select_edge(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edge) = self.doc.edges.get(index) else {
            return;
        };
        let end = [edge.from, edge.to]
            .into_iter()
            .find(|id| self.doc.node(*id).is_some());
        if let Some(id) = end {
            self.show(id, window);
        }
        self.select(Selection::Edge(index), cx);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Escape from inside a text field. Closes the find overlay or the palette when that is
    /// where the field is, since each is nothing but that field and its results.
    fn on_focus_canvas(&mut self, _: &FocusCanvas, window: &mut Window, cx: &mut Context<Self>) {
        if self.searching || self.palette_open {
            self.dismiss_overlays(cx);
        }
        window.focus(&self.focus_handle);
    }

    /// Switches the left pane's tab. A rename aimed at the list you just left would rename
    /// the wrong thing, so switching tabs puts the shared field back to declaring.
    fn switch_pane(&mut self, pane: SidePane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.renaming = None;
        self.set = String::new();
        self.var_input.update(cx, |input, cx| {
            input.set_content("", cx);
            // The hint is how one field says which list its `+` adds to.
            input.set_placeholder(declare_hint(pane), cx);
        });
        cx.notify();
    }

    /// Puts another flow on the canvas. Only the view moves, so no checkpoint: which canvas
    /// you are looking at is not a document edit. The selection goes, though — nodes on the
    /// canvas you left are neither drawn nor hit-testable, so leaving them selected would let
    /// the inspector edit an invisible node and `Tab` branch off one, which is the only way
    /// to author a `CrossFlowEdge`. The view refits, because the pan and zoom you left were
    /// aimed at another canvas and may point at nothing here. Choosing the flow you are
    /// already on changes nothing, so a stray click on its tab does not throw away your pan.
    /// `show` deliberately does not refit: it centres on one node, which is the point of it.
    fn show_flow(&mut self, flow: &str, cx: &mut Context<Self>) {
        if self.flow != flow {
            self.flow = flow.to_string();
            self.fit_pending = true;
            self.select(Selection::None, cx);
        }
        cx.notify();
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = format!(
            "palaver — {}{}{}",
            self.file_name(),
            // Omitted on the main flow, so a document that never made one reads as it did.
            if self.flow.is_empty() {
                String::new()
            } else {
                format!(" › {}", self.flow)
            },
            if self.doc.is_dirty() { "*" } else { "" }
        );
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        // Before anything below reads the view. A one-shot, so nothing here runs per frame.
        if self.fit_pending {
            self.fit_pending = false;
            self.fit_view(OPEN_FIT_MIN_ZOOM, window);
        }
        let lines = self.edge_lines();
        let pending = self.pending_edge();
        let view = self.view;
        // Once per frame, for the status bar's badge, the drawer and the inspector's summary.
        let problems = self.doc.problems();
        let canvas_w = f32::from(window.viewport_size().width) - VARS_W - SIDEBAR_W;

        // The `"Editor"` context and every action listener live here, at the root, not on
        // the canvas: gpui matches a binding against the focused element's *ancestors*, and
        // every text field sits in the vars pane or the sidebar, beside the canvas rather
        // than inside it. On the canvas, Ctrl+S while typing a line simply did nothing.
        div()
            .key_context("Editor")
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_reset))
            .on_action(cx.listener(Self::on_fit_view))
            .on_action(cx.listener(Self::on_find))
            .on_action(cx.listener(Self::on_toggle_problems))
            .on_action(cx.listener(Self::on_show_help))
            .on_action(cx.listener(Self::on_show_palette))
            .on_action(cx.listener(Self::on_palette_next))
            .on_action(cx.listener(Self::on_palette_prev))
            .on_action(cx.listener(Self::on_palette_run))
            .on_action(cx.listener(Self::on_branch))
            .on_action(cx.listener(Self::on_new_hub))
            .on_action(cx.listener(Self::on_new_jump))
            .on_action(cx.listener(Self::on_new_instruction))
            .on_action(cx.listener(Self::on_new_condition))
            .on_action(cx.listener(Self::on_new_note))
            .on_action(cx.listener(Self::on_focus_canvas))
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_export_fluent))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_play))
            .on_action(cx.listener(Self::on_play_from_here))
            .on_action(cx.listener(Self::on_stop_play))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .font_family(theme::UI_FONT)
            .text_size(px(theme::BODY))
            .child(self.menu_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.vars_pane(cx))
                    .child(self.canvas_pane(lines, pending, view, cx))
                    .child(self.sidebar(&problems, cx)),
            )
            .child(self.status_bar(&problems, cx))
            // Overlays come last so they paint above everything, and are only in the tree
            // while open: the find field in particular is not rendered when closed.
            .when(self.searching, |this| {
                this.child(self.search_overlay(canvas_w, cx))
            })
            .when(self.help_open, |this| this.child(self.help_overlay(cx)))
            .when(self.palette_open, |this| {
                this.child(self.palette_overlay(cx))
            })
    }
}

impl Editor {
    fn menu_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(MENU_H))
            .items_center()
            .px_1()
            .bg(theme::PANE)
            .border_b_1()
            .border_color(theme::DIVIDER)
            .text_color(theme::TEXT)
            .children(MENUS.iter().map(|(name, items)| {
                let open = self.open_menu == Some(*name);
                div()
                    .relative()
                    .child(
                        div()
                            .id(*name)
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .when(open, |this| this.bg(theme::PRESSED))
                            .hover(|style| style.bg(theme::HOVER))
                            .child(*name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_menu = (!open).then_some(*name);
                                cx.notify();
                            })),
                    )
                    .when(open, |this| this.child(menu_dropdown(items, cx)))
            }))
    }

    /// The graph itself. Owns the mouse handling, so sidebar clicks never reach it, and
    /// the focus: its `"Canvas"` context is where the keys that mean something else in a
    /// text field are bound — Delete, Backspace and Tab — so they never fire from one.
    fn canvas_pane(
        &self,
        lines: Vec<(Point<Pixels>, Point<Pixels>, Rgba, f32)>,
        pending: Option<(Point<Pixels>, Point<Pixels>)>,
        view: View,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .key_context("Canvas")
            .track_focus(&self.focus_handle)
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(theme::CANVAS)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_pan_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        // Paint coordinates are window-absolute, canvas bounds are not at the origin.
                        let mut draw =
                            |from: Point<Pixels>, to: Point<Pixels>, color: Rgba, width: f32| {
                                let (from, to) = (from + bounds.origin, to + bounds.origin);
                                let mut line = PathBuilder::stroke(px(width));
                                line.move_to(from);
                                line.line_to(to);
                                if let Ok(line) = line.build() {
                                    window.paint_path(line, color);
                                }
                                // The stroke stays a hairline at every zoom; only the head scales.
                                if let Some(head) = arrow_head(to, from, view.zoom) {
                                    window.paint_path(head, color);
                                }
                            };

                        for (from, to, color, width) in lines {
                            draw(from, to, color, width);
                        }
                        if let Some((from, to)) = pending {
                            draw(from, to, theme::ACCENT, EDGE_W);
                        }
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .children(self.doc.nodes_in(&self.flow).map(|node| {
                let playing = self.play.as_ref().and_then(|path| path.last().copied());
                let passed = self
                    .play
                    .as_ref()
                    .is_some_and(|path| path.contains(&node.id));
                let jump_target = (node.kind == NodeKind::Jump)
                    .then(|| self.doc.choices(node.id).first().map(|&(to, _)| to))
                    .flatten()
                    .and_then(|to| self.doc.node(to))
                    .map(|target| node_label(&self.doc, target));
                node_box(
                    node,
                    view,
                    self.selection.nodes().contains(&node.id),
                    playing == Some(node.id),
                    passed,
                    jump_target,
                    speaker_color(&self.doc, &node.speaker, theme::SPEAKER_DEFAULT),
                )
            }))
            // Over the nodes, not under them: a label that a node box painted over was cut off
            // mid-word. Each is bounded to the gap it sits in, so it covers no text anyway.
            .children(self.edge_labels())
            .children(
                self.doc
                    .nodes_in(&self.flow)
                    // A note takes no edges, so it gets no handle to drag one from —
                    // `Doc::port_hit_test` skips it for the same reason.
                    .filter(|node| node.kind != NodeKind::Note)
                    .map(|node| port_dot(node, view)),
            )
            .children(
                self.doc
                    .roots()
                    .into_iter()
                    .filter_map(|id| self.doc.node(id))
                    .filter(|node| node.flow == self.flow)
                    .map(|node| root_dot(node, view)),
            )
            .when_some(self.marquee_rect(), |this, (origin, size)| {
                this.child(
                    div()
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(size.x)
                        .h(size.y)
                        .bg(theme::ACCENT_WASH)
                        .border_1()
                        .border_color(theme::ACCENT),
                )
            })
    }

    /// Choice text on each labelled edge, as a pill centred on the line's midpoint.
    ///
    /// The width is the *gap* the line crosses. An edge runs from a node's right edge to the
    /// next one's left edge, so its midpoint is the middle of that gap, and a label wider than
    /// the gap ran underneath a node box and was cut off mid-word. Bounded to the gap it
    /// covers no text, whatever the order things paint in. What does not fit wraps to a
    /// second line and then ends in `…` — see `Typography::one_line` for why the ellipsis
    /// needs the clamp rather than `truncate()`.
    fn edge_labels(&self) -> Vec<impl IntoElement> {
        /// Room left either side of the pill, so it never touches a port or a box: a port
        /// dot is `2 * PORT_R` across and centred on the gap's edge, so this clears it with a
        /// few units to spare.
        const MARGIN: f32 = 2.0 * PORT_R + 10.0;
        /// What a label may be where there is no gap to fit it in: an edge running back
        /// across the graph, where every width is equally arbitrary.
        const BACK_W: f32 = 120.0;
        const MIN_W: f32 = 48.0;
        const MAX_W: f32 = 200.0;
        /// What a character of the label font may take at 1:1, deliberately over the real
        /// average (~6.8): the estimate is a *width given to the text*, so one that is short
        /// wraps a word in the middle ("As" / "k"), while one that is long only pads a pill.
        const CHAR_W: f32 = 7.4;
        const PAD_X: f32 = 6.0;
        /// A pill's border, both sides, plus a little slack for the rounding gpui does.
        const FRAME: f32 = 6.0;
        /// Two lines of label text with the pill's padding, in document units.
        const H: f32 = 48.0;

        self.doc
            .edges
            .iter()
            // A condition's edges always carry a label — which pin they leave by — even
            // though the player never chooses one, so the empty-label skip lets them past.
            .filter(|edge| {
                !self.doc.is_jump(edge.from)
                    && (!edge.label.is_empty()
                        || self.doc.is_condition(edge.from)
                        || self.doc.is_once(edge))
            })
            .filter_map(|edge| {
                let (text, color) = if self.doc.is_condition(edge.from) {
                    match edge.branch {
                        true => ("true".to_string(), theme::TRUE),
                        false => ("false".to_string(), theme::FALSE),
                    }
                } else if self.doc.is_once(edge) {
                    // Shown on the canvas as well as in the inspector, the way a condition's
                    // branch is: a flag you have to click every edge to find is guessed at.
                    // In front of the label, not after it: a long choice is truncated, so a
                    // marker on the end is one you cannot see. `is_once`, not the bare flag,
                    // so one left behind by a kind change is not drawn where the inspector
                    // offers no toggle for it.
                    (
                        format!("(once) {}", edge.label).trim_end().to_string(),
                        theme::TEXT_MUTED,
                    )
                } else {
                    (edge.label.clone(), theme::TEXT_MUTED)
                };
                let (from_node, to_node) = (self.doc.node(edge.from)?, self.doc.node(edge.to)?);
                // Skipped for the reason `edge_lines` skips it, and through the same test so
                // the two cannot drift.
                if from_node.flow != self.flow || to_node.flow != self.flow {
                    return None;
                }
                let from = out_port(from_node);
                let to = in_port(to_node);
                let gap = to.0 - from.0;
                let fit = if gap > 0.0 { gap - MARGIN } else { BACK_W };
                let mid = self.screen_pos(((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
                // A *definite* width for the pill, estimated from the text and capped at the
                // gap. Left to size itself against a `max_w`, gpui measured it at its full
                // width first and then wrapped one line wider than the box, clipping it on
                // both sides with no ellipsis; given a width it wraps and truncates the way a
                // node's text does. Generous per character, so a label that fits stays on one
                // line, and a short one is not stretched to the gap.
                let wanted = text.chars().count() as f32 * CHAR_W + 2.0 * PAD_X + FRAME;
                let pill = wanted.min(fit.clamp(MIN_W, MAX_W));
                let width = self.view.scale(MAX_W);
                let height = self.view.scale(H);
                Some(
                    // A fixed box the pill is centred in, so it centres on the line without
                    // being measured — and so one line or two sits on the same midpoint.
                    div()
                        .absolute()
                        .left(mid.x - px(width / 2.0))
                        .top(mid.y - px(height / 2.0))
                        .w(px(width))
                        .h(px(height))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(self.view.scale(theme::SMALL)))
                        .child(
                            div()
                                .w(px(self.view.scale(pill)))
                                .px(px(self.view.scale(PAD_X)))
                                .py(px(self.view.scale(2.0)))
                                .rounded_md()
                                .bg(theme::RAISED)
                                .border_1()
                                .border_color(theme::OUTLINE)
                                .text_color(color)
                                .text_center()
                                .text_ellipsis()
                                .line_clamp(2)
                                .child(text),
                        ),
                )
            })
            .collect()
    }

    /// Find, as a card over the top of the canvas: opened by Ctrl+F, closed by Escape. It
    /// lived at the head of the inspector, which made the one control that is about the
    /// whole document the first thing in a panel about the selection — and left a permanent
    /// text field on screen for something used in bursts.
    ///
    /// It stays open while you click through results, so a run of hits can be walked; the
    /// canvas is still live underneath it for the same reason.
    fn search_overlay(&self, canvas_w: f32, cx: &mut Context<Self>) -> impl IntoElement {
        /// Beyond this the list stops being scannable, even scrolled.
        const MAX_ROWS: usize = 50;
        const CARD_W: f32 = 420.0;

        let query = self.search_input.read(cx).content().to_string();
        // ponytail: rescans every node each frame, but only while the overlay is open and a
        // query is typed — which is never while dragging. Cache it if searching a huge graph
        // ever stutters.
        let hits = self.doc.find(&query);
        let typed = !query.trim().is_empty();
        let more = hits.len().saturating_sub(MAX_ROWS);

        deferred(
            div()
                .absolute()
                .top(px(MENU_H + 12.0))
                .left(px(VARS_W + ((canvas_w - CARD_W) / 2.0).max(8.0)))
                .w(px(CARD_W))
                .occlude()
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .bg(theme::RAISED)
                .border_1()
                .border_color(theme::OUTLINE)
                .rounded_md()
                .shadow_lg()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .child(self.search_input.clone())
                .child(
                    div()
                        .id("search-results")
                        .flex()
                        .flex_col()
                        .max_h(px(280.0))
                        .overflow_y_scroll()
                        .when(!typed, |this| {
                            this.child(
                                div()
                                    .px_1()
                                    .type_hint()
                                    .child("Search speakers, lines and aliases."),
                            )
                        })
                        .when(typed && hits.is_empty(), |this| {
                            this.child(div().px_1().type_hint().child("No matches."))
                        })
                        .children(hits.iter().take(MAX_ROWS).filter_map(|&id| {
                            let node = self.doc.node(id)?;
                            let (glyph, _, color) = kind_style(node.kind);
                            Some(
                                div()
                                    .id(("search-result", id))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::HOVER))
                                    .child(div().flex_none().w_4().text_color(color).child(glyph))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .one_line()
                                            .text_color(theme::TEXT)
                                            .child(node_label(&self.doc, node)),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.jump_to(id, window, cx)
                                    })),
                            )
                        }))
                        .when(more > 0, |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .type_hint()
                                    .child(format!("…and {more} more")),
                            )
                        }),
                ),
        )
    }

    /// The document's global variables, in a pane of their own on the left — they belong to
    /// the whole document rather than to whatever is selected, which is the one thing the
    /// right-hand inspector is for.
    ///
    /// It stays up during play, where it switches from showing what each variable *starts*
    /// at to what it stands at now, and goes read-only: a click that edited a default while
    /// the pane displayed a live value would be editing one thing and showing another.
    fn vars_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let playing = self.play.as_ref().map(|path| self.doc.vars_along(path));
        // A rename the document has moved on from — undone, deleted, or in a file that has
        // since been closed — is simply not a rename any more.
        let renaming = self
            .renaming
            .clone()
            .filter(|rename| match (rename, self.pane) {
                (Rename::Set(set), _) => self.doc.all_sets().contains(&set.as_str()),
                (Rename::Item(name), SidePane::Variables) => self.doc.variable(name).is_some(),
                (Rename::Item(name), SidePane::Cast) => self.doc.character(name).is_some(),
                (Rename::Item(name), SidePane::Flows) => {
                    self.doc.all_flows().contains(&name.as_str())
                }
            });
        let is_rename = renaming.is_some();
        let renaming_set = matches!(renaming, Some(Rename::Set(_)));
        let pane = self.pane;
        // Aimed at a set the document no longer has is aimed at nothing, exactly as above.
        let set = match self.doc.all_sets().contains(&self.set.as_str()) {
            true => self.set.clone(),
            false => String::new(),
        };
        // Owned for the same reason the flow names are: the picker chips borrow `cx`.
        let sets: Vec<String> = self
            .doc
            .all_sets()
            .into_iter()
            .map(str::to_string)
            .collect();
        // Owned, so the rows can borrow `cx` mutably. The main flow leads, then every other
        // canvas the document has — declared or merely stood on, so no node is unreachable.
        let flows: Vec<String> = std::iter::once(String::new())
            .chain(self.doc.all_flows().into_iter().map(str::to_string))
            .collect();

        // Grouped by the set in the name, in first-seen order, each group carrying the
        // *index* into `doc.variables` — every row id and handler still keys on the whole
        // dotted name, since that is what the document is keyed by.
        let mut groups: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
        for (i, variable) in self.doc.variables.iter().enumerate() {
            let set = split_set(&variable.name).0;
            match groups.iter_mut().find(|(s, _)| *s == set) {
                Some((_, rows)) => rows.push(i),
                None => groups.push((set, vec![i])),
            }
        }
        // Nothing dotted means nothing to group, and a document that never adopted the
        // convention must not grow a header saying so.
        let any_sets = groups.iter().any(|(set, _)| set.is_some());

        div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(VARS_W))
            .flex_none()
            .h_full()
            .p_3()
            .bg(theme::PANE)
            .border_r_1()
            .border_color(theme::DIVIDER)
            .text_color(theme::TEXT)
            // A segmented control in equal thirds. Chips that wrapped left "Flows" alone on a
            // second line, and "Variables (now)" while playing was wider than the pane; the
            // live state is a line of its own below instead of a longer label.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap(px(2.0))
                    .p(px(2.0))
                    .rounded_md()
                    .bg(theme::CANVAS)
                    .children(
                        [
                            (SidePane::Variables, "Variables", "pane-variables"),
                            (SidePane::Cast, "Cast", "pane-cast"),
                            (SidePane::Flows, "Flows", "pane-flows"),
                        ]
                        .map(|(pane, label, element_id)| {
                            let on = pane == self.pane;
                            div()
                                .id(element_id)
                                .flex_1()
                                .flex()
                                .justify_center()
                                .py_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(theme::SMALL))
                                .font_weight(if on {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .when(on, |this| this.bg(theme::PRESSED))
                                .text_color(if on { theme::TEXT } else { theme::TEXT_FAINT })
                                .hover(|style| style.text_color(theme::TEXT))
                                .child(label)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.switch_pane(pane, cx)),
                                )
                        }),
                    ),
            )
            .when(playing.is_some() && self.pane == SidePane::Variables, |this| {
                this.child(
                    div()
                        .type_hint()
                        .text_color(theme::PLAYING)
                        .child("Live values — read-only"),
                )
            })
            .child(
                // The list scrolls, not the pane: the heading above and the declare field
                // below stay put however many variables the document holds. `min_h(0)` is
                // what lets a flex child shrink below its content and actually overflow.
                div()
                    .id("var-list")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .map(|list| match self.pane {
                        SidePane::Cast => list.children(
                            (0..self.doc.characters.len())
                                .map(|i| self.cast_row(i, playing.is_some(), cx)),
                        ),
                        SidePane::Flows => list.children(
                            flows
                                .iter()
                                .cloned()
                                .enumerate()
                                .map(|(i, flow)| self.flow_row(i, flow, playing.is_some(), cx))
                                .collect::<Vec<_>>(),
                        ),
                        SidePane::Variables => list.children(groups.into_iter().map(|(set, rows)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(any_sets, |this| {
                                let header = set.map(str::to_string);
                                this.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex_grow()
                                                .type_label()
                                                .one_line()
                                                .child(header.clone().unwrap_or_else(|| {
                                                    "(ungrouped)".to_string()
                                                })),
                                        )
                                        // `(ungrouped)` gets none: it is not a set, so
                                        // there is no name to rewrite.
                                        .when_some(
                                            header.filter(|_| playing.is_none()),
                                            |this, name| {
                                                this.child(self.set_rename_button(&name, cx))
                                            },
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .when(any_sets, |this| this.pl_2())
                                    .children(rows.into_iter().map(|i| {
                                        self.var_row(i, playing.as_ref(), cx)
                                    })),
                            )
                        })),
                    }),
            )
            .when(playing.is_none(), |this| {
                this.when_some(renaming.clone(), |this, old| {
                    let what = match old {
                        Rename::Set(name) => format!("Rename set \"{name}\""),
                        Rename::Item(name) => format!("Rename \"{name}\""),
                    };
                    this.child(div().type_label().one_line().child(what))
                })
                // Only above the variables, and not while the field holds a set name.
                .when(pane == SidePane::Variables && !renaming_set, |this| {
                    this.child(div().type_section().pt_2().child("DECLARE IN SET"))
                        .child(self.set_picker(&set, &sets, cx))
                })
                .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_grow().child(self.var_input.clone()))
                    // No `TextInput` in the crate binds enter, so the click is the whole
                    // affordance. A blank or duplicate name is simply refused, and the
                    // field keeping its content is what says so — which is also how a
                    // rename to the name it already has reads.
                    .child(
                        div()
                            .id("var-commit")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(theme::RAISED)
                            .text_color(theme::TEXT_MUTED)
                            .hover(|style| style.text_color(theme::TEXT))
                            .child(if is_rename { "✓" } else { "+" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let typed = this.var_input.read(cx).content().to_string();
                                // On the variables tab the field holds only the rest of the
                                // name; the picker holds the set. Not when it is renaming a
                                // set, where the field *is* the set name.
                                let name = match (pane, renaming_set) {
                                    (SidePane::Variables, false) => join_set(&set, typed.trim()),
                                    _ => typed,
                                };
                                let free = match (pane, renaming_set) {
                                    (_, true) => this.doc.set_name_free(&name),
                                    (SidePane::Cast, _) => this.doc.character_name_free(&name),
                                    (SidePane::Variables, _) => {
                                        this.doc.variable_name_free(&name)
                                    }
                                    (SidePane::Flows, _) => this.doc.flow_name_free(&name),
                                };
                                if !free {
                                    return;
                                }
                                this.doc.checkpoint();
                                match (&renaming, pane) {
                                    // Every dotted name in the set follows, and so does
                                    // every effect and check naming one.
                                    (Some(Rename::Set(old)), _) => {
                                        let moved = this.doc.rename_set(old, &name);
                                        let s = if moved == 1 { "" } else { "s" };
                                        let name = name.trim();
                                        if this.set == *old {
                                            this.set = name.to_string();
                                        }
                                        this.set_status(
                                            format!(
                                                "Renamed \"{old}\" to \"{name}\" — {moved} variable{s}."
                                            ),
                                            cx,
                                        );
                                    }
                                    // Every line the character speaks follows the record.
                                    (Some(Rename::Item(old)), SidePane::Cast) => {
                                        let moved = this.doc.rename_character(old, &name);
                                        let s = if moved == 1 { "" } else { "s" };
                                        let name = name.trim();
                                        this.set_status(
                                            format!(
                                                "Renamed \"{old}\" to \"{name}\" — {moved} line{s}."
                                            ),
                                            cx,
                                        );
                                    }
                                    // Effects and checks follow it, so say how many did.
                                    (Some(Rename::Item(old)), SidePane::Variables) => {
                                        let moved = this.doc.rename_variable(old, &name);
                                        let s = if moved == 1 { "" } else { "s" };
                                        let name = name.trim();
                                        this.set_status(
                                            format!(
                                                "Renamed \"{old}\" to \"{name}\" — {moved} reference{s}."
                                            ),
                                            cx,
                                        );
                                    }
                                    // Renaming a canvas carries every node standing on it,
                                    // and the one on screen with them.
                                    (Some(Rename::Item(old)), SidePane::Flows) => {
                                        let moved = this.doc.rename_flow(old, &name);
                                        let s = if moved == 1 { "" } else { "s" };
                                        let name = name.trim();
                                        if this.flow == *old {
                                            this.flow = name.to_string();
                                        }
                                        this.set_status(
                                            format!(
                                                "Renamed \"{old}\" to \"{name}\" — {moved} node{s}."
                                            ),
                                            cx,
                                        );
                                    }
                                    // A colour off the palette, so a new cast comes out tinted
                                    // apart without anybody picking one.
                                    (None, SidePane::Cast) => {
                                        let color = CAST_COLORS
                                            [this.doc.characters.len() % CAST_COLORS.len()];
                                        this.doc.add_character(&name, color);
                                    }
                                    (None, SidePane::Variables) => {
                                        this.doc.add_variable(&name);
                                    }
                                    (None, SidePane::Flows) => {
                                        this.doc.add_flow(&name);
                                    }
                                }
                                this.renaming = None;
                                // The set is deliberately *not* cleared: declaring three
                                // flags into one set should not mean clicking its chip
                                // three times. Switching tabs and cancelling clear it.
                                this.var_input
                                    .update(cx, |input, cx| input.set_content("", cx));
                                cx.notify();
                            })),
                    )
                    .when(is_rename, |this| {
                        this.child(
                            div()
                                .id("var-rename-cancel")
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_color(theme::TEXT_FAINT)
                                .hover(|style| style.text_color(theme::TEXT))
                                .child("✗")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.renaming = None;
                                    this.set = String::new();
                                    this.var_input
                                        .update(cx, |input, cx| input.set_content("", cx));
                                    cx.notify();
                                })),
                        )
                    }),
                )
            })
    }

    /// The target the number editor is aimed at, if the document still has it. A variable or
    /// a node that has gone — undone, deleted, or in a file that has since been closed — is
    /// not a target any more, which is the filtering `renaming` gets and for the same reason:
    /// one place, rather than every site that could invalidate it.
    fn editing_target(&self) -> Option<&ValueTarget> {
        self.editing.as_ref().filter(|target| match target {
            ValueTarget::Default(name) => self.doc.variable(name).is_some(),
            ValueTarget::Effect(id, _) | ValueTarget::Check(id, _) => self.doc.node(*id).is_some(),
        })
    }

    /// Aims the number editor at `target`, starting from the number that is there now.
    fn edit_value(&mut self, target: ValueTarget, from: i64, cx: &mut Context<Self>) {
        self.editing = Some(target);
        self.value_input
            .update(cx, |input, cx| input.set_content(&from.to_string(), cx));
        cx.notify();
    }

    /// A number as it sits in a row: click it to type a new one.
    fn number_cell(
        &self,
        id: (&'static str, usize),
        n: i64,
        target: ValueTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id(id)
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(theme::ACCENT)
            .hover(|style| style.bg(theme::HOVER))
            .child(n.to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_value(target.clone(), n, cx);
            }))
    }

    /// The number field and its commit button, drawn in place of whatever value the author
    /// clicked. One field for the whole crate, so there is never a second half-typed number
    /// lying around. A number that will not parse is simply refused, and the field keeping
    /// its content is what says so — the same idiom as a duplicate variable name.
    fn value_editor(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let target = self.editing.clone();
        div()
            .flex()
            .gap_1()
            .items_center()
            .child(div().w(px(52.0)).child(self.value_input.clone()))
            .child(
                div()
                    .id("value-commit")
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(theme::TEXT_MUTED)
                    .hover(|style| style.text_color(theme::TEXT))
                    .child("✓")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(target) = target.clone() else {
                            return;
                        };
                        let Ok(n) = this.value_input.read(cx).content().trim().parse::<i64>()
                        else {
                            return;
                        };
                        let value = Value::Int(n);
                        this.doc.checkpoint();
                        match &target {
                            ValueTarget::Default(name) => this.doc.set_default(name, value),
                            // The operator is not what is being edited here, so it is read
                            // back and written straight through.
                            ValueTarget::Effect(id, name) => {
                                let op = this.doc.effect_of(*id, name).map_or(SetOp::Set, |e| e.op);
                                this.doc.set_effect(*id, name, Some(value), op);
                            }
                            ValueTarget::Check(id, name) => {
                                let op = this.doc.condition_of(*id, name).map_or(Cmp::Eq, |c| c.op);
                                this.doc.set_condition(*id, name, Some(value), op);
                            }
                        }
                        this.editing = None;
                        cx.notify();
                    })),
            )
    }

    /// The set chips above the declare field: `(none)` plus one per set the document has.
    /// Chips rather than a dropdown because the crate has no dropdown and every other
    /// choice in it is a chip or a cycle — `kind_picker` is the same shape.
    ///
    /// There is deliberately no "new set" chip: a set is still made by typing the dot once,
    /// exactly as before. The picker is autocomplete for the sets that already exist, which
    /// is the whole of the typo problem, not a replacement for the convention.
    fn set_picker(
        &self,
        current: &str,
        sets: &[String],
        cx: &mut Context<Self>,
        // Owns everything it draws, like the rows it sits under.
    ) -> impl IntoElement + use<> {
        let chips: Vec<String> = std::iter::once(String::new())
            .chain(sets.iter().cloned())
            .collect();
        let current = current.to_string();

        // Wraps for `kind_picker`'s reason: a handful of set names do not fit across
        // `VARS_W`, and without this the last one is clipped off rather than wrapped.
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(chips.into_iter().enumerate().map(|(i, set)| {
                let on = set == current;
                let label = match set.is_empty() {
                    true => "(none)".to_string(),
                    false => set.clone(),
                };
                div()
                    .id(("set-chip", i))
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if on { theme::PRESSED } else { theme::RAISED })
                    .text_color(if on { theme::TEXT } else { theme::TEXT_MUTED })
                    .hover(|style| style.text_color(theme::TEXT))
                    .one_line()
                    .child(label)
                    // Only aims the field, so no checkpoint — same as a `\u{270e}`.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set.clone_from(&set);
                        cx.notify();
                    }))
            }))
    }

    /// The `\u{270e}` on a set header. Aims the shared field at the set name rather than at
    /// a variable's, which is the one job `Editor::pane` cannot say apart on its own.
    fn set_rename_button(&self, set: &str, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let set = set.to_string();
        div()
            .id(SharedString::from(format!("set-rename-{set}")))
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(theme::TEXT_FAINT)
            .hover(|style| style.text_color(theme::TEXT))
            .child("\u{270e}")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.renaming = Some(Rename::Set(set.clone()));
                this.var_input
                    .update(cx, |input, cx| input.set_content(&set, cx));
                cx.notify();
            }))
    }

    /// One row of the variables pane: the flag's state or the number it holds, its name, and
    /// the type, rename and remove buttons. `i` is the index into `doc.variables` and is only
    /// an element id and a lookup — every mutation below names the variable by its whole
    /// dotted name, because that, not the position, is what the document is keyed by.
    fn var_row(
        &self,
        i: usize,
        playing: Option<&VarState>,
        cx: &mut Context<Self>,
        // The row owns everything it draws, so it borrows nothing back — which is what lets the
        // grouped `.children(..)` loops below build one inside a closure.
    ) -> impl IntoElement + use<> {
        let variable = &self.doc.variables[i];
        let shown = playing
            .and_then(|vars| vars.get(&variable.name).copied())
            .unwrap_or(variable.default);
        let name = variable.name.clone();
        let to_type = variable.name.clone();
        let to_rename = variable.name.clone();
        let to_remove = variable.name.clone();
        // Under a group header the set is already overhead, so the row shows only the rest.
        let label = split_set(&variable.name).1.to_string();
        let is_int = matches!(variable.default, Value::Int(_));
        let editing = self
            .editing_target()
            .is_some_and(|target| *target == ValueTarget::Default(name.clone()));

        div()
            .flex()
            .gap_2()
            .items_center()
            .child(match shown {
                // A flag is its own control: the box shows the state and clicking it flips.
                Value::Bool(on) => div()
                    .id(("var-toggle", i))
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(if on { theme::TRUE } else { theme::TEXT })
                    .child(div().flex_none().child(if on { "☑" } else { "☐" }))
                    .child(div().flex_1().min_w(px(0.0)).one_line().child(label))
                    .when(playing.is_none(), |this| {
                        this.cursor_pointer()
                            .hover(|style| style.text_color(theme::TEXT))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.doc.checkpoint();
                                this.doc.set_default(&name, Value::Bool(!on));
                                cx.notify();
                            }))
                    })
                    .into_any_element(),
                // A number is typed rather than cycled, so the name and the value are two
                // controls instead of one.
                Value::Int(n) => div()
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .text_color(theme::TEXT)
                    .child(div().flex_1().min_w(px(0.0)).one_line().child(label))
                    .child(if editing {
                        self.value_editor(cx).into_any_element()
                    } else if playing.is_some() {
                        // Read-only while playing, like everything else in the pane.
                        div()
                            .px_1()
                            .text_color(theme::TEXT)
                            .child(n.to_string())
                            .into_any_element()
                    } else {
                        self.number_cell(("var-value", i), n, ValueTarget::Default(name), cx)
                            .into_any_element()
                    })
                    .into_any_element(),
            })
            .when(playing.is_none() && !editing, |this| {
                this.child(
                    // The one place a variable's type changes. Lit when it holds a number,
                    // since a row showing 0 says nothing about the type on its own.
                    div()
                        .id(("var-type", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(if is_int {
                            theme::ACCENT
                        } else {
                            theme::TEXT_FAINT
                        })
                        .hover(|style| style.text_color(theme::TEXT))
                        .child("#")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.checkpoint();
                            let to = if is_int {
                                Value::Bool(false)
                            } else {
                                Value::Int(0)
                            };
                            // Effects and checks are converted with it, so say how many.
                            let moved = this.doc.retype_variable(&to_type, to);
                            let kind = if is_int { "a flag" } else { "a number" };
                            let s = if moved == 1 { "" } else { "s" };
                            this.set_status(
                                format!("\"{to_type}\" is now {kind} — {moved} reference{s}."),
                                cx,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("var-rename", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::TEXT))
                        .child("✎")
                        // Not an edit yet, so no checkpoint — it only aims
                        // the field the declare button already uses.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Split the way the declare field is joined, so clicking a
                            // different chip mid-rename moves the variable between sets.
                            let (set, rest) = split_set(&to_rename);
                            this.set = set.unwrap_or_default().to_string();
                            this.renaming = Some(Rename::Item(to_rename.clone()));
                            this.var_input
                                .update(cx, |input, cx| input.set_content(rest, cx));
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("var-remove", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::DANGER))
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.checkpoint();
                            // Effects naming it go with it, so say how many.
                            let dropped = this.doc.remove_variable(&to_remove);
                            if dropped > 0 {
                                let s = if dropped == 1 { "" } else { "s" };
                                this.set_status(
                                    format!("Removed \"{to_remove}\" and {dropped} effect{s}."),
                                    cx,
                                );
                            }
                            cx.notify();
                        })),
                )
            })
    }

    /// A row of the Flows list: the canvas name, how many nodes stand on it, and the
    /// controls. Clicking the name switches the canvas; `→` moves the selection onto it,
    /// which is the affordance the whole feature exists for — it is how a graph that got
    /// too big gets split up. The main flow is `""`: it is always present, so it can be
    /// neither renamed nor removed.
    fn flow_row(
        &self,
        i: usize,
        flow: String,
        playing: bool,
        cx: &mut Context<Self>,
        // Owns everything it draws and borrows nothing back, like its two sibling rows.
    ) -> impl IntoElement + use<> {
        let on = self.flow == flow;
        let count = self.doc.nodes_in(&flow).count();
        let main = flow.is_empty();
        let selected = self.selection.nodes().len();
        let (to_show, to_rename, to_remove, to_move) =
            (flow.clone(), flow.clone(), flow.clone(), flow.clone());

        div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id(("flow-name", i))
                    .flex_1()
                    .min_w(px(0.0))
                    .one_line()
                    .cursor_pointer()
                    .text_color(if on { theme::TEXT } else { theme::TEXT_MUTED })
                    .hover(|style| style.text_color(theme::TEXT))
                    .child(if main { "(main)".to_string() } else { flow })
                    .on_click(cx.listener(move |this, _, _, cx| this.show_flow(&to_show, cx))),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::TEXT_FAINT)
                    .child(count.to_string()),
            )
            .when(!playing && selected > 0 && !on, |this| {
                this.child(
                    div()
                        .id(("flow-move", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::TEXT))
                        .child("→")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let ids = this.selection.nodes().to_vec();
                            let count = ids.len();
                            this.doc.checkpoint();
                            let cut = this.doc.move_to_flow(&ids, &to_move);
                            let s = if count == 1 { "" } else { "s" };
                            // The nodes just left this canvas, so nothing is selected on it.
                            this.select(Selection::None, cx);
                            // Said now, since neither canvas will ever draw the edges cut.
                            let status = match cut {
                                0 => format!("Moved {count} node{s}."),
                                1 => format!(
                                    "Moved {count} node{s} — 1 edge now crosses flows; \
                                     make its source a Jump."
                                ),
                                cut => format!(
                                    "Moved {count} node{s} — {cut} edges now cross flows; \
                                     make their sources Jumps."
                                ),
                            };
                            this.set_status(status, cx);
                            cx.notify();
                        })),
                )
            })
            .when(!playing && !main, |this| {
                this.child(
                    div()
                        .id(("flow-rename", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::TEXT))
                        .child("\u{270e}")
                        // Not an edit yet, so no checkpoint — it only aims the shared field.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.renaming = Some(Rename::Item(to_rename.clone()));
                            this.var_input
                                .update(cx, |input, cx| input.set_content(&to_rename, cx));
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("flow-remove", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::DANGER))
                        .child("\u{d7}")
                        // The nodes come back to the main flow rather than going with it:
                        // deleting a canvas must not delete what is drawn on it.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.checkpoint();
                            let moved = this.doc.remove_flow(&to_remove);
                            if this.flow == *to_remove {
                                this.flow.clear();
                            }
                            let s = if moved == 1 { "" } else { "s" };
                            this.set_status(
                                format!("Removed \"{to_remove}\" — {moved} node{s} moved."),
                                cx,
                            );
                            cx.notify();
                        })),
                )
            })
    }

    /// One row of the cast list: the character's colour, their name, and the rename and
    /// remove buttons. The sibling of `var_row`, and `i` is an index into `doc.characters`
    /// for the same two purposes — an element id and a lookup, with every mutation naming
    /// the character, because the name is what the document is keyed by.
    fn cast_row(
        &self,
        i: usize,
        playing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let character = &self.doc.characters[i];
        let color = character.color;
        let name = character.name.clone();
        let to_recolor = name.clone();
        let to_rename = name.clone();
        let to_remove = name.clone();

        div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                // The swatch is the colour control: clicking cycles the palette, since the
                // crate has no picker widget and every other choice in it cycles too.
                div()
                    .id(("cast-color", i))
                    .flex_none()
                    .size_3()
                    .rounded_sm()
                    .bg(rgb(color))
                    .when(!playing, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let at = CAST_COLORS.iter().position(|c| *c == color);
                                let next =
                                    CAST_COLORS[at.map_or(0, |at| (at + 1) % CAST_COLORS.len())];
                                this.doc.checkpoint();
                                this.doc.set_character_color(&to_recolor, next);
                                cx.notify();
                            }))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .one_line()
                    .text_color(theme::TEXT)
                    .child(name),
            )
            .when(!playing, |this| {
                this.child(
                    div()
                        .id(("cast-rename", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::TEXT))
                        .child("✎")
                        // Not an edit yet, so no checkpoint — it only aims the shared field.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.renaming = Some(Rename::Item(to_rename.clone()));
                            this.var_input
                                .update(cx, |input, cx| input.set_content(&to_rename, cx));
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("cast-remove", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_FAINT)
                        .hover(|style| style.text_color(theme::DANGER))
                        .child("×")
                        // Undeclaring only: the nodes keep saying the name, and the problem
                        // list is what points out that nobody declared it.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.checkpoint();
                            this.doc.remove_character(&to_remove);
                            cx.notify();
                        })),
                )
            })
    }

    /// What the instructions between the previous node the player saw and the current one
    /// wrote, in the order they ran.
    fn effects_fired(&self, path: &[NodeId]) -> Vec<(String, SetOp, Value)> {
        let Some(before) = path.len().checked_sub(1) else {
            return Vec::new();
        };
        let mut run: Vec<NodeId> = path[..before]
            .iter()
            .rev()
            .take_while(|&&id| !self.doc.pauses_play(id))
            .copied()
            .collect();
        run.reverse();
        run.iter()
            .filter_map(|&id| self.doc.node(id))
            .filter(|node| node.kind == NodeKind::Instruction)
            .flat_map(|node| node.effects.iter())
            .map(|effect| (effect.var.clone(), effect.op, effect.value))
            .collect()
    }

    /// The conversation as a reader meets it: what the current node says, and the choices
    /// leading out of it. Replaces the inspector while it is up, so the sidebar is never
    /// both an editor and a player.
    fn play_panel(&self, path: &[NodeId], cx: &mut Context<Self>) -> impl IntoElement {
        let row = |id: (&'static str, usize), label: String, color: Rgba| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_color(color)
                .hover(|style| style.bg(theme::HOVER))
                .child(label)
        };

        let current = *path.last().expect("a play-through is never empty");
        let node = self.doc.node(current);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .child(div().type_title().child("Playing"))
                    .child(
                        row(("play-stop", 0), "Stop".into(), theme::TEXT_MUTED).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.on_stop_play(&StopPlay, window, cx)
                            }),
                        ),
                    ),
            )
            .map(|this| {
                // The node can be deleted from the canvas mid-play; say so rather than
                // silently showing nothing.
                let Some(node) = node else {
                    return this.child(
                        div()
                            .text_color(theme::DANGER)
                            .child("This node has been deleted."),
                    );
                };

                // The walk only stops on a node the player sees, unless it ran out of graph
                // — a condition with that pin unwired, or a jump pointing at nothing.
                if !self.doc.pauses_play(current) {
                    return this.child(
                        div()
                            .text_color(theme::DANGER)
                            .child("The flow stops here — nothing leads on."),
                    );
                }

                // Through the kind, not the raw fields: a hub that used to be a line still
                // carries the old speaker, and nobody speaks at a hub.
                let speaker = node.player_speaker().unwrap_or("").trim();
                let text = node.player_text().unwrap_or("").trim();
                let is_hub = node.kind == NodeKind::Hub;
                this.when(!speaker.is_empty(), |this| {
                    this.child(
                        div()
                            .text_color(speaker_color(&self.doc, speaker, theme::SPEAKER_DEFAULT))
                            .child(speaker.to_string()),
                    )
                })
                // A blank hub is complete — `Problem::Empty` exempts it — so it shows its
                // choices with nothing above them rather than an "unwritten" placeholder.
                .when(!(is_hub && text.is_empty()), |this| {
                    this.child(if text.is_empty() {
                        div().type_hint().child("(no line written yet)")
                    } else {
                        div().child(text.to_string())
                    })
                })
            })
            // What the instructions walked through on the way here did. Without this the
            // only evidence is the variables panel changing in the corner of your eye.
            .children(
                self.effects_fired(path)
                    .into_iter()
                    .map(|(var, op, value)| {
                        div().text_color(theme::KIND_INSTRUCTION).child(format!(
                            "{var} {} {}",
                            set_text(op),
                            value_text(value)
                        ))
                    }),
            )
            .children(
                self.doc
                    .choices(current)
                    .into_iter()
                    .enumerate()
                    .map(|(i, (to, label))| {
                        // A blank label is an unwritten choice, not a missing one.
                        let label = match label.trim() {
                            "" => "Continue".to_string(),
                            label => label.to_string(),
                        };
                        // A spent once-only choice is greyed out, not dropped: `choices` is
                        // never filtered, the canvas still draws the edge, and this is the
                        // same place the player already diverges from the canvas by walking
                        // through nodes it does not show. No hover and no click, so nothing
                        // about the row offers something it will not do.
                        if self.doc.spent(path, current, to) {
                            return div()
                                .px_1()
                                .text_color(theme::TEXT_DISABLED)
                                .child(format!("→ {label} (already taken)"))
                                .into_any_element();
                        }
                        row(("play-choice", i), format!("→ {label}"), theme::TEXT)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.play_advance(to, window, cx)
                            }))
                            .into_any_element()
                    }),
            )
            // `all` over an empty list is true, so a leaf node still ends here — and a hub
            // whose questions are all used up ends rather than stranding the player on a
            // list of rows that no longer do anything.
            .when(
                self.doc
                    .choices(current)
                    .iter()
                    .all(|&(to, _)| self.doc.spent(path, current, to)),
                |this| this.child(div().type_hint().child("End of conversation.")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    // Offered only when there is an earlier node the player actually saw:
                    // the invisible run walked through to get here is not a step back.
                    .when(self.doc.step_back(&mut path.to_vec()), |this| {
                        this.child(
                            row(("play-nav", 0), "← Back".into(), theme::TEXT_MUTED).on_click(
                                cx.listener(|this, _, window, cx| {
                                    let Some(mut path) = this.play.take() else {
                                        return;
                                    };
                                    this.doc.step_back(&mut path);
                                    this.follow(&path, window);
                                    this.play = Some(path);
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .child(
                        row(("play-nav", 1), "Restart".into(), theme::TEXT_MUTED).on_click(
                            cx.listener(|this, _, window, cx| {
                                let Some(path) = this.play.as_ref() else {
                                    return;
                                };
                                // Re-walked rather than truncated, so a conversation that
                                // opens on an instruction runs it again.
                                let seed = path[0];
                                this.start_play(seed, window, cx);
                            }),
                        ),
                    ),
            )
    }

    /// The strip under the three panes: what is true of the *document and the view* rather
    /// than of the selection — problems, which canvas, the zoom, what the last action did, and
    /// whether there is unsaved work. All of it used to be stacked in the inspector at the
    /// same weight as the fields it sat between, and changed the inspector's height as it
    /// came and went.
    fn status_bar(&self, problems: &[Problem], cx: &mut Context<Self>) -> impl IntoElement {
        let errors = problems
            .iter()
            .filter(|p| p.severity() == Severity::Error)
            .count();
        let warnings = problems.len() - errors;
        let flow = if self.flow.is_empty() {
            "main".to_string()
        } else {
            self.flow.clone()
        };
        let cell = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h_full()
                .px_2()
                .type_hint()
        };

        div()
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(STATUS_H))
            .bg(theme::PANE)
            .border_t_1()
            .border_color(theme::DIVIDER)
            .child(
                cell("problems-badge")
                    .cursor_pointer()
                    .when(self.problems_open, |this| this.bg(theme::PRESSED))
                    .hover(|style| style.bg(theme::HOVER))
                    .map(|this| match (errors, warnings) {
                        (0, 0) => this
                            .child(div().text_color(theme::SUCCESS).child("✓"))
                            .child("No problems"),
                        (errors, warnings) => this
                            .when(errors > 0, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .text_color(theme::DANGER)
                                        .child("✕")
                                        .child(errors.to_string()),
                                )
                            })
                            .when(warnings > 0, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .text_color(theme::WARNING)
                                        .child("▲")
                                        .child(warnings.to_string()),
                                )
                            }),
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_toggle_problems(&ToggleProblems, window, cx)
                    })),
            )
            .child(
                cell("flow-cell")
                    .child("Flow")
                    .child(div().text_color(theme::TEXT_MUTED).child(flow)),
            )
            // A click resets to 100%, which is the thing you want a zoom readout for.
            .child(
                cell("zoom-cell")
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::HOVER).text_color(theme::TEXT))
                    .child(format!("{}%", (self.view.zoom * 100.0).round()))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_zoom_reset(&ZoomReset, window, cx)
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px_2()
                    .one_line()
                    .text_size(px(theme::SMALL))
                    .text_color(theme::TEXT_MUTED)
                    .children(self.status.clone()),
            )
            .child(
                cell("file-cell")
                    .child(self.file_name())
                    .when(self.doc.is_dirty(), |this| {
                        this.child(div().text_color(theme::WARNING).child("●"))
                    }),
            )
            .child(
                cell("help-button")
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::HOVER).text_color(theme::TEXT))
                    .when(self.help_open, |this| this.bg(theme::PRESSED))
                    .child("?")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.on_show_help(&ShowHelp, window, cx)),
                    ),
            )
            .when(self.problems_open, |this| {
                this.child(self.problems_drawer(problems, cx))
            })
    }

    /// Everything wrong with the document, worst first, in a card that opens upward from the
    /// status bar's badge. Clicking a row jumps to the node — or selects the edge, for the
    /// problems a click on the canvas cannot reach. It stays open while you do, so a list can
    /// be worked down; Escape or the badge closes it.
    fn problems_drawer(&self, problems: &[Problem], cx: &mut Context<Self>) -> impl IntoElement {
        /// Scrolls, so this only bounds the work done per frame.
        const MAX_ROWS: usize = 100;
        let more = problems.len().saturating_sub(MAX_ROWS);

        deferred(
            div()
                .absolute()
                .bottom(px(STATUS_H))
                .left(px(8.0))
                .w(px(480.0))
                .occlude()
                .flex()
                .flex_col()
                .bg(theme::RAISED)
                .border_1()
                .border_color(theme::OUTLINE)
                .rounded_md()
                .shadow_lg()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::DIVIDER)
                        .child(div().type_label().child("Problems"))
                        .child(div().type_hint().child(format!("{}", problems.len()))),
                )
                .child(
                    div()
                        .id("problems-list")
                        .flex()
                        .flex_col()
                        .p_1()
                        .max_h(px(280.0))
                        .overflow_y_scroll()
                        .when(problems.is_empty(), |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .flex()
                                    .gap_2()
                                    .type_hint()
                                    .child(div().text_color(theme::SUCCESS).child("✓"))
                                    .child("Nothing to fix."),
                            )
                        })
                        .children(
                            problems
                                .iter()
                                .take(MAX_ROWS)
                                .enumerate()
                                .map(|(i, problem)| {
                                    // Severity lives on the glyph, not the sentence: a whole row in
                                    // amber made three routine dead ends the loudest thing on
                                    // screen. An error's text stays a tier brighter, since it is
                                    // the one that means the document is broken.
                                    let (glyph, color, text) = match problem.severity() {
                                        Severity::Error => ("✕", theme::DANGER, theme::TEXT),
                                        Severity::Warning => {
                                            ("▲", theme::WARNING, theme::TEXT_MUTED)
                                        }
                                    };
                                    let (node, edge) = (problem.node(), problem.edge());
                                    div()
                                        .id(("problem", i))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .child(
                                            div()
                                                .flex_none()
                                                .w_4()
                                                .text_size(px(theme::SMALL))
                                                .text_color(color)
                                                .child(glyph),
                                        )
                                        // `min_w(0)` + the clamp is what makes the ellipsis appear
                                        // instead of the row being clipped mid-word.
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .one_line()
                                                .text_color(text)
                                                .child(problem_text(problem, &self.doc)),
                                        )
                                        .when(node.is_some() || edge.is_some(), |this| {
                                            this.cursor_pointer()
                                                .hover(|style| style.bg(theme::HOVER))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        // An edge problem selects its edge, since a
                                                        // click on the canvas cannot: a duplicate lies
                                                        // under its original and a cross-flow edge is
                                                        // drawn nowhere. Selected, it can be deleted
                                                        // like any other.
                                                        if let Some(index) = edge {
                                                            this.select_edge(index, window, cx);
                                                        } else if let Some(id) = node {
                                                            this.jump_to(id, window, cx);
                                                        }
                                                    },
                                                ))
                                        })
                                }),
                        )
                        .when(more > 0, |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .type_hint()
                                    .child(format!("…and {more} more")),
                            )
                        }),
                ),
        )
    }

    /// The command palette: every menu command, found by typing instead of by remembering
    /// which menu it hangs off. Opened by Ctrl+K, Enter runs the highlighted row, Escape or
    /// a click outside closes it.
    ///
    /// Modal, with the help sheet's pattern — an invisible backdrop that occludes the window
    /// and closes the palette on a click, and a card that stops its own clicks reaching it —
    /// where find is not, because find is meant to be walked through with the canvas live and
    /// a palette is one command and out. No dimming: the document stays legible behind it.
    fn palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        const CARD_W: f32 = 480.0;
        let query = self.palette_input.read(cx).content().to_string();
        let entries = palette_entries(&query);
        let selected = self.palette_sel.min(entries.len().saturating_sub(1));

        deferred(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .occlude()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(MENU_H + 48.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.dismiss_overlays(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .key_context("Palette")
                        .occlude()
                        .w(px(CARD_W))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_2()
                        .bg(theme::RAISED)
                        .border_1()
                        .border_color(theme::OUTLINE)
                        .rounded_md()
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .child(self.palette_input.clone())
                        .child(
                            div()
                                .id("palette-list")
                                .flex()
                                .flex_col()
                                .max_h(px(320.0))
                                .overflow_y_scroll()
                                .track_scroll(&self.palette_scroll)
                                .when(entries.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .type_hint()
                                            .child("No matching command."),
                                    )
                                })
                                .children(entries.iter().enumerate().map(
                                    |(row, &(menu, &(label, keys, action)))| {
                                        div()
                                            .id(("palette-row", row))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .px_2()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            // One rung up from the card (see `theme`). There is
                                            // no separate hover style: pointing at a row
                                            // *selects* it, so the list has one highlight.
                                            // A hover colour as well showed two rows lit
                                            // whenever the pointer rested over the list
                                            // while the arrow keys moved the selection.
                                            .when(row == selected, |this| this.bg(theme::PRESSED))
                                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                                if this.palette_sel != row {
                                                    this.palette_sel = row;
                                                    cx.notify();
                                                }
                                            }))
                                            // The menu it lives in, in a column of its own
                                            // so the labels line up and can be scanned.
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .w(px(40.0))
                                                    .type_hint()
                                                    .child(menu),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .one_line()
                                                    .text_color(theme::TEXT)
                                                    .child(label.trim_end_matches('…')),
                                            )
                                            .when(!keys.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .flex_none()
                                                        .px_1()
                                                        .rounded_sm()
                                                        .bg(theme::CANVAS)
                                                        .type_hint()
                                                        .text_color(theme::TEXT)
                                                        .child(keys),
                                                )
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.run_command(action, window, cx)
                                            }))
                                    },
                                )),
                        ),
                ),
        )
    }

    /// Every command and gesture, over a dimmed window. The shortcuts are read straight off
    /// `MENUS`, so the sheet is the menus' own table and cannot fall out of step with them;
    /// only the mouse gestures, which no menu holds, are written out here.
    fn help_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The mouse map, as `on_mouse_down` has it. Alt+left and middle both pan.
        const MOUSE: &[(&str, &str)] = &[
            ("Connect two nodes", "Drag from a port"),
            ("New node", "Double-click the canvas"),
            ("Select several", "Drag on empty canvas"),
            ("Add to selection", "Shift-click"),
            ("Pan", "Middle-drag or Alt-drag"),
            ("Zoom", "Scroll"),
            ("Back to the canvas", "Esc"),
        ];
        let group = |title: &str, rows: Vec<(String, String)>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .w(px(250.0))
                .child(div().type_section().pb_1().child(title.to_uppercase()))
                .children(rows.into_iter().map(|(what, keys)| {
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .min_w(px(0.0))
                                .one_line()
                                .text_color(theme::TEXT_MUTED)
                                .child(what),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_1()
                                .rounded_sm()
                                .bg(theme::CANVAS)
                                .type_hint()
                                .text_color(theme::TEXT)
                                .child(keys),
                        )
                }))
        };

        deferred(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::SCRIM)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.help_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("help-card")
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .max_h(px(640.0))
                        .overflow_y_scroll()
                        .p_5()
                        .bg(theme::RAISED)
                        .border_1()
                        .border_color(theme::OUTLINE)
                        .rounded_lg()
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_between()
                                .items_center()
                                .child(div().type_title().child("Keyboard & mouse"))
                                .child(div().type_hint().child("Esc to close")),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap_x_6()
                                .gap_y_4()
                                .w(px(540.0))
                                .children(MENUS.iter().map(|(name, items)| {
                                    group(
                                        name,
                                        items
                                            .iter()
                                            .map(|(label, keys, _)| {
                                                (label.to_string(), keys.to_string())
                                            })
                                            .collect(),
                                    )
                                }))
                                .child(group(
                                    "Mouse",
                                    MOUSE
                                        .iter()
                                        .map(|(what, how)| (what.to_string(), how.to_string()))
                                        .collect(),
                                )),
                        ),
                ),
        )
    }

    /// What the node is. What a conversion keeps and drops is `Doc::set_kind`'s to decide,
    /// where it can be tested; this only checkpoints first, so an undo brings back every
    /// edge and effect the change cut.
    fn kind_picker(&self, id: NodeId, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.doc.node(id).map_or(NodeKind::Speech, |node| node.kind);

        // A grid, not a wrapping row: six labels do not fit across `SIDEBAR_W`, and wrapping
        // them left a ragged second row with one chip alone on it. Three even columns hold
        // the longest label ("Instruction") with room to spare.
        div().grid().grid_cols(3).gap_1().children(
            [
                (NodeKind::Speech, "kind-speech"),
                (NodeKind::Hub, "kind-hub"),
                (NodeKind::Jump, "kind-jump"),
                (NodeKind::Instruction, "kind-instruction"),
                (NodeKind::Condition, "kind-condition"),
                (NodeKind::Note, "kind-note"),
            ]
            .map(|(kind, element_id)| {
                let on = kind == current;
                let (glyph, label, color) = kind_style(kind);
                div()
                    .id(element_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(theme::SMALL))
                    .bg(if on { theme::PRESSED } else { theme::RAISED })
                    .text_color(if on { theme::TEXT } else { theme::TEXT_MUTED })
                    .hover(|style| style.text_color(theme::TEXT))
                    // The glyph keeps its kind's colour whether or not the chip is on, so the
                    // picker doubles as the key to what the canvas draws.
                    .child(div().text_color(color).child(glyph))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.doc.node(id).is_none_or(|node| node.kind == kind) {
                            return;
                        }
                        this.doc.checkpoint();
                        this.doc.set_kind(id, kind);
                        // Changing kind in place does not reload the fields.
                        this.text_input
                            .update(cx, |input, cx| input.set_placeholder(text_prompt(kind), cx));
                        cx.notify();
                    }))
            }),
        )
    }

    /// The content of an instruction or a condition node: what it writes, or what it
    /// checks. One row per declared variable, cycling nothing → true → false → nothing, so
    /// there is no add button, no remove button, and no way to name the same variable
    /// twice. Both kinds share this so the two editors cannot drift apart — the model keeps
    /// `effects` and `conditions` apart because the *names* would lie, not the widgets.
    fn var_rows(&self, id: NodeId, mode: VarRow, cx: &mut Context<Self>) -> impl IntoElement {
        // Scrolls rather than capping like the sibling panels: a cap here was not a shorter
        // list but an unreachable variable — the ninth one declared could be neither set
        // nor checked from anywhere. Bounded so the rest of the inspector stays on screen.
        const ROWS_H: f32 = 240.0;

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().type_label().child(match mode {
                VarRow::Sets => "Sets",
                VarRow::Checks => "Checks",
            }))
            .when(self.doc.variables.is_empty(), |this| {
                this.child(
                    div()
                        .type_hint()
                        .child("Declare one in the Variables pane first."),
                )
            })
            .child(
                div()
                    .id("var-rows")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(ROWS_H))
                    .overflow_y_scroll()
                    .children(self.doc.variables.iter().enumerate().map(|(i, variable)| {
                        let name = variable.name.clone();
                        let int = matches!(variable.default, Value::Int(_));
                        // What the node holds now: the value, and the operator glyph that
                        // goes in front of it.
                        let current: Option<(Value, &'static str)> = match mode {
                            VarRow::Sets => self
                                .doc
                                .effect_of(id, &variable.name)
                                .map(|e| (e.value, set_text(e.op))),
                            VarRow::Checks => self
                                .doc
                                .condition_of(id, &variable.name)
                                .map(|c| (c.want, cmp_text(c.op))),
                        };
                        // A flag is a checkbox; a number has no box to tick, so it shows the
                        // operator there instead and the value beside the name.
                        let (glyph, color) = match current {
                            None => ("—", theme::TEXT_FAINT),
                            Some((Value::Bool(true), _)) => ("☑", theme::TRUE),
                            Some((Value::Bool(false), _)) => ("☐", theme::FALSE),
                            Some((Value::Int(_), op)) => (op, theme::ACCENT),
                        };
                        let number = match current {
                            Some((Value::Int(n), _)) => Some(n),
                            _ => None,
                        };
                        let target = match mode {
                            VarRow::Sets => ValueTarget::Effect(id, name.clone()),
                            VarRow::Checks => ValueTarget::Check(id, name.clone()),
                        };
                        let editing = self.editing_target().is_some_and(|aimed| *aimed == target);

                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .id(("var-row", i))
                                    .flex()
                                    .gap_2()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .one_line()
                                    .cursor_pointer()
                                    .text_color(color)
                                    .hover(|style| style.bg(theme::HOVER))
                                    .child(glyph)
                                    .child(variable.name.clone())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.doc.checkpoint();
                                        // One gesture per side, both ending back at cleared:
                                        // a flag cycles its two states, a number cycles its
                                        // operators and keeps whatever value it already had.
                                        match mode {
                                            VarRow::Sets => {
                                                let now = this
                                                    .doc
                                                    .effect_of(id, &name)
                                                    .map(|e| (e.value, e.op));
                                                let next = match (int, now) {
                                                    (false, None) => {
                                                        Some((Value::Bool(true), SetOp::Set))
                                                    }
                                                    (false, Some((Value::Bool(true), _))) => {
                                                        Some((Value::Bool(false), SetOp::Set))
                                                    }
                                                    (false, _) => None,
                                                    (true, None) => {
                                                        Some((Value::Int(0), SetOp::Set))
                                                    }
                                                    (true, Some((value, SetOp::Set))) => {
                                                        Some((value, SetOp::Add))
                                                    }
                                                    (true, Some(_)) => None,
                                                };
                                                this.doc.set_effect(
                                                    id,
                                                    &name,
                                                    next.map(|(value, _)| value),
                                                    next.map_or(SetOp::Set, |(_, op)| op),
                                                );
                                            }
                                            VarRow::Checks => {
                                                let now = this
                                                    .doc
                                                    .condition_of(id, &name)
                                                    .map(|c| (c.want, c.op));
                                                let next = match (int, now) {
                                                    (false, None) => {
                                                        Some((Value::Bool(true), Cmp::Eq))
                                                    }
                                                    (false, Some((Value::Bool(true), _))) => {
                                                        Some((Value::Bool(false), Cmp::Eq))
                                                    }
                                                    (false, _) => None,
                                                    (true, None) => Some((Value::Int(0), Cmp::Eq)),
                                                    (true, Some((want, op))) => {
                                                        next_cmp(op).map(|op| (want, op))
                                                    }
                                                };
                                                this.doc.set_condition(
                                                    id,
                                                    &name,
                                                    next.map(|(want, _)| want),
                                                    next.map_or(Cmp::Eq, |(_, op)| op),
                                                );
                                            }
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(editing, |this| this.child(self.value_editor(cx)))
                            .when_some(number.filter(|_| !editing), |this, n| {
                                this.child(self.number_cell(("var-num", i), n, target, cx))
                            })
                    })),
            )
    }

    /// Flips which pin of a condition an edge leaves by. A discrete edit, so it checkpoints
    /// outright rather than coalescing, like `end_toggle`.
    fn branch_toggle(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.doc.edges.get(index).is_some_and(|edge| edge.branch);
        div()
            .id("branch-toggle")
            .flex()
            .gap_2()
            .cursor_pointer()
            .text_color(if on { theme::TRUE } else { theme::FALSE })
            .hover(|style| style.text_color(theme::TEXT))
            .child(if on {
                "☑ true branch"
            } else {
                "☐ false branch"
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.doc.edges.get(index).is_none() {
                    return;
                }
                this.doc.checkpoint();
                this.doc.edges[index].branch = !this.doc.edges[index].branch;
                cx.notify();
            }))
    }

    /// Marks a choice as once-only: offered until the player takes it, then greyed out for
    /// the rest of the play-through. The sibling of `branch_toggle` — a discrete edit, so
    /// it checkpoints outright rather than coalescing.
    fn once_toggle(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.doc.edges.get(index).is_some_and(|edge| edge.once);
        div()
            .id("once-toggle")
            .flex()
            .gap_2()
            .cursor_pointer()
            .text_color(if on { theme::TEXT } else { theme::TEXT_MUTED })
            .hover(|style| style.text_color(theme::TEXT))
            .child(if on { "☑ once only" } else { "☐ once only" })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.doc.edges.get(index).is_none() {
                    return;
                }
                this.doc.checkpoint();
                this.doc.edges[index].once = !this.doc.edges[index].once;
                cx.notify();
            }))
    }

    /// Marks a node as a deliberate ending, which is what keeps it out of the dead-end
    /// warnings. A discrete edit, so it checkpoints outright rather than coalescing.
    fn end_toggle(&self, id: NodeId, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.doc.node(id).is_some_and(|node| node.end);
        div()
            .id("end-toggle")
            .flex()
            .gap_2()
            .cursor_pointer()
            .text_color(if on { theme::ENDING } else { theme::TEXT_MUTED })
            .hover(|style| style.text_color(theme::TEXT))
            .child(if on { "☑" } else { "☐" })
            .child("Ending")
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.doc.node(id).is_none() {
                    return;
                }
                this.doc.checkpoint();
                if let Some(node) = this.doc.node_mut(id) {
                    node.end = !node.end;
                }
                cx.notify();
            }))
    }

    /// Inspector for the selected node or edge.
    /// The inspector: scoped to the selection, and nothing else. Find, the problem list, the
    /// key hints and the last status message used to be stacked in here at the same weight as
    /// the fields; each now has the surface its scope belongs to (the find overlay, the
    /// problems drawer, the cheat sheet, the status bar). What is left scrolls, so a tall
    /// Sets list never pushes a field off the bottom of a short window.
    fn sidebar(&self, problems: &[Problem], cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .bg(theme::PANE)
            .border_l_1()
            .border_color(theme::DIVIDER)
            .text_color(theme::TEXT)
            .child(
                div()
                    .id("inspector")
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p_3()
                    .child(self.inspector(problems, cx)),
            )
    }

    /// What the sidebar shows for the current state: the player while playing, otherwise
    /// the selection's own fields, or a summary of the document when nothing is selected.
    fn inspector(&self, problems: &[Problem], cx: &mut Context<Self>) -> AnyElement {
        let field = |label: &str, input: &Entity<TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().type_label().child(label.to_string()))
                .child(input.clone())
        };

        if let Some(path) = self.play.clone() {
            return self.play_panel(&path, cx).into_any_element();
        }

        match (self.selection.only_node(), &self.selection) {
            (Some(id), _) => {
                // Each kind shows the fields it actually has: a jump has none of its own,
                // and a hub has no speaker because nobody speaks at one.
                let kind = self.doc.node(id).map_or(NodeKind::Speech, |node| node.kind);
                let (glyph, name, color) = kind_style(kind);
                let text_label = match kind {
                    NodeKind::Hub => "Label",
                    NodeKind::Note => "Note",
                    _ => "Text",
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(inspector_header(
                        glyph,
                        color,
                        name,
                        node_number(&self.doc, id),
                    ))
                    .child(section("Type", self.kind_picker(id, cx)))
                    .when(kind == NodeKind::Speech, |this| {
                        this.child(section(
                            "Content",
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(field("Speaker", &self.speaker_input))
                                .child(field("Text", &self.text_input)),
                        ))
                    })
                    .when(matches!(kind, NodeKind::Hub | NodeKind::Note), |this| {
                        this.child(section("Content", field(text_label, &self.text_input)))
                    })
                    .when(kind == NodeKind::Instruction, |this| {
                        this.child(section("Logic", self.var_rows(id, VarRow::Sets, cx)))
                    })
                    .when(kind == NodeKind::Condition, |this| {
                        this.child(section("Logic", self.var_rows(id, VarRow::Checks, cx)))
                    })
                    // A jump has no fields of its own, which read as a broken inspector.
                    .when(kind == NodeKind::Jump, |this| {
                        this.child(section(
                            "Target",
                            div().type_hint().child(
                                "Drag from this node's port to the node it should jump to. \
                                 The box names it.",
                            ),
                        ))
                    })
                    // Outside every kind guard on purpose: an alias is orthogonal to
                    // `NodeKind` the way `end` is, and a game may want to enter a
                    // conversation at a hub or a condition too.
                    .child(section(
                        "Identity",
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(field("Alias", &self.alias_input))
                            .when(kind == NodeKind::Speech, |this| {
                                this.child(self.end_toggle(id, cx))
                            }),
                    ))
                    .into_any_element()
            }
            (None, Selection::Nodes(ids)) => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(inspector_header(
                    "#",
                    theme::ACCENT,
                    "Selection",
                    format!("{} nodes", ids.len()),
                ))
                .child(
                    div()
                        .type_hint()
                        .child("Drag any of them to move the group."),
                )
                .into_any_element(),
            (None, Selection::Edge(index)) => {
                let index = *index;
                let edge = self.doc.edges.get(index);
                let ends = match edge {
                    Some(edge) => format!(
                        "{} → {}",
                        node_number(&self.doc, edge.from),
                        node_number(&self.doc, edge.to)
                    ),
                    None => "a missing edge".into(),
                };
                // Out of a condition this edge is a pin, not a choice: the player never picks
                // it, so a label would be text nobody ever reads.
                let from_condition = edge.is_some_and(|edge| self.doc.is_condition(edge.from));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(inspector_header(
                        "→",
                        theme::EDGE,
                        if from_condition { "Branch" } else { "Choice" },
                        ends,
                    ))
                    .child(if from_condition {
                        section("Branch", self.branch_toggle(index, cx)).into_any_element()
                    } else {
                        section(
                            "Text",
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(self.label_input.clone())
                                // Only where the player is the one choosing. Out of an
                                // instruction or a jump the walk has to take the edge it
                                // finds, so a once-only flag there would be a promise
                                // nothing keeps.
                                .when(
                                    edge.is_some_and(|edge| self.doc.pauses_play(edge.from)),
                                    |this| this.child(self.once_toggle(index, cx)),
                                ),
                        )
                        .into_any_element()
                    })
                    .into_any_element()
            }
            _ => {
                // What the document is, while nothing in it is selected: the inspector's
                // empty state, which used to be one line telling you to select something.
                let stat = |label: &str, value: String, color: Rgba| {
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(div().text_color(theme::TEXT_MUTED).child(label.to_string()))
                        .child(div().text_color(color).child(value))
                };
                let errors = problems
                    .iter()
                    .filter(|p| p.severity() == Severity::Error)
                    .count();
                let (problem_text, problem_color) = match (problems.len(), errors) {
                    (0, _) => ("None".to_string(), theme::SUCCESS),
                    (n, 0) => (n.to_string(), theme::WARNING),
                    (n, _) => (n.to_string(), theme::DANGER),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(inspector_header(
                        "≡",
                        theme::TEXT_MUTED,
                        "Document",
                        self.file_name(),
                    ))
                    .child(section(
                        "Overview",
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(stat("Nodes", self.doc.nodes.len().to_string(), theme::TEXT))
                            .child(stat(
                                "Conversations",
                                self.doc.roots().len().to_string(),
                                theme::TEXT,
                            ))
                            .child(stat(
                                "Variables",
                                self.doc.variables.len().to_string(),
                                theme::TEXT,
                            ))
                            .child(stat(
                                "Cast",
                                self.doc.characters.len().to_string(),
                                theme::TEXT,
                            ))
                            // The main flow is always there and never listed.
                            .child(stat(
                                "Flows",
                                (self.doc.all_flows().len() + 1).to_string(),
                                theme::TEXT,
                            ))
                            .child(stat("Problems", problem_text, problem_color)),
                    ))
                    .child(section(
                        "Tips",
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .type_hint()
                            .child(if self.doc.nodes.is_empty() {
                                "Double-click the canvas to add the first node."
                            } else {
                                "Click a node or an edge to edit it."
                            })
                            .child("Ctrl+F finds a node.")
                            .child("F1 lists every shortcut."),
                    ))
                    .into_any_element()
            }
        }
    }
}

/// A named group in the inspector. The rule and the small capitals are what separate one
/// group of fields from the next: with neither, Alias, Speaker and Text were three lines of
/// the same weight, and the rarely-touched one sat first.
fn section(title: &str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pt_4()
        .border_t_1()
        .border_color(theme::DIVIDER)
        .child(div().type_section().child(title.to_uppercase()))
        .child(body)
}

/// What the inspector is about: a glyph chip in the thing's colour, its name, and where it
/// is. The one heavy, large element in the pane, so the eye lands here first.
fn inspector_header(
    glyph: &'static str,
    color: Rgba,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size_8()
                .rounded_md()
                .bg(theme::RAISED)
                .text_size(px(theme::TITLE))
                .text_color(color)
                .child(glyph),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().type_title().one_line().child(title.into()))
                .child(div().type_hint().one_line().child(subtitle.into())),
        )
}

/// Deferred so it paints above the canvas instead of under it.
fn menu_dropdown(items: &'static [MenuItem], cx: &mut Context<Editor>) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .top(px(MENU_H - 4.))
            .left(px(0.))
            .w(px(220.))
            .py_1()
            .bg(theme::RAISED)
            .border_1()
            .border_color(theme::OUTLINE)
            .rounded_md()
            .shadow_md()
            // The panel overlaps the canvas, whose mouse-down handler would otherwise
            // close the menu before the click on an item completes.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .children(items.iter().map(|(label, shortcut, run)| {
                let run = *run;
                div()
                    .id(*label)
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap_4()
                    .px_2()
                    .py_1()
                    .hover(|style| style.bg(theme::PRESSED))
                    .child(*label)
                    .child(div().type_hint().child(*shortcut))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_menu = None;
                        run(this, window, cx);
                        cx.notify();
                    }))
            })),
    )
}

/// What to call a node when it has nothing to say for itself. Its *position*, not its id:
/// ids are random `u64` and would print as nineteen digits. A position shifts when a node
/// ahead of it is deleted, which is fine for a label — the alias is the handle that does
/// not move, and it is deliberately empty on almost every node.
fn node_number(doc: &Doc, id: NodeId) -> String {
    match doc.nodes.iter().position(|n| n.id == id) {
        Some(index) => format!("Node {}", index + 1),
        None => "a missing node".into(),
    }
}

/// How a node reads in a list: what it says, or who says it, or its position as a last resort.
fn node_label(doc: &Doc, node: &Node) -> String {
    match (node.speaker.trim(), node.text.trim()) {
        ("", "") => node_number(doc, node.id),
        ("", text) => text.to_string(),
        (speaker, "") => speaker.to_string(),
        (speaker, text) => format!("{speaker} — {text}"),
    }
}

/// Wording for a problem row. Lives here, not in `doc.rs`, which stays free of view text.
fn problem_text(problem: &Problem, doc: &Doc) -> String {
    let label = |id: NodeId| match doc.node(id) {
        Some(node) => node_label(doc, node),
        None => node_number(doc, id),
    };
    match problem {
        Problem::CrossFlowEdge(_) => "Edge between two flows".to_string(),
        Problem::DanglingEdge(index) => format!("Edge {index} points at a missing node"),
        Problem::NoStart => "No start node — every node is linked to".into(),
        Problem::Unreachable(id) => format!("Unreachable: {}", label(*id)),
        // A jump with nothing leaving it is a jump to nowhere, not a conversation ending.
        Problem::DeadEnd(id) if doc.is_jump(*id) => format!("Jump with no target: {}", label(*id)),
        Problem::DeadEnd(id) => format!("Dead end: {}", label(*id)),
        // "Empty" means holding none of whatever the kind exists to hold.
        Problem::Empty(id) if doc.is_instruction(*id) => {
            format!("Sets nothing: {}", label(*id))
        }
        Problem::Empty(id) if doc.is_condition(*id) => {
            format!("Checks nothing: {}", label(*id))
        }
        Problem::Empty(id) => format!("No text: {}", label(*id)),
        Problem::UnknownVar(id, var) => {
            format!("Uses an undeclared variable \"{var}\": {}", label(*id))
        }
        Problem::TypeMismatch(id, var) => {
            format!("Wrong kind of value for \"{var}\": {}", label(*id))
        }
        Problem::UnwiredBranch(id, branch) => {
            format!("Nothing on the {branch} branch of: {}", label(*id))
        }
        Problem::UnknownSpeaker(id, speaker) => {
            format!("Nobody declared \"{speaker}\": {}", label(*id))
        }
        Problem::DuplicateAlias(id, alias) => {
            format!("Alias \"{alias}\" is used twice: {}", label(*id))
        }
        // Named by its ends rather than its index: two edges between the same pair is what
        // the author has to see, and the index alone says nothing about where to look.
        Problem::DuplicateEdge(index) => match doc.edges.get(*index) {
            Some(edge) => format!(
                "The same choice twice: {} → {}",
                label(edge.from),
                label(edge.to)
            ),
            None => format!("Edge {index} is a duplicate"),
        },
    }
}

/// The handle an edge is dragged out of.
fn port_dot(node: &Node, view: View) -> impl IntoElement {
    let (x, y) = view.to_screen(out_port(node));
    let r = view.scale(PORT_R);
    div()
        .absolute()
        .left(px(x - r))
        .top(px(y - r))
        .size(px(r * 2.0))
        .rounded_full()
        .bg(theme::PRESSED)
        .border_2()
        .border_color(theme::EDGE)
}

/// The bar down a node's left edge in its kind's colour, so a graph reads by structure —
/// dialogue, plumbing, notes — before any of it is read as text. It comes from `kind_style`
/// and nothing else: a speaker's colour is deliberately not here, because colour on the
/// box's outline would be confused with the selection and play highlights.
///
/// Absolute, so it takes no room from the content and every kind keeps its footprint. The
/// two left corners follow the box's own rounding (its radius less the 1px border) or the
/// bar would square off the corner.
fn kind_strip(kind: NodeKind, view: View) -> impl IntoElement {
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .bottom(px(0.0))
        // Floored, so a zoomed-out node still shows its kind.
        .w(px(view.scale(4.0).max(3.0)))
        .rounded_tl(px(5.0))
        .rounded_bl(px(5.0))
        .bg(kind_style(kind).2)
}

/// A hollow ring where an edge would arrive, on a node nothing links to — the top of a
/// conversation. A node that *is* linked to has no dot, on purpose: its arrowhead ends exactly
/// there, and a dot would sit on the tip and hide the direction the line is going.
fn root_dot(node: &Node, view: View) -> impl IntoElement {
    let (x, y) = view.to_screen(in_port(node));
    // Floored: at the zoom a document opens on, 5 units is a speck, and a 2px border on a
    // 5px dot leaves no hole — it stops reading as a ring.
    let r = view.scale(ROOT_R).max(4.0);
    div()
        .absolute()
        .left(px(x - r))
        .top(px(y - r))
        .size(px(r * 2.0))
        .rounded_full()
        .bg(theme::CANVAS)
        .border_2()
        .border_color(theme::EDGE)
}

/// The colour every line a character speaks is tinted, or `fallback` when nobody declared
/// that name — so an undeclared cast looks exactly as it did before there was one. The two
/// places a speaker is drawn share this, and nothing else is tinted: colour on a border or an
/// edge would make the selection and play highlights ambiguous.
fn speaker_color(doc: &Doc, speaker: &str, fallback: u32) -> Rgba {
    rgb(doc
        .character(speaker.trim())
        .map_or(fallback, |character| character.color))
}

/// `jump_target` is the label of the node a jump points at, and `speaker` the colour of its
/// cast member, both resolved by the caller.
///
/// Every kind keeps the same `NODE_W`×`NODE_H` footprint on purpose: hit-testing is
/// arithmetic over those constants in `doc.rs`, so a box drawn smaller than it is clicked
/// would put the two out of step. Kinds differ in what they draw, never in how big.
fn node_box(
    node: &Node,
    view: View,
    selected: bool,
    playing: bool,
    passed: bool,
    jump_target: Option<String>,
    speaker: Rgba,
) -> impl IntoElement {
    let (x, y) = view.to_screen((node.x, node.y));
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(view.scale(NODE_W)))
        .h(px(view.scale(NODE_H)))
        .text_size(px(view.scale(theme::CANVAS_TEXT)))
        .overflow_hidden()
        .flex()
        .flex_col()
        // ponytail: padding, border and radius stay fixed while the box scales, so a node
        // reads chrome-heavy at 0.25x and thin at 3x. Scale them too if that starts to grate.
        .gap_1()
        .p_2()
        .bg(theme::RAISED)
        .border_1()
        // Where the play-through is standing outranks where it has been, which outranks an
        // ending; the selection outranks all three.
        .border_color(match (selected, playing, passed, node.is_ending()) {
            (true, ..) => theme::ACCENT,
            (false, true, ..) => theme::PLAYING,
            (false, false, true, _) => theme::TRAIL,
            (false, false, false, true) => theme::ENDING,
            (false, false, false, false) => theme::OUTLINE,
        })
        .rounded_md()
        .shadow_md()
        .child(kind_strip(node.kind, view))
        .map(|this| match node.kind {
            NodeKind::Speech => this
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .text_color(speaker)
                        .one_line()
                        .child(node.speaker.clone()),
                )
                .child(
                    div()
                        .w_full()
                        .text_color(theme::TEXT)
                        .text_ellipsis()
                        .line_clamp(3)
                        .child(node.text.clone()),
                ),
            // A branch point says nothing, so its `text` is a label for it rather than a
            // line — an unnamed hub in a graph of hubs is not worth much.
            NodeKind::Hub => this.justify_center().items_center().child(
                div()
                    .text_color(theme::KIND_HUB)
                    .one_line()
                    .child(match node.text.trim() {
                        "" => "◇".to_string(),
                        label => format!("◇ {label}"),
                    }),
            ),
            NodeKind::Jump => this.justify_center().child(
                div()
                    .w_full()
                    .text_color(theme::KIND_JUMP)
                    .one_line()
                    .child(match jump_target {
                        Some(target) => format!("↪ {target}"),
                        None => "↪ nowhere".into(),
                    }),
            ),
            // The writes as they read on the page, which is the whole point of logic being
            // a node: `⚙ paid = true` is legible without opening anything.
            NodeKind::Instruction => this.justify_center().child(glyph_row(
                kind_style(NodeKind::Instruction).0,
                theme::KIND_INSTRUCTION,
                div()
                    .text_color(theme::TEXT)
                    .text_ellipsis()
                    .line_clamp(3)
                    .child(match node.effects.as_slice() {
                        [] => "(sets nothing)".to_string(),
                        effects => effects
                            .iter()
                            .map(|e| {
                                format!("{} {} {}", e.var, set_text(e.op), value_text(e.value))
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    }),
            )),
            // The checks as articy writes them — `? paid == true` — with no parser behind
            // it; the inspector edits them with the same picker an instruction uses.
            NodeKind::Condition => this.justify_center().child(glyph_row(
                kind_style(NodeKind::Condition).0,
                theme::KIND_CONDITION,
                div()
                    .text_color(theme::TEXT)
                    .text_ellipsis()
                    .line_clamp(3)
                    .child(match node.conditions.as_slice() {
                        [] => "(checks nothing)".to_string(),
                        conditions => conditions
                            .iter()
                            .map(|c| format!("{} {} {}", c.var, cmp_text(c.op), value_text(c.want)))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    }),
            )),
            // Its own font, so a note never reads as a line somebody says: italic, muted,
            // and no speaker line to share the box with. Same footprint as every other
            // kind — hit-testing is arithmetic over `NODE_W`/`NODE_H`.
            NodeKind::Note => this.child(glyph_row(
                kind_style(NodeKind::Note).0,
                theme::KIND_NOTE,
                div()
                    // Its own face, not just its own colour: the bundled UI font has no
                    // italic, so `.italic()` alone renders identically to a line of
                    // dialogue. A serif is the one thing on the canvas that is not.
                    .font_family(theme::NOTE_FONT)
                    .italic()
                    .text_color(theme::TEXT_MUTED)
                    .text_ellipsis()
                    .line_clamp(4)
                    .child(node.text.clone()),
            )),
        })
}

/// What the shared text field asks for, which depends on what it is editing: one field
/// serves a line, a hub's label and a note, and a hub's "Label" said "Line of dialog".
fn text_prompt(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Hub => "Optional label, e.g. \"Anything else?\"",
        NodeKind::Note => "A note to yourself",
        _ => "Line of dialog",
    }
}

/// A kind's glyph beside its content instead of above it. Stacked, the glyph took a whole
/// line of a box that is only `NODE_H` tall — the one line an instruction's second effect or
/// a note's fourth line of text needed. `flex_1` + `min_w_0` is what gives the content a
/// width to wrap and clip to, since it sits in a row now rather than filling a column.
fn glyph_row(glyph: &'static str, color: Rgba, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .items_start()
        .child(div().flex_none().text_color(color).child(glyph))
        .child(div().flex_1().min_w(px(0.0)).child(content))
}

/// How a kind introduces itself — glyph, name and colour. The canvas box, the inspector
/// header and the kind picker all read it, so they cannot disagree about what a hub looks
/// like. `Speech` has no glyph of its own on the canvas (its speaker header says who is
/// talking), so its pilcrow is only for the chip, the header and the find results. A quote
/// mark was the obvious choice and was unreadable at chip size: it rendered as a speck.
fn kind_style(kind: NodeKind) -> (&'static str, &'static str, Rgba) {
    match kind {
        NodeKind::Speech => ("¶", "Speech", theme::KIND_SPEECH),
        NodeKind::Hub => ("◇", "Hub", theme::KIND_HUB),
        NodeKind::Jump => ("↪", "Jump", theme::KIND_JUMP),
        NodeKind::Instruction => ("⚙", "Instruction", theme::KIND_INSTRUCTION),
        NodeKind::Condition => ("?", "Condition", theme::KIND_CONDITION),
        NodeKind::Note => ("▤", "Note", theme::KIND_NOTE),
    }
}

/// A value as the canvas and the play panel both write it.
fn value_text(value: Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
    }
}

/// How an instruction's write reads.
fn set_text(op: SetOp) -> &'static str {
    match op {
        SetOp::Set => "=",
        SetOp::Add => "+=",
    }
}

/// How a condition's check reads.
fn cmp_text(op: Cmp) -> &'static str {
    match op {
        Cmp::Eq => "==",
        Cmp::Ne => "!=",
        Cmp::Lt => "<",
        Cmp::Le => "<=",
        Cmp::Gt => ">",
        Cmp::Ge => ">=",
    }
}

/// The next comparison the inspector's click-cycle offers, ending at `None` — cleared — so
/// the gesture matches the flag cycle beside it.
fn next_cmp(op: Cmp) -> Option<Cmp> {
    match op {
        Cmp::Eq => Some(Cmp::Ne),
        Cmp::Ne => Some(Cmp::Lt),
        Cmp::Lt => Some(Cmp::Le),
        Cmp::Le => Some(Cmp::Gt),
        Cmp::Gt => Some(Cmp::Ge),
        Cmp::Ge => None,
    }
}

/// Filled triangle pointing at `tip`, aimed along the direction from `from`.
fn arrow_head(tip: Point<Pixels>, from: Point<Pixels>, zoom: f32) -> Option<Path<Pixels>> {
    let size = 10.0 * zoom;

    let (tip_x, tip_y) = (f32::from(tip.x), f32::from(tip.y));
    let (dx, dy) = (tip_x - f32::from(from.x), tip_y - f32::from(from.y));
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (base_x, base_y) = (tip_x - ux * size, tip_y - uy * size);
    // Perpendicular to the direction, half the head's width.
    let (wx, wy) = (-uy * size * 0.4, ux * size * 0.4);

    let mut head = PathBuilder::fill();
    head.move_to(tip);
    head.line_to(point(px(base_x + wx), px(base_y + wy)));
    head.line_to(point(px(base_x - wx), px(base_y - wy)));
    head.close();
    head.build().ok()
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            // `"Canvas"`, not `"Editor"`: these three mean something else in a text field,
            // and the canvas is the only place focus can be without being in one.
            KeyBinding::new("delete", DeleteNode, Some("Canvas")),
            KeyBinding::new("backspace", DeleteNode, Some("Canvas")),
            KeyBinding::new("tab", Branch, Some("Canvas")),
            // Bare `f`, so canvas-only like the three above: in a text field it is a letter.
            KeyBinding::new("f", FitView, Some("Canvas")),
            KeyBinding::new("ctrl-z", Undo, Some("Editor")),
            KeyBinding::new("ctrl-shift-z", Redo, Some("Editor")),
            KeyBinding::new("ctrl-=", ZoomIn, Some("Editor")),
            KeyBinding::new("ctrl-+", ZoomIn, Some("Editor")),
            KeyBinding::new("ctrl--", ZoomOut, Some("Editor")),
            KeyBinding::new("ctrl-0", ZoomReset, Some("Editor")),
            KeyBinding::new("ctrl-f", Find, Some("Editor")),
            KeyBinding::new("ctrl-shift-m", ToggleProblems, Some("Editor")),
            KeyBinding::new("f1", ShowHelp, Some("Editor")),
            KeyBinding::new("ctrl-k", ShowPalette, Some("Editor")),
            // `"Palette"` is set on the palette's card, an ancestor of its field, so these
            // mean something only while it is open — and a text field binds none of them.
            KeyBinding::new("down", PaletteNext, Some("Palette")),
            KeyBinding::new("up", PalettePrev, Some("Palette")),
            KeyBinding::new("enter", PaletteRun, Some("Palette")),
            // Ctrl+1 duplicates Tab so the number row is whole; the menu advertises Tab,
            // which is the older and better key for the kind you reach for most.
            KeyBinding::new("ctrl-1", Branch, Some("Editor")),
            KeyBinding::new("ctrl-2", NewHub, Some("Editor")),
            KeyBinding::new("ctrl-3", NewJump, Some("Editor")),
            KeyBinding::new("ctrl-4", NewInstruction, Some("Editor")),
            KeyBinding::new("ctrl-5", NewCondition, Some("Editor")),
            KeyBinding::new("ctrl-6", NewNote, Some("Editor")),
            KeyBinding::new("ctrl-y", Redo, Some("Editor")),
            KeyBinding::new("ctrl-n", New, Some("Editor")),
            KeyBinding::new("ctrl-s", Save, Some("Editor")),
            KeyBinding::new("ctrl-shift-s", SaveAs, Some("Editor")),
            KeyBinding::new("ctrl-o", Open, Some("Editor")),
            KeyBinding::new("ctrl-e", ExportFluent, Some("Editor")),
            KeyBinding::new("f5", Play, Some("Editor")),
            KeyBinding::new("shift-f5", PlayFromHere, Some("Editor")),
            // A no-op when nothing is playing. In a field the deeper `"TextInput"` binding
            // below wins, so escape there leaves the field instead of stopping play.
            KeyBinding::new("escape", StopPlay, Some("Editor")),
            KeyBinding::new("escape", FocusCanvas, Some("TextInput")),
        ]);
        text_input::bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Editor::new(window, cx)),
            )
            .unwrap();

        // Closing a dirty document has to be vetoable, and `on_window_closed` fires too
        // late to veto anything. So: refuse the first close, ask, and let the answer
        // drive a second one that goes through.
        handle
            .update(cx, |_, window, cx| {
                let editor = cx.entity();
                window.on_window_should_close(cx, move |window, cx| {
                    let settled = {
                        let this = editor.read(cx);
                        this.closing || !this.doc.is_dirty()
                    };
                    if settled {
                        return true;
                    }
                    editor.update(cx, |this, cx| this.confirm_close(handle, window, cx));
                    false
                });
            })
            .unwrap();

        // Anything a previous run left behind is offered back before the first frame.
        handle
            .update(cx, |this, window, cx| this.offer_recovery(window, cx))
            .unwrap();

        // Development builds only: a socket a script drives the editor through, so it can be
        // put into a state and screenshotted. See `automation.rs`.
        #[cfg(all(feature = "automation", unix))]
        handle.update(cx, automation::start).unwrap();

        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(query: &str) -> Vec<&'static str> {
        palette_entries(query)
            .into_iter()
            .map(|(_, (label, _, _))| *label)
            .collect()
    }

    #[test]
    fn an_empty_query_lists_every_command_but_the_palette_itself() {
        let all: usize = MENUS.iter().map(|(_, items)| items.len()).sum();
        assert_eq!(palette_entries("").len(), all - 1);
        assert!(!labels("").contains(&PALETTE_LABEL));
        // ...and so a query for it finds nothing, rather than a row that does nothing.
        assert!(labels("palette").is_empty());
    }

    #[test]
    fn words_match_in_any_order_and_any_case() {
        assert!(labels("node hub").contains(&"Hub"));
        assert!(labels("hub node").contains(&"Hub"));
        assert!(labels("FIT").contains(&"Fit to view"));
        assert!(labels("zoom nothing").is_empty());
    }

    #[test]
    fn a_label_that_starts_with_the_query_outranks_one_that_merely_has_it() {
        // "Zoom in" holds "in" mid-label; "Instruction" starts with it.
        let found = labels("in");
        let instruction = found.iter().position(|l| *l == "Instruction").unwrap();
        let zoom_in = found.iter().position(|l| *l == "Zoom in").unwrap();
        assert!(instruction < zoom_in);
    }

    #[test]
    fn a_match_on_the_menu_name_alone_ranks_last() {
        // `Stop` is in the Play menu but does not say "play".
        let found = labels("play");
        assert_eq!(found.last(), Some(&"Stop"));
        assert!(found.len() > 1);
    }
}
