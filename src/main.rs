mod doc;
mod text_input;

use doc::{
    Doc, FILE_EXT, NODE_H, NODE_W, Node, PORT_R, Problem, Severity, View, clear_backup, in_port,
    node_center, out_port, read_backup, with_default_extension,
};
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder, PathPromptOptions, Pixels,
    Point, PromptLevel, Render, Rgba, ScrollWheelEvent, Window, WindowBounds, WindowHandle,
    WindowOptions, actions, canvas, deferred, div, point, prelude::*, px, rgb, rgba, size,
};
use std::path::PathBuf;
use std::time::Duration;
use text_input::TextInput;

actions!(
    papugai,
    [
        DeleteNode,
        Undo,
        Redo,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Find,
        Branch,
        FocusCanvas,
        New,
        Save,
        SaveAs,
        Open,
        Play,
        PlayFromHere,
        StopPlay
    ]
);

/// Height of the menu bar. Mouse positions are window-relative, so this is also the
/// offset between window coordinates and the canvas pane below the bar.
const MENU_H: f32 = 30.0;
/// Width of the inspector on the right. With [`MENU_H`], this is what bounds the canvas.
const SIDEBAR_W: f32 = 280.0;

/// How much one pixel of scroll changes the zoom. Multiplicative, so a scroll up and an
/// equal scroll down cancel out exactly.
const ZOOM_PER_PIXEL: f32 = 1.0015;
/// Zoom step for the keyboard commands. `1 / 1.25 == 0.8`, so in-then-out is a round trip.
const ZOOM_STEP: f32 = 1.25;
/// Node and label text size at 1:1, matching the `text_sm` the rest of the UI uses.
const BASE_TEXT: f32 = 14.0;

/// How often a dirty document gets a crash copy. Also the most work a crash can cost.
const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

type MenuAction = fn(&mut Editor, &mut Window, &mut Context<Editor>);
/// Label, shortcut hint, what it runs.
type MenuItem = (&'static str, &'static str, MenuAction);
type MenuSpec = (&'static str, &'static [MenuItem]);

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
        ],
    ),
    (
        "Edit",
        &[
            ("Undo", "Ctrl+Z", |e, w, cx| e.on_undo(&Undo, w, cx)),
            ("Redo", "Ctrl+Shift+Z", |e, w, cx| e.on_redo(&Redo, w, cx)),
            ("Zoom in", "Ctrl+=", |e, w, cx| e.on_zoom_in(&ZoomIn, w, cx)),
            ("Zoom out", "Ctrl+-", |e, w, cx| {
                e.on_zoom_out(&ZoomOut, w, cx)
            }),
            ("Actual size", "Ctrl+0", |e, w, cx| {
                e.on_zoom_reset(&ZoomReset, w, cx)
            }),
            ("Find…", "Ctrl+F", |e, w, cx| e.on_find(&Find, w, cx)),
            ("New connected node", "Tab", |e, w, cx| {
                e.on_branch(&Branch, w, cx)
            }),
            ("Delete selection", "Del", |e, w, cx| {
                e.on_delete(&DeleteNode, w, cx)
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

const EDGE_COLOR: Rgba = Rgba {
    r: 0.49,
    g: 0.49,
    b: 0.55,
    a: 1.0,
};
const ACCENT_COLOR: Rgba = Rgba {
    r: 0.48,
    g: 0.64,
    b: 0.97,
    a: 1.0,
};

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
    label_input: Entity<TextInput>,
    /// Search query. Deliberately outside the `sync_fields` loop — it is not document content.
    search_input: Entity<TextInput>,
    /// What is currently loaded into the sidebar fields.
    edited: Selection,
    /// Whether this editing session already pushed an undo checkpoint.
    text_dirty: bool,
    /// Where the document was last saved or loaded from.
    path: Option<PathBuf>,
    /// Outcome of the last save or load, shown in the sidebar.
    status: Option<String>,
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
    play: Option<Vec<u32>>,
}

/// Node ids are stable within a history step, so a node selection can be a set.
/// Edge indices are not — they shift when edges are removed, so an edge selection
/// stays singular and is cleared on every structural change.
#[derive(Clone, Default, PartialEq)]
enum Selection {
    #[default]
    None,
    Nodes(Vec<u32>),
    Edge(usize),
}

#[derive(Clone, Copy)]
enum Drag {
    /// `grab` is the cursor's offset inside the node, in document coordinates. The rest
    /// of the selection rides along, so this drags a whole group.
    Node {
        id: u32,
        grab: (f32, f32),
        moved: bool,
    },
    /// Rubber band from a node's out-port to the cursor.
    Edge { from: u32, cursor: (f32, f32) },
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
    fn nodes(&self) -> &[u32] {
        match self {
            Selection::Nodes(ids) => ids,
            _ => &[],
        }
    }

    /// The one selected node, when the selection is exactly one node. Everything that
    /// edits a single node goes through this, so a multi-selection simply edits nothing.
    fn only_node(&self) -> Option<u32> {
        match self.nodes() {
            [id] => Some(*id),
            _ => None,
        }
    }

    /// Adds the id when absent, removes it when present — what shift-click does.
    fn toggled(&self, id: u32) -> Selection {
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
    fn of(ids: Vec<u32>) -> Selection {
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
        let label_input = cx.new(|cx| TextInput::new("Choice", cx));
        for input in [&speaker_input, &text_input, &label_input] {
            cx.observe(input, |this, _, cx| this.sync_fields(cx))
                .detach();
        }

        // The search field is not document content, so it stays out of the loop above and
        // only asks for a re-render — without this, typing would not refresh the results.
        let search_input = cx.new(|cx| TextInput::new("Find a node", cx));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

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
            label_input,
            search_input,
            edited: Selection::None,
            text_dirty: false,
            path: None,
            status: None,
            title: String::new(),
            open_menu: None,
            closing: false,
            autosave_off: false,
            play: None,
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
        self.drag = None;
        // Node ids belong to the document that is going away.
        self.play = None;
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
        let path = with_default_extension(path);
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

    fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
    }

    /// Loads the selection's content into the sidebar fields, starting a fresh editing session.
    fn load_fields(&mut self, selection: Selection, cx: &mut Context<Self>) {
        let (speaker, text, label) = match (selection.only_node(), &selection) {
            (Some(id), _) => match self.doc.node(id) {
                Some(node) => (node.speaker.clone(), node.text.clone(), String::new()),
                None => Default::default(),
            },
            (None, Selection::Edge(index)) => match self.doc.edges.get(*index) {
                Some(edge) => (String::new(), String::new(), edge.label.clone()),
                None => Default::default(),
            },
            _ => Default::default(),
        };

        self.speaker_input
            .update(cx, |input, cx| input.set_content(&speaker, cx));
        self.text_input
            .update(cx, |input, cx| input.set_content(&text, cx));
        self.label_input
            .update(cx, |input, cx| input.set_content(&label, cx));
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

                let Some(node) = self.doc.node(id) else {
                    return;
                };
                if node.speaker == speaker && node.text == text {
                    return;
                }
                self.checkpoint_once();
                if let Some(node) = self.doc.node_mut(id) {
                    node.speaker = speaker;
                    node.text = text;
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

    /// Mouse positions arrive in window coordinates; the canvas starts below the menu bar.
    fn window_pos(pos: Point<Pixels>) -> (f32, f32) {
        (f32::from(pos.x), f32::from(pos.y) - MENU_H)
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
        let size = window.viewport_size();
        (
            (f32::from(size.width) - SIDEBAR_W) / 2.0,
            (f32::from(size.height) - MENU_H) / 2.0,
        )
    }

    /// One entry per edge: the line from the source's out-port to the target's in-port,
    /// and whether it is the selected edge.
    fn edge_lines(&self) -> Vec<(Point<Pixels>, Point<Pixels>, bool)> {
        self.doc
            .edges
            .iter()
            .enumerate()
            .filter_map(|(index, e)| {
                let from = self.doc.node(e.from)?;
                let to = self.doc.node(e.to)?;
                Some((
                    self.screen_pos(out_port(from)),
                    self.screen_pos(in_port(to)),
                    self.selection == Selection::Edge(index),
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
        if let Some(from) = self.doc.port_hit_test(x, y) {
            self.drag = Some(Drag::Edge {
                from,
                cursor: (x, y),
            });
        } else if let Some(id) = self.doc.hit_test(x, y) {
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
        } else if let Some(index) = self.doc.edge_at(x, y) {
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
                if let Some(to) = self.doc.hit_test(x, y)
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
                for id in self.doc.nodes_in_rect(start, cursor) {
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
        let live: Vec<u32> = self
            .selection
            .nodes()
            .iter()
            .copied()
            .filter(|id| self.doc.node(*id).is_some())
            .collect();
        let selection = Selection::of(live);
        self.selection = selection.clone();
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

    fn on_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search_input.read(cx).focus_handle(cx));
    }

    /// Creates a node already connected to the selected one and drops the cursor in its
    /// Speaker field, so a branch is one keystroke instead of a double-click and a drag.
    fn on_branch(&mut self, _: &Branch, window: &mut Window, cx: &mut Context<Self>) {
        // Exactly one node, since a branch needs one parent to hang off.
        let Some(from) = self.selection.only_node() else {
            return;
        };
        // Guard before the checkpoint: one whose mutation then fails is an empty undo
        // step and a false dirty flag.
        if self.doc.node(from).is_none() {
            return;
        }
        self.doc.checkpoint();
        let Some(id) = self.doc.branch_from(from) else {
            return;
        };
        self.select(Selection::Nodes(vec![id]), cx);
        window.focus(&self.speaker_input.read(cx).focus_handle(cx));
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

    fn on_stop_play(&mut self, _: &StopPlay, _: &mut Window, cx: &mut Context<Self>) {
        self.play = None;
        cx.notify();
    }

    /// The node play starts from: the selected one, or the first in the document so that a
    /// stray F5 plays something rather than nagging.
    fn play_seed(&self) -> Option<u32> {
        self.selection
            .only_node()
            .or_else(|| self.doc.nodes.first().map(|n| n.id))
    }

    fn start_play(&mut self, at: u32, window: &Window, cx: &mut Context<Self>) {
        self.play = Some(vec![at]);
        self.show(at, window);
        cx.notify();
    }

    /// Advances the play-through, or ends it if the choice leads nowhere real.
    fn play_advance(&mut self, to: u32, window: &Window, cx: &mut Context<Self>) {
        if let Some(path) = self.play.as_mut() {
            path.push(to);
            self.show(to, window);
            cx.notify();
        }
    }

    /// Brings a node into view without touching the selection — which is what separates
    /// following a play-through from [`Editor::jump_to`].
    fn show(&mut self, id: u32, window: &Window) {
        if let Some(node) = self.doc.node(id) {
            self.view
                .center_on(node_center(node), Self::pane_center(window));
        }
    }

    /// Selects a node and brings it into view without changing zoom.
    fn jump_to(&mut self, id: u32, window: &Window, cx: &mut Context<Self>) {
        if self.doc.node(id).is_none() {
            return;
        }
        self.show(id, window);
        self.select(Selection::Nodes(vec![id]), cx);
        cx.notify();
    }

    fn on_focus_canvas(&mut self, _: &FocusCanvas, window: &mut Window, _: &mut Context<Self>) {
        window.focus(&self.focus_handle);
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = format!(
            "papugai — {}{}",
            self.file_name(),
            if self.doc.is_dirty() { "*" } else { "" }
        );
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        let lines = self.edge_lines();
        let pending = self.pending_edge();
        let view = self.view;

        div()
            .flex()
            .flex_col()
            .size_full()
            .text_sm()
            .child(self.menu_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.canvas_pane(lines, pending, view, cx))
                    .child(self.sidebar(cx)),
            )
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
            .bg(rgb(0x26262c))
            .border_b_1()
            .border_color(rgb(0x3a3a44))
            .text_color(rgb(0xd6d6dd))
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
                            .when(open, |this| this.bg(rgb(0x3a3a44)))
                            .hover(|style| style.bg(rgb(0x33333c)))
                            .child(*name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_menu = (!open).then_some(*name);
                                cx.notify();
                            })),
                    )
                    .when(open, |this| this.child(menu_dropdown(items, cx)))
            }))
    }

    /// The graph itself. Owns the mouse and key handling, so sidebar clicks never reach it.
    fn canvas_pane(
        &self,
        lines: Vec<(Point<Pixels>, Point<Pixels>, bool)>,
        pending: Option<(Point<Pixels>, Point<Pixels>)>,
        view: View,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(rgb(0x1e1e22))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_reset))
            .on_action(cx.listener(Self::on_find))
            .on_action(cx.listener(Self::on_branch))
            .on_action(cx.listener(Self::on_focus_canvas))
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_play))
            .on_action(cx.listener(Self::on_play_from_here))
            .on_action(cx.listener(Self::on_stop_play))
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
                        let mut draw = |from: Point<Pixels>, to: Point<Pixels>, color: Rgba| {
                            let (from, to) = (from + bounds.origin, to + bounds.origin);
                            let mut line = PathBuilder::stroke(px(1.5));
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

                        for (from, to, selected) in lines {
                            draw(from, to, if selected { ACCENT_COLOR } else { EDGE_COLOR });
                        }
                        if let Some((from, to)) = pending {
                            draw(from, to, ACCENT_COLOR);
                        }
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .children(self.edge_labels())
            .children(self.doc.nodes.iter().map(|node| {
                let playing = self.play.as_ref().and_then(|path| path.last().copied());
                node_box(
                    node,
                    view,
                    self.selection.nodes().contains(&node.id),
                    playing == Some(node.id),
                )
            }))
            .children(self.doc.nodes.iter().map(|node| port_dot(node, view)))
            .when_some(self.marquee_rect(), |this, (origin, size)| {
                this.child(
                    div()
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(size.x)
                        .h(size.y)
                        .bg(rgba(0x7aa2f722))
                        .border_1()
                        .border_color(ACCENT_COLOR),
                )
            })
    }

    /// Choice text, centred on each labelled edge.
    fn edge_labels(&self) -> Vec<impl IntoElement> {
        const W: f32 = 120.0;

        self.doc
            .edges
            .iter()
            .filter(|edge| !edge.label.is_empty())
            .filter_map(|edge| {
                let from = out_port(self.doc.node(edge.from)?);
                let to = in_port(self.doc.node(edge.to)?);
                let mid = self.screen_pos(((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
                let width = self.view.scale(W);
                Some(
                    // Fixed width and centred content, so the label centres without measuring it.
                    div()
                        .absolute()
                        .left(mid.x - px(width / 2.0))
                        .top(mid.y - px(self.view.scale(18.)))
                        .w(px(width))
                        .flex()
                        .justify_center()
                        .text_size(px(self.view.scale(BASE_TEXT)))
                        .child(
                            div()
                                .px_1()
                                .bg(rgb(0x1e1e22))
                                .text_color(rgb(0x9a9aa6))
                                .truncate()
                                .child(edge.label.clone()),
                        ),
                )
            })
            .collect()
    }

    /// Search box and results, above the inspector so both stay usable at once.
    fn search_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        /// Beyond this the list stops being scannable, and the sidebar would need to scroll.
        const MAX_ROWS: usize = 20;

        let query = self.search_input.read(cx).content().to_string();
        // ponytail: rescans every node each frame, but only while a query is active — which
        // is never while dragging. Cache it if searching a huge graph ever stutters.
        let hits = self.doc.find(&query);
        let more = hits.len().saturating_sub(MAX_ROWS);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.search_input.clone())
            .when(!query.trim().is_empty(), |this| {
                this.when(hits.is_empty(), |this| {
                    this.child(div().text_color(rgb(0x6c6c78)).child("No matches."))
                })
                .children(hits.iter().take(MAX_ROWS).filter_map(|&id| {
                    let label = node_label(self.doc.node(id)?);
                    Some(
                        div()
                            .id(("search-result", id))
                            .px_1()
                            .rounded_sm()
                            .truncate()
                            .text_color(rgb(0xb8b8c4))
                            .hover(|style| style.bg(rgb(0x33333c)))
                            .child(label)
                            .on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.jump_to(id, window, cx)
                                }),
                            ),
                    )
                }))
                .when(more > 0, |this| {
                    this.child(
                        div()
                            .text_color(rgb(0x6c6c78))
                            .child(format!("…and {more} more")),
                    )
                })
            })
    }

    /// The conversation as a reader meets it: what the current node says, and the choices
    /// leading out of it. Replaces the inspector while it is up, so the sidebar is never
    /// both an editor and a player.
    fn play_panel(&self, path: &[u32], cx: &mut Context<Self>) -> impl IntoElement {
        let row = |id: (&'static str, usize), label: String, color: u32| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_color(rgb(color))
                .hover(|style| style.bg(rgb(0x33333c)))
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
                    .child(div().text_color(rgb(0x9ecbff)).child("Playing"))
                    .child(row(("play-stop", 0), "Stop".into(), 0x8a8a96).on_click(
                        cx.listener(|this, _, window, cx| this.on_stop_play(&StopPlay, window, cx)),
                    )),
            )
            .map(|this| {
                // The node can be deleted from the canvas mid-play; say so rather than
                // silently showing nothing.
                let Some(node) = node else {
                    return this.child(
                        div()
                            .text_color(rgb(0xff7a7a))
                            .child("This node has been deleted."),
                    );
                };

                let speaker = node.speaker.trim();
                let text = node.text.trim();
                this.when(!speaker.is_empty(), |this| {
                    this.child(div().text_color(rgb(0x8a8a96)).child(speaker.to_string()))
                })
                .child(if text.is_empty() {
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("(no line written yet)")
                } else {
                    div().child(text.to_string())
                })
            })
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
                        row(("play-choice", i), format!("→ {label}"), 0xd6d6dd).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.play_advance(to, window, cx)
                            }),
                        )
                    }),
            )
            .when(self.doc.choices(current).is_empty(), |this| {
                this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("End of conversation."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .when(path.len() > 1, |this| {
                        this.child(row(("play-nav", 0), "← Back".into(), 0x8a8a96).on_click(
                            cx.listener(|this, _, window, cx| {
                                let Some(path) = this.play.as_mut() else {
                                    return;
                                };
                                path.pop();
                                let back = *path.last().expect("Back is hidden at the top");
                                this.show(back, window);
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        row(("play-nav", 1), "Restart".into(), 0x8a8a96).on_click(cx.listener(
                            |this, _, window, cx| {
                                let Some(path) = this.play.as_mut() else {
                                    return;
                                };
                                path.truncate(1);
                                let start = path[0];
                                this.show(start, window);
                                cx.notify();
                            },
                        )),
                    ),
            )
    }

    /// Everything wrong with the document, worst first. Clicking a row jumps to the node.
    fn problems_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        /// Same ceiling as the search results, for the same reason.
        const MAX_ROWS: usize = 20;

        // ponytail: revalidates the whole document each frame. Cheap at this scale, and a
        // cache would need invalidating on every edit, undo and load.
        let problems = self.doc.problems();
        let more = problems.len().saturating_sub(MAX_ROWS);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(!problems.is_empty(), |this| {
                this.child(
                    div()
                        .text_color(rgb(0x8a8a96))
                        .child(format!("Problems ({})", problems.len())),
                )
                .children(
                    problems
                        .iter()
                        .take(MAX_ROWS)
                        .enumerate()
                        .map(|(i, problem)| {
                            let color = match problem.severity() {
                                Severity::Error => rgb(0xff7a7a),
                                Severity::Warning => rgb(0xb8b8c4),
                            };
                            div()
                                .id(("problem", i))
                                .px_1()
                                .rounded_sm()
                                .truncate()
                                .text_color(color)
                                .child(problem_text(problem, &self.doc))
                                // A dangling edge has no live node to jump to, so its row is inert.
                                .when_some(problem.node(), |this, id| {
                                    this.hover(|style| style.bg(rgb(0x33333c))).on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.jump_to(id, window, cx)
                                        }),
                                    )
                                })
                        }),
                )
                .when(more > 0, |this| {
                    this.child(
                        div()
                            .text_color(rgb(0x6c6c78))
                            .child(format!("…and {more} more")),
                    )
                })
            })
    }

    /// Marks a node as a deliberate ending, which is what keeps it out of the dead-end
    /// warnings. A discrete edit, so it checkpoints outright rather than coalescing.
    fn end_toggle(&self, id: u32, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.doc.node(id).is_some_and(|node| node.end);
        div()
            .id("end-toggle")
            .flex()
            .gap_2()
            .cursor_pointer()
            .text_color(if on { rgb(0xf7c96a) } else { rgb(0x8a8a96) })
            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let field = |label: &str, input: &Entity<TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(rgb(0x8a8a96)).child(label.to_string()))
                .child(input.clone())
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(SIDEBAR_W))
            .h_full()
            .p_3()
            .bg(rgb(0x26262c))
            .border_l_1()
            .border_color(rgb(0x3a3a44))
            .text_color(rgb(0xd6d6dd))
            .child(self.search_panel(cx))
            .when_some(self.play.clone(), |this, path| {
                this.child(self.play_panel(&path, cx))
            })
            .when(self.play.is_none(), |this| {
                this.map(|this| match (self.selection.only_node(), &self.selection) {
                    (Some(id), _) => this
                        .child(div().text_color(rgb(0x9ecbff)).child(format!("Node {id}")))
                        .child(field("Speaker", &self.speaker_input))
                        .child(field("Text", &self.text_input))
                        .child(self.end_toggle(id, cx)),
                    (None, Selection::Nodes(ids)) => this
                        .child(
                            div()
                                .text_color(rgb(0x9ecbff))
                                .child(format!("{} nodes selected", ids.len())),
                        )
                        .child(
                            div()
                                .text_color(rgb(0x6c6c78))
                                .child("Drag any of them to move the group."),
                        ),
                    (None, Selection::Edge(index)) => {
                        let title = match self.doc.edges.get(*index) {
                            Some(edge) => format!("Node {} → Node {}", edge.from, edge.to),
                            None => "Edge".into(),
                        };
                        this.child(div().text_color(rgb(0x9ecbff)).child(title))
                            .child(field("Choice", &self.label_input))
                    }
                    _ => this.child(
                        div()
                            .text_color(rgb(0x6c6c78))
                            .child("Select a node or an edge to edit it."),
                    ),
                })
            })
            .child(self.problems_panel(cx))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_color(rgb(0x6c6c78))
                    .when_some(self.status.clone(), |this, status| this.child(status))
                    .when(self.selection != Selection::None, |this| {
                        this.child("Esc — back to canvas")
                    })
                    .child("Middle-drag pans · Shift-click adds to the selection")
                    .child("Ctrl+N new · Ctrl+S save · Ctrl+O open"),
            )
    }
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
            .bg(rgb(0x2b2b33))
            .border_1()
            .border_color(rgb(0x4a4a57))
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
                    .hover(|style| style.bg(rgb(0x3a3a44)))
                    .child(*label)
                    .child(div().text_color(rgb(0x8a8a96)).child(*shortcut))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_menu = None;
                        run(this, window, cx);
                        cx.notify();
                    }))
            })),
    )
}

/// How a node reads in a list: what it says, or who says it, or its id as a last resort.
fn node_label(node: &Node) -> String {
    match (node.speaker.trim(), node.text.trim()) {
        ("", "") => format!("Node {}", node.id),
        ("", text) => text.to_string(),
        (speaker, "") => speaker.to_string(),
        (speaker, text) => format!("{speaker} — {text}"),
    }
}

/// Wording for a problem row. Lives here, not in `doc.rs`, which stays free of view text.
fn problem_text(problem: &Problem, doc: &Doc) -> String {
    let label = |id: u32| match doc.node(id) {
        Some(node) => node_label(node),
        None => format!("Node {id}"),
    };
    match problem {
        Problem::DanglingEdge(index) => format!("Edge {index} points at a missing node"),
        Problem::NoStart => "No start node — every node is linked to".into(),
        Problem::Unreachable(id) => format!("Unreachable: {}", label(*id)),
        Problem::DeadEnd(id) => format!("Dead end: {}", label(*id)),
        Problem::Empty(id) => format!("No text: {}", label(*id)),
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
        .bg(rgb(0x3a3a44))
        .border_2()
        .border_color(rgb(0x7d7d8c))
}

fn node_box(node: &Node, view: View, selected: bool, playing: bool) -> impl IntoElement {
    let (x, y) = view.to_screen((node.x, node.y));
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(view.scale(NODE_W)))
        .h(px(view.scale(NODE_H)))
        .text_size(px(view.scale(BASE_TEXT)))
        .overflow_hidden()
        .flex()
        .flex_col()
        // ponytail: padding, border and radius stay fixed while the box scales, so a node
        // reads chrome-heavy at 0.25x and thin at 3x. Scale them too if that starts to grate.
        .gap_1()
        .p_2()
        .bg(rgb(0x2b2b33))
        .border_1()
        // Where the play-through is standing outranks an ending; the selection outranks both.
        .border_color(match (selected, playing, node.end) {
            (true, _, _) => rgb(0x7aa2f7),
            (false, true, _) => rgb(0x8ce99a),
            (false, false, true) => rgb(0xf7c96a),
            (false, false, false) => rgb(0x4a4a57),
        })
        .rounded_md()
        .shadow_md()
        .child(
            div()
                .flex_none()
                .w_full()
                .text_color(rgb(0x9ecbff))
                .truncate()
                .child(node.speaker.clone()),
        )
        .child(
            div()
                .w_full()
                .text_color(rgb(0xd6d6dd))
                .text_ellipsis()
                .line_clamp(3)
                .child(node.text.clone()),
        )
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
            KeyBinding::new("delete", DeleteNode, Some("Editor")),
            KeyBinding::new("backspace", DeleteNode, Some("Editor")),
            KeyBinding::new("ctrl-z", Undo, Some("Editor")),
            KeyBinding::new("ctrl-shift-z", Redo, Some("Editor")),
            KeyBinding::new("ctrl-=", ZoomIn, Some("Editor")),
            KeyBinding::new("ctrl-+", ZoomIn, Some("Editor")),
            KeyBinding::new("ctrl--", ZoomOut, Some("Editor")),
            KeyBinding::new("ctrl-0", ZoomReset, Some("Editor")),
            KeyBinding::new("ctrl-f", Find, Some("Editor")),
            KeyBinding::new("tab", Branch, Some("Editor")),
            KeyBinding::new("ctrl-y", Redo, Some("Editor")),
            KeyBinding::new("ctrl-n", New, Some("Editor")),
            KeyBinding::new("ctrl-s", Save, Some("Editor")),
            KeyBinding::new("ctrl-shift-s", SaveAs, Some("Editor")),
            KeyBinding::new("ctrl-o", Open, Some("Editor")),
            KeyBinding::new("f5", Play, Some("Editor")),
            KeyBinding::new("shift-f5", PlayFromHere, Some("Editor")),
            // A no-op when nothing is playing; the field's own escape is scoped elsewhere.
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

        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
