mod doc;
mod text_input;

use doc::{
    Cmp, Doc, FILE_EXT, FTL_EXT, Mention, NODE_H, NODE_W, Node, NodeId, NodeKind, PORT_R, Problem,
    Ref, SCRIPT_EXT, SetOp, Severity, Use, Value, VarState, View, clear_backup, in_port, join_set,
    node_center, out_port, read_backup, snippet, split_set, with_default_extension,
};
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder, PathPromptOptions, Pixels,
    Point, PromptLevel, Render, Rgba, ScrollWheelEvent, SharedString, Window, WindowBounds,
    WindowHandle, WindowOptions, actions, canvas, deferred, div, point, prelude::*, px, rgb, rgba,
    size,
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
        ExportScreenplay,
        Open,
        Play,
        PlayFromHere,
        StopPlay
    ]
);

/// Height of the menu bar. Mouse positions are window-relative, so this is also the
/// offset between window coordinates and the canvas pane below the bar.
const MENU_H: f32 = 30.0;
/// Width of the inspector on the right.
const SIDEBAR_W: f32 = 280.0;
/// Width of the global-variables pane on the left. With [`SIDEBAR_W`] and [`MENU_H`], this
/// is what bounds the canvas — and what `window_pos` has to subtract off every mouse event.
const VARS_W: f32 = 200.0;

/// How much one pixel of scroll changes the zoom. Multiplicative, so a scroll up and an
/// equal scroll down cancel out exactly.
const ZOOM_PER_PIXEL: f32 = 1.0015;
/// Zoom step for the keyboard commands. `1 / 1.25 == 0.8`, so in-then-out is a round trip.
const ZOOM_STEP: f32 = 1.25;
/// Node and label text size at 1:1, matching the `text_sm` the rest of the UI uses.
const BASE_TEXT: f32 = 14.0;

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

/// The colours a character can be tinted, cycled by clicking the swatch. Drawn from the
/// colours already in this file, and it lives here rather than in `doc.rs` for the same
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
            ("Export Fluent…", "", |e, w, cx| {
                e.on_export_fluent(&ExportFluent, w, cx)
            }),
            ("Export Screenplay…", "", |e, w, cx| {
                e.on_export_screenplay(&ExportScreenplay, w, cx)
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
/// A condition's two pins. Drawn as well as labelled, so which branch an edge leaves by is
/// never something the author has to guess at.
const TRUE_COLOR: Rgba = Rgba {
    r: 0.55,
    g: 0.91,
    b: 0.60,
    a: 1.0,
};
const FALSE_COLOR: Rgba = Rgba {
    r: 1.0,
    g: 0.48,
    b: 0.48,
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
    play: Option<Vec<NodeId>>,
    /// Which list the left pane is showing. Its only new state: the cast rides the container
    /// and the text field the variables list already proved out, and a *third* pane was
    /// rejected because `window_pos` is the one place a new one breaks every hit test.
    pane: SidePane,
    /// Which canvas is on screen. A flow is a *view* onto the one flat node list, so this
    /// is editor state and never serialized — the whole feature is this filter plus a tab.
    flow: String,
    /// Whose "Used by" list is open under its row in the left pane, if any. View state, and
    /// filtered at render like `renaming`: a name the document no longer has matches no
    /// row, so the list simply is not drawn.
    used_by: Option<Ref>,
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
            title: String::new(),
            open_menu: None,
            closing: false,
            autosave_off: false,
            play: None,
            pane: SidePane::default(),
            used_by: None,
            flow: String::new(),
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

    /// Writes the translatable strings out for a game build.
    fn on_export_fluent(&mut self, _: &ExportFluent, _: &mut Window, cx: &mut Context<Self>) {
        self.export(FTL_EXT, Doc::export_ftl, cx);
    }

    /// Writes the dialogue out as a script for a table read or a recording session.
    fn on_export_screenplay(
        &mut self,
        _: &ExportScreenplay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export(SCRIPT_EXT, Doc::export_screenplay, cx);
    }

    /// Asks where, then runs `write` there. Deliberately not a save: it leaves `self.path`,
    /// the dirty flag and the crash copy alone, so exporting never re-points the editor at
    /// the file it wrote — and `write` takes `&Doc`, so it could not if it tried.
    fn export(
        &mut self,
        ext: &'static str,
        write: fn(&Doc, &std::path::Path) -> Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let suggested = PathBuf::from(self.file_name())
            .with_extension(ext)
            .to_string_lossy()
            .into_owned();
        let response = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn(async move |this, cx| {
            match response.await {
                Ok(Ok(Some(path))) => {
                    this.update(cx, |this, cx| {
                        let path = with_default_extension(path, ext);
                        let status = match write(&this.doc, &path) {
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

    fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
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
        let size = window.viewport_size();
        (
            (f32::from(size.width) - SIDEBAR_W - VARS_W) / 2.0,
            (f32::from(size.height) - MENU_H) / 2.0,
        )
    }

    /// One entry per edge: the line from the source's out-port to the target's in-port, and
    /// the colour to draw it in — the selection outranks a condition's branch, which
    /// outranks the ordinary edge colour. An edge leaving a jump is not drawn at all: the
    /// node itself names its target, which is the whole point of the kind.
    fn edge_lines(&self) -> Vec<(Point<Pixels>, Point<Pixels>, Rgba)> {
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
                let color = match (self.selection == Selection::Edge(index), e.branch) {
                    (true, _) => ACCENT_COLOR,
                    _ if !self.doc.is_condition(e.from) => EDGE_COLOR,
                    (false, true) => TRUE_COLOR,
                    (false, false) => FALSE_COLOR,
                };
                Some((
                    self.screen_pos(out_port(from)),
                    self.screen_pos(in_port(to)),
                    color,
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

    fn on_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search_input.read(cx).focus_handle(cx));
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
                let id = self.doc.add_node(x - NODE_W / 2.0, y - NODE_H / 2.0);
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

    fn on_stop_play(&mut self, _: &StopPlay, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_focus_canvas(&mut self, _: &FocusCanvas, window: &mut Window, _: &mut Context<Self>) {
        window.focus(&self.focus_handle);
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = format!(
            "papugai — {}{}{}",
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
                    .child(self.vars_pane(cx))
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
        lines: Vec<(Point<Pixels>, Point<Pixels>, Rgba)>,
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
            .on_action(cx.listener(Self::on_export_screenplay))
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

                        for (from, to, color) in lines {
                            draw(from, to, color);
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
            .children(self.doc.nodes_in(&self.flow).map(|node| {
                let playing = self.play.as_ref().and_then(|path| path.last().copied());
                let target = (node.kind == NodeKind::Jump)
                    .then(|| self.doc.choices(node.id).first().map(|&(to, _)| to))
                    .flatten()
                    .and_then(|to| self.doc.node(to));
                node_box(
                    node,
                    view,
                    self.selection.nodes().contains(&node.id),
                    playing == Some(node.id),
                    target.map(|target| node_label(&self.doc, target)),
                    target.map(|target| JumpPreview::of(&self.doc, target)),
                    speaker_color(&self.doc, &node.speaker, 0x9ecbff),
                )
            }))
            .children(
                self.doc
                    .nodes_in(&self.flow)
                    // A note takes no edges, so it gets no handle to drag one from —
                    // `Doc::port_hit_test` skips it for the same reason.
                    .filter(|node| node.kind != NodeKind::Note)
                    .map(|node| port_dot(node, view)),
            )
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
            // A condition's edges always carry a label — which pin they leave by — even
            // though the player never chooses one, so the empty-label skip lets them past.
            .filter(|edge| {
                !self.doc.is_jump(edge.from)
                    && (!edge.label.is_empty() || self.doc.is_condition(edge.from) || edge.once)
            })
            .filter_map(|edge| {
                let (text, color) = if self.doc.is_condition(edge.from) {
                    match edge.branch {
                        true => ("true".to_string(), TRUE_COLOR),
                        false => ("false".to_string(), FALSE_COLOR),
                    }
                } else if edge.once {
                    // Shown on the canvas as well as in the inspector, the way a condition's
                    // branch is: a flag you have to click every edge to find is guessed at.
                    // In front of the label, not after it: the slot is a fixed width and
                    // a long choice is already truncated, so a marker on the end is one
                    // you cannot see. Shown on the canvas as well as in the inspector for
                    // the reason a condition's branch is — a flag you have to click every
                    // edge to find is a flag that gets guessed at.
                    (
                        format!("(once) {}", edge.label).trim_end().to_string(),
                        rgba(0x9a9aa6ff),
                    )
                } else {
                    (edge.label.clone(), rgba(0x9a9aa6ff))
                };
                let (from_node, to_node) = (self.doc.node(edge.from)?, self.doc.node(edge.to)?);
                // Skipped for the reason `edge_lines` skips it, and through the same test so
                // the two cannot drift.
                if from_node.flow != self.flow || to_node.flow != self.flow {
                    return None;
                }
                let from = out_port(from_node);
                let to = in_port(to_node);
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
                                .text_color(color)
                                .truncate()
                                .child(text),
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

        // An idle search box lists the bookmarks instead: every aliased node, the author's
        // own landmarks. Beneath the field rather than in a panel of their own, since "take
        // me somewhere" is the question both answer.
        let bookmarks = match query.trim() {
            "" => self.doc.bookmarks(),
            _ => Vec::new(),
        };
        let more_marks = bookmarks.len().saturating_sub(MAX_ROWS);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.search_input.clone())
            .when(!bookmarks.is_empty(), |this| {
                this.child(div().text_color(rgb(0x8a8a96)).child("Bookmarks"))
                    .children(bookmarks.iter().take(MAX_ROWS).filter_map(|&id| {
                        let node = self.doc.node(id)?;
                        Some(
                            div()
                                .id(("bookmark", id))
                                .flex()
                                .gap_2()
                                .px_1()
                                .rounded_sm()
                                .hover(|style| style.bg(rgb(0x33333c)))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(rgb(0xc3a6f7))
                                        .child(node.alias.trim().to_string()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(rgb(0x6c6c78))
                                        .child(node_label(&self.doc, node)),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.jump_to(id, window, cx)
                                })),
                        )
                    }))
                    .when(more_marks > 0, |this| {
                        this.child(
                            div()
                                .text_color(rgb(0x6c6c78))
                                .child(format!("…and {more_marks} more")),
                        )
                    })
            })
            .when(!query.trim().is_empty(), |this| {
                this.when(hits.is_empty(), |this| {
                    this.child(div().text_color(rgb(0x6c6c78)).child("No matches."))
                })
                .children(hits.iter().take(MAX_ROWS).filter_map(|&id| {
                    let label = node_label(&self.doc, self.doc.node(id)?);
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
            .bg(rgb(0x26262c))
            .border_r_1()
            .border_color(rgb(0x3a3a44))
            .text_color(rgb(0xd6d6dd))
            // Two tabs over one list container, styled like `kind_picker` and wrapping for
            // the same reason it does.
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        (
                            SidePane::Variables,
                            match playing {
                                Some(_) => "Variables (now)",
                                None => "Variables",
                            },
                            "pane-variables",
                        ),
                        (SidePane::Cast, "Cast", "pane-cast"),
                        (SidePane::Flows, "Flows", "pane-flows"),
                    ]
                    .map(|(pane, label, element_id)| {
                        let on = pane == self.pane;
                        div()
                            .id(element_id)
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(if on { rgb(0x3a3a44) } else { rgb(0x2b2b33) })
                            .text_color(if on { rgb(0xd6d6dd) } else { rgb(0x8a8a96) })
                            .hover(|style| style.text_color(rgb(0xd6d6dd)))
                            .child(label)
                            // A rename aimed at the list you just left would rename the wrong
                            // thing, so switching tabs puts the shared field back to declaring.
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pane = pane;
                                this.renaming = None;
                                this.set = String::new();
                                this.var_input.update(cx, |input, cx| {
                                    input.set_content("", cx);
                                    // The hint is how one field says which list its `+` adds to.
                                    input.set_placeholder(declare_hint(pane), cx);
                                });
                                cx.notify();
                            }))
                    }),
                ),
            )
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
                                                .text_color(rgb(0x8a8a96))
                                                .truncate()
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
            // The foot: the whole cast's dialogue on the Cast tab, the open canvas's on the
            // Flows tab — the one a click on a flow row has just switched to.
            .map(|this| {
                let (whose, count) = match pane {
                    SidePane::Variables => return this,
                    SidePane::Cast => ("Whole cast".to_string(), self.doc.stats(|_| true)),
                    SidePane::Flows => {
                        let flow = self.flow.as_str();
                        let whose = if flow.is_empty() { "(main)" } else { flow };
                        (format!("On {whose}"), self.doc.stats(|node| node.flow == flow))
                    }
                };
                let plural = |n: usize, one: &str| match n {
                    1 => format!("1 {one}"),
                    _ => format!("{n} {one}s"),
                };
                this.child(
                    div()
                        .pt_1()
                        .border_t_1()
                        .border_color(rgb(0x3a3a44))
                        .text_color(rgb(0x6c6c78))
                        // Two lines, since a flow name and both counts overflow the pane.
                        .child(div().truncate().child(whose))
                        .child(div().truncate().child(format!(
                            "{} · {}",
                            plural(count.lines, "line"),
                            plural(count.words, "word")
                        ))),
                )
            })
            .when(playing.is_none(), |this| {
                this.when_some(renaming.clone(), |this, old| {
                    let what = match old {
                        Rename::Set(name) => format!("Rename set \"{name}\""),
                        Rename::Item(name) => format!("Rename \"{name}\""),
                    };
                    this.child(div().text_color(rgb(0x8a8a96)).truncate().child(what))
                })
                // Only above the variables, and not while the field holds a set name.
                .when(pane == SidePane::Variables && !renaming_set, |this| {
                    this.child(self.set_picker(&set, &sets, cx))
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
                            .bg(rgb(0x2b2b33))
                            .text_color(rgb(0x8a8a96))
                            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                                        // The rename rewrites who speaks, never what is said,
                                        // so say how much prose still has the old name in it.
                                        let left = this
                                            .doc
                                            .mentions(&Ref::Character(old.clone()))
                                            .len();
                                        let still = match left {
                                            0 => String::new(),
                                            1 => format!(" 1 line still says \"{old}\"."),
                                            n => format!(" {n} lines still say \"{old}\"."),
                                        };
                                        this.set_status(
                                            format!(
                                                "Renamed \"{old}\" to \"{name}\" — {moved} line{s}.{still}"
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
                                    // A colour off the palette, so a new cast comes out tinted
                                    // apart without anybody picking one.
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
                                .text_color(rgb(0x6c6c78))
                                .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
            .text_color(rgb(0xf7c96a))
            .hover(|style| style.bg(rgb(0x33333c)))
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
                    .text_color(rgb(0x8a8a96))
                    .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                    .bg(if on { rgb(0x3a3a44) } else { rgb(0x2b2b33) })
                    .text_color(if on { rgb(0xd6d6dd) } else { rgb(0x8a8a96) })
                    .hover(|style| style.text_color(rgb(0xd6d6dd)))
                    .truncate()
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
            .text_color(rgb(0x6c6c78))
            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
        let target = Ref::Variable(variable.name.clone());
        let used_by = self.used_by_toggle(("var-refs", i), &target, cx);
        let open = (self.used_by.as_ref() == Some(&target)).then(|| self.used_by_list(&target, cx));

        let row = div()
            .flex()
            .gap_2()
            .items_center()
            .child(match shown {
                // A flag is its own control: the box shows the state and clicking it flips.
                Value::Bool(on) => div()
                    .id(("var-toggle", i))
                    .flex()
                    .gap_2()
                    .flex_grow()
                    .truncate()
                    .text_color(if on { rgb(0xf7c96a) } else { rgb(0xb8b8c4) })
                    .child(if on { "☑" } else { "☐" })
                    .child(label)
                    .when(playing.is_none(), |this| {
                        this.cursor_pointer()
                            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                    .flex_grow()
                    .truncate()
                    .items_center()
                    .text_color(rgb(0xb8b8c4))
                    .child(div().flex_grow().truncate().child(label))
                    .child(if editing {
                        self.value_editor(cx).into_any_element()
                    } else if playing.is_some() {
                        // Read-only while playing, like everything else in the pane.
                        div()
                            .px_1()
                            .text_color(rgb(0xf7c96a))
                            .child(n.to_string())
                            .into_any_element()
                    } else {
                        self.number_cell(("var-value", i), n, ValueTarget::Default(name), cx)
                            .into_any_element()
                    })
                    .into_any_element(),
            })
            .when(!editing, |this| this.child(used_by))
            .when(playing.is_none() && !editing, |this| {
                this.child(
                    // The one place a variable's type changes. Lit when it holds a number,
                    // since a row showing 0 says nothing about the type on its own.
                    div()
                        .id(("var-type", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(if is_int { rgb(0xf7c96a) } else { rgb(0x6c6c78) })
                        .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xff7a7a)))
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
            });
        div().flex().flex_col().child(row).children(open)
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
        let main = flow.is_empty();
        let target = Ref::Flow(flow.clone());
        let used_by = self.used_by_toggle(("flow-refs", i), &target, cx);
        let open = (self.used_by.as_ref() == Some(&target)).then(|| self.used_by_list(&target, cx));
        let selected = self.selection.nodes().len();
        let (to_show, to_rename, to_remove, to_move) =
            (flow.clone(), flow.clone(), flow.clone(), flow.clone());

        let row = div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id(("flow-name", i))
                    .flex_grow()
                    .truncate()
                    .cursor_pointer()
                    .text_color(if on { rgb(0xd6d6dd) } else { rgb(0xb8b8c4) })
                    .hover(|style| style.text_color(rgb(0xd6d6dd)))
                    .child(if main { "(main)".to_string() } else { flow })
                    // Only the view moves, so no checkpoint: which canvas you are looking
                    // at is not a document edit. The selection goes, though — nodes on the
                    // canvas you left are neither drawn nor hit-testable, so leaving them
                    // selected would let the inspector edit an invisible node and `Tab`
                    // branch off one, which is the only way to author a `CrossFlowEdge`.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.flow != to_show {
                            this.flow.clone_from(&to_show);
                            this.select(Selection::None, cx);
                        }
                        cx.notify();
                    })),
            )
            // How many nodes stand on it, which opens the list of them — the canvas itself
            // shows them too, but not while another one is on screen.
            .child(used_by)
            .when(!playing && selected > 0 && !on, |this| {
                this.child(
                    div()
                        .id(("flow-move", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xd6d6dd)))
                        .child("→")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let ids = this.selection.nodes().to_vec();
                            let count = ids.len();
                            this.doc.checkpoint();
                            this.doc.move_to_flow(&ids, &to_move);
                            let s = if count == 1 { "" } else { "s" };
                            // The nodes just left this canvas, so nothing is selected on it.
                            this.select(Selection::None, cx);
                            this.set_status(format!("Moved {count} node{s}."), cx);
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
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xff7a7a)))
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
            });
        div().flex().flex_col().child(row).children(open)
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
        let target = Ref::Character(name.clone());
        let used_by = self.used_by_toggle(("cast-refs", i), &target, cx);
        let open = (self.used_by.as_ref() == Some(&target)).then(|| {
            div()
                .flex()
                .flex_col()
                .child(self.used_by_list(&target, cx))
                .children(self.mentions_list(&target, cx))
        });
        let to_recolor = name.clone();
        let to_rename = name.clone();
        let to_remove = name.clone();

        let row = div()
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
                    .flex_grow()
                    .truncate()
                    .text_color(rgb(0xb8b8c4))
                    .child(name),
            )
            // How many lines they speak — the number casting asks first, and the flow row's
            // node count is its sibling — which opens the list of those lines. Words go at
            // the foot of the tab, where there is width for them.
            .child(used_by)
            .when(!playing, |this| {
                this.child(
                    div()
                        .id(("cast-rename", i))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                        .text_color(rgb(0x6c6c78))
                        .hover(|style| style.text_color(rgb(0xff7a7a)))
                        .child("×")
                        // Undeclaring only: the nodes keep saying the name, and the problem
                        // list is what points out that nobody declared it.
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.doc.checkpoint();
                            this.doc.remove_character(&to_remove);
                            cx.notify();
                        })),
                )
            });
        div().flex().flex_col().child(row).children(open)
    }

    /// The count that opens a row's "Used by" list: how many nodes refer to the name, lit
    /// while its list is open. Clicking it is view state, never an edit, so it stays live
    /// during play — looking up where a flag is set is exactly what a play-through prompts.
    fn used_by_toggle(
        &self,
        id: (&'static str, usize),
        target: &Ref,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut nodes: Vec<NodeId> = self
            .doc
            .references(target)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        nodes.dedup();
        // A cast row counts lines, the number the Cast foot adds up; a speaker on a blank
        // node still shows up in the list, since that node does name them.
        let count = match target {
            Ref::Character(name) => self.doc.stats(|node| node.speaker == *name).lines,
            _ => nodes.len(),
        };
        let open = self.used_by.as_ref() == Some(target);
        let target = target.clone();
        div()
            .id(id)
            .flex_none()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(if open { rgb(0x7aa2f7) } else { rgb(0x6c6c78) })
            .hover(|style| style.text_color(rgb(0xd6d6dd)))
            .child(count.to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.used_by = match this.used_by.take() {
                    Some(open) if open == target => None,
                    _ => Some(target.clone()),
                };
                cx.notify();
            }))
    }

    /// Every node that refers to `target`, one clickable row each, jumping through
    /// `jump_to` — so a row on another canvas crosses to it with no arm of its own. Shared
    /// by the left pane's rows and the inspector's "Linked from", which differ only in
    /// what they ask [`Doc::references`] about.
    fn used_by_list(&self, target: &Ref, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        /// Past this the list stops being scannable in a pane this narrow.
        const MAX_ROWS: usize = 12;

        let found = self.doc.references(target);
        let more = found.len().saturating_sub(MAX_ROWS);
        let rows: Vec<_> = found
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .filter_map(|(row, (id, how))| {
                let node = self.doc.node(id)?;
                // Under a variable the row is already about that variable, so it says what
                // the node does to it — `sets = true` — rather than repeating the name.
                let what = match (target, how) {
                    (Ref::Variable(var), Use::Writes) => self.doc.effect_of(id, var).map(|e| {
                        format!(
                            "{} sets {} {}",
                            node_number(&self.doc, id),
                            e.op.symbol(),
                            e.value
                        )
                    }),
                    (Ref::Variable(var), Use::Reads) => self.doc.condition_of(id, var).map(|c| {
                        format!(
                            "{} checks {} {}",
                            node_number(&self.doc, id),
                            c.op.symbol(),
                            c.want
                        )
                    }),
                    // Under a character, likewise: the line, not who says it.
                    (Ref::Character(_), _) => Some(match node.text.trim() {
                        "" => node_number(&self.doc, id),
                        text => text.to_string(),
                    }),
                    _ => None,
                };
                let verb = if how == Use::LeadsHere && node.kind == NodeKind::Jump {
                    "↪ "
                } else {
                    ""
                };
                let elsewhere = match node.flow.as_str() {
                    flow if flow == self.flow => String::new(),
                    "" => " · (main)".to_string(),
                    flow => format!(" · {flow}"),
                };
                let what = what.unwrap_or_else(|| format!("{verb}{}", ref_label(&self.doc, node)));
                let label = format!("{what}{elsewhere}");
                Some(
                    div()
                        .id(("used-by", row))
                        .px_1()
                        .rounded_sm()
                        .truncate()
                        .cursor_pointer()
                        .text_color(rgb(0x9a9aa6))
                        .hover(|style| style.bg(rgb(0x33333c)).text_color(rgb(0xd6d6dd)))
                        .child(label)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.jump_to(id, window, cx)),
                        ),
                )
            })
            .collect();
        let empty = rows.is_empty();

        div()
            .flex()
            .flex_col()
            .ml_3()
            .mb_1()
            .pl_1()
            .border_l_1()
            .border_color(rgb(0x3a3a44))
            .when(empty, |this| {
                this.child(div().text_color(rgb(0x6c6c78)).child(match target {
                    Ref::Node(_) => "Nothing leads here — a start.",
                    _ => "Used nowhere.",
                }))
            })
            .children(rows)
            .when(more > 0, |this| {
                this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child(format!("…and {more} more")),
                )
            })
    }

    /// "Mentioned in": the prose that names `target` without a link to it, from
    /// [`Doc::mentions`] — `None` when there is none, so a heading never sits over an empty
    /// list. Each row shows the text around the name, since the name is the point and a
    /// line truncated from its start could hide it. It never offers to fix a row: clicking
    /// goes to it, and the author edits the line.
    fn mentions_list(
        &self,
        target: &Ref,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        const MAX_ROWS: usize = 12;

        let name = match target {
            Ref::Character(name) => name.clone(),
            Ref::Node(id) => self.doc.node(*id)?.alias.trim().to_string(),
            _ => return None,
        };
        let found = self.doc.mentions(target);
        if found.is_empty() {
            return None;
        }
        let more = found.len().saturating_sub(MAX_ROWS);
        let rows: Vec<_> = found
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .filter_map(|(row, at)| {
                let (text, node) = match at {
                    Mention::Node(id) => (self.doc.node(id)?.text.as_str(), id),
                    Mention::Edge(index) => {
                        let edge = self.doc.edges.get(index)?;
                        (edge.label.as_str(), edge.from)
                    }
                };
                let quote = snippet(text, &name);
                let label = match at {
                    Mention::Node(_) => quote,
                    Mention::Edge(_) => format!("choice · {quote}"),
                };
                Some(
                    div()
                        .id(("mention", row))
                        .px_1()
                        .rounded_sm()
                        .truncate()
                        .cursor_pointer()
                        .text_color(rgb(0x9a9aa6))
                        .hover(|style| style.bg(rgb(0x33333c)).text_color(rgb(0xd6d6dd)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| match at {
                            Mention::Node(id) => this.jump_to(id, window, cx),
                            // A choice is found on the canvas at the node it leaves, and
                            // selected as itself so the Choice field is right there.
                            Mention::Edge(index) => {
                                this.show(node, window);
                                this.select(Selection::Edge(index), cx);
                                cx.notify();
                            }
                        })),
                )
            })
            .collect();

        Some(
            div()
                .flex()
                .flex_col()
                .child(div().text_color(rgb(0x8a8a96)).child("Mentioned in"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .ml_3()
                        .mb_1()
                        .pl_1()
                        .border_l_1()
                        .border_color(rgb(0x3a3a44))
                        .children(rows)
                        .when(more > 0, |this| {
                            this.child(
                                div()
                                    .text_color(rgb(0x6c6c78))
                                    .child(format!("…and {more} more")),
                            )
                        }),
                ),
        )
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

                // The walk only stops on a node the player sees, unless it ran out of graph
                // — a condition with that pin unwired, or a jump pointing at nothing.
                if !self.doc.pauses_play(current) {
                    return this.child(
                        div()
                            .text_color(rgb(0xff7a7a))
                            .child("The flow stops here — nothing leads on."),
                    );
                }

                let speaker = node.speaker.trim();
                let text = node.text.trim();
                this.when(!speaker.is_empty(), |this| {
                    this.child(
                        div()
                            .text_color(speaker_color(&self.doc, speaker, 0x8a8a96))
                            .child(speaker.to_string()),
                    )
                })
                .child(if text.is_empty() {
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("(no line written yet)")
                } else {
                    div().child(text.to_string())
                })
            })
            // What the instructions walked through on the way here did. Without this the
            // only evidence is the variables panel changing in the corner of your eye.
            .children(
                self.effects_fired(path)
                    .into_iter()
                    .map(|(var, op, value)| {
                        div().text_color(rgb(0x8ce99a)).child(format!(
                            "{var} {} {}",
                            op.symbol(),
                            value
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
                                .text_color(rgb(0x6c6c78))
                                .child(format!("→ {label} (already taken)"))
                                .into_any_element();
                        }
                        row(("play-choice", i), format!("→ {label}"), 0xd6d6dd)
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
                |this| {
                    this.child(
                        div()
                            .text_color(rgb(0x6c6c78))
                            .child("End of conversation."),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    // Offered only when there is an earlier node the player actually saw:
                    // the invisible run walked through to get here is not a step back.
                    .when(self.doc.step_back(&mut path.to_vec()), |this| {
                        this.child(row(("play-nav", 0), "← Back".into(), 0x8a8a96).on_click(
                            cx.listener(|this, _, window, cx| {
                                let Some(mut path) = this.play.take() else {
                                    return;
                                };
                                this.doc.step_back(&mut path);
                                this.follow(&path, window);
                                this.play = Some(path);
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        row(("play-nav", 1), "Restart".into(), 0x8a8a96).on_click(cx.listener(
                            |this, _, window, cx| {
                                let Some(path) = this.play.as_ref() else {
                                    return;
                                };
                                // Re-walked rather than truncated, so a conversation that
                                // opens on an instruction runs it again.
                                let seed = path[0];
                                this.start_play(seed, window, cx);
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

    /// What the node is. Switching to a note drops every edge touching it and switching to
    /// a jump drops every outgoing edge but the first,
    /// keeping the one-target invariant `Doc::add_edge` enforces — otherwise the extra
    /// edges would stop being drawn and the canvas would quietly lie about the graph. It
    /// is checkpointed like any other edit, so an undo brings them back.
    fn kind_picker(&self, id: NodeId, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.doc.node(id).map_or(NodeKind::Speech, |node| node.kind);

        // Wraps: five labels do not fit across `SIDEBAR_W`, and without this the last kind
        // is simply clipped off the right edge — present but unreachable.
        div().flex().flex_wrap().gap_1().children(
            [
                (NodeKind::Speech, "Speech", "kind-speech"),
                (NodeKind::Hub, "Hub", "kind-hub"),
                (NodeKind::Jump, "Jump", "kind-jump"),
                (NodeKind::Instruction, "Instruction", "kind-instruction"),
                (NodeKind::Condition, "Condition", "kind-condition"),
                (NodeKind::Note, "Note", "kind-note"),
            ]
            .map(|(kind, label, element_id)| {
                let on = kind == current;
                div()
                    .id(element_id)
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if on { rgb(0x3a3a44) } else { rgb(0x2b2b33) })
                    .text_color(if on { rgb(0xd6d6dd) } else { rgb(0x8a8a96) })
                    .hover(|style| style.text_color(rgb(0xd6d6dd)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.doc.node(id).is_none_or(|node| node.kind == kind) {
                            return;
                        }
                        this.doc.checkpoint();
                        if let Some(node) = this.doc.node_mut(id) {
                            node.kind = kind;
                        }
                        // A note holds no edges at either end, so converting to one drops
                        // every edge that touched it — the same lie an undrawn edge out of
                        // a jump would be, and one undo brings them all back.
                        if kind == NodeKind::Note {
                            this.doc.edges.retain(|e| e.from != id && e.to != id);
                        }
                        if kind == NodeKind::Jump {
                            let mut kept = false;
                            this.doc
                                .edges
                                .retain(|e| e.from != id || !std::mem::replace(&mut kept, true));
                        }
                        // Effects and checks are their kind's content, not a property every
                        // node carries. Leaving them on a node that no longer runs them
                        // would be the same lie as an undrawn edge out of a jump.
                        if let Some(node) = this.doc.node_mut(id) {
                            if kind != NodeKind::Instruction {
                                node.effects.clear();
                            }
                            if kind != NodeKind::Condition {
                                node.conditions.clear();
                            }
                        }
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
        // ponytail: capped like the sibling panels. Past this the sidebar needs to scroll,
        // which is the actual fix rather than a bigger cap.
        const MAX_ROWS: usize = 8;

        let more = self.doc.variables.len().saturating_sub(MAX_ROWS);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(0x8a8a96)).child(match mode {
                VarRow::Sets => "Sets",
                VarRow::Checks => "Checks",
            }))
            .when(self.doc.variables.is_empty(), |this| {
                this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("Declare one in the Variables pane first."),
                )
            })
            .children(
                self.doc
                    .variables
                    .iter()
                    .take(MAX_ROWS)
                    .enumerate()
                    .map(|(i, variable)| {
                        let name = variable.name.clone();
                        let int = matches!(variable.default, Value::Int(_));
                        // What the node holds now: the value, and the operator glyph that
                        // goes in front of it.
                        let current: Option<(Value, &'static str)> = match mode {
                            VarRow::Sets => self
                                .doc
                                .effect_of(id, &variable.name)
                                .map(|e| (e.value, e.op.symbol())),
                            VarRow::Checks => self
                                .doc
                                .condition_of(id, &variable.name)
                                .map(|c| (c.want, c.op.symbol())),
                        };
                        // A flag is a checkbox; a number has no box to tick, so it shows the
                        // operator there instead and the value beside the name.
                        let (glyph, color) = match current {
                            None => ("—", rgb(0x6c6c78)),
                            Some((Value::Bool(true), _)) => ("☑", rgb(0x8ce99a)),
                            Some((Value::Bool(false), _)) => ("☐", rgb(0xf7c96a)),
                            Some((Value::Int(_), op)) => (op, rgb(0x8ce99a)),
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
                                    .flex_grow()
                                    .truncate()
                                    .cursor_pointer()
                                    .text_color(color)
                                    .hover(|style| style.bg(rgb(0x33333c)))
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
                    }),
            )
            .when(more > 0, |this| {
                this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child(format!("…and {more} more")),
                )
            })
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
            .text_color(if on { TRUE_COLOR } else { FALSE_COLOR })
            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
            .text_color(rgb(if on { 0xd6d6dd } else { 0x8a8a96 }))
            .hover(|style| style.text_color(rgb(0xd6d6dd)))
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
                    (Some(id), _) => {
                        // Each kind shows the fields it actually has: a jump has none of
                        // its own, and a hub has no speaker because nobody speaks at one.
                        let kind = self.doc.node(id).map_or(NodeKind::Speech, |node| node.kind);
                        this.child(
                            div()
                                .text_color(rgb(0x9ecbff))
                                .child(node_number(&self.doc, id)),
                        )
                        .child(self.kind_picker(id, cx))
                        // Outside every kind guard on purpose: an alias is orthogonal to
                        // `NodeKind` the way `end` is, and a game may want to enter a
                        // conversation at a hub or a condition too.
                        .child(field("Alias", &self.alias_input))
                        .when(kind == NodeKind::Speech, |this| {
                            this.child(field("Speaker", &self.speaker_input))
                        })
                        .when(
                            matches!(kind, NodeKind::Speech | NodeKind::Hub | NodeKind::Note),
                            |this| {
                                let label = match kind {
                                    NodeKind::Hub => "Label",
                                    NodeKind::Note => "Note",
                                    _ => "Text",
                                };
                                this.child(field(label, &self.text_input))
                            },
                        )
                        .when(kind == NodeKind::Instruction, |this| {
                            this.child(self.var_rows(id, VarRow::Sets, cx))
                        })
                        .when(kind == NodeKind::Condition, |this| {
                            this.child(self.var_rows(id, VarRow::Checks, cx))
                        })
                        .when(kind == NodeKind::Speech, |this| {
                            this.child(self.end_toggle(id, cx))
                        })
                        // A note takes no edges, so nothing can ever lead to one.
                        .when(kind != NodeKind::Note, |this| {
                            this.child(div().text_color(rgb(0x8a8a96)).child("Linked from"))
                                .child(self.used_by_list(&Ref::Node(id), cx))
                        })
                        // Only an aliased node has a name prose could use.
                        .children(self.mentions_list(&Ref::Node(id), cx))
                    }
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
                        let index = *index;
                        let edge = self.doc.edges.get(index);
                        let title = match edge {
                            Some(edge) => format!(
                                "{} → {}",
                                node_number(&self.doc, edge.from),
                                node_number(&self.doc, edge.to)
                            ),
                            None => "Edge".into(),
                        };
                        // Out of a condition this edge is a pin, not a choice: the player
                        // never picks it, so a label would be text nobody ever reads.
                        let from_condition =
                            edge.is_some_and(|edge| self.doc.is_condition(edge.from));
                        this.child(div().text_color(rgb(0x9ecbff)).child(title))
                            .when(from_condition, |this| {
                                this.child(self.branch_toggle(index, cx))
                            })
                            .when(!from_condition, |this| {
                                this.child(field("Choice", &self.label_input))
                            })
                            // Only where the player is the one choosing. Out of an
                            // instruction or a jump the walk has to take the edge it finds,
                            // so a once-only flag there would be a promise nothing keeps.
                            .when(
                                edge.is_some_and(|edge| self.doc.pauses_play(edge.from)),
                                |this| this.child(self.once_toggle(index, cx)),
                            )
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

/// How a node reads in a "Used by" row: what it says, or, for the logic kinds that say
/// nothing, what they do — a list of eight rows reading "Node 7" is no list at all.
fn ref_label(doc: &Doc, node: &Node) -> String {
    let joined = |parts: Vec<String>, sep: &str| parts.join(sep);
    match node.kind {
        NodeKind::Instruction if !node.effects.is_empty() => joined(
            node.effects
                .iter()
                .map(|e| format!("{} {} {}", e.var, e.op.symbol(), e.value))
                .collect(),
            ", ",
        ),
        NodeKind::Condition if !node.conditions.is_empty() => joined(
            node.conditions
                .iter()
                .map(|c| format!("{} {} {}", c.var, c.op.symbol(), c.want))
                .collect(),
            " and ",
        ),
        // A jump says nothing and does nothing on its own, so it is named by what leads into
        // it: "↪ from Two coppers a night" is which branch returns here.
        NodeKind::Jump => match doc.references(&Ref::Node(node.id)).first() {
            Some(&(from, _)) => match doc.node(from) {
                // One level only: a ring of jumps leading into each other would otherwise
                // name itself forever.
                Some(from) if from.kind == NodeKind::Jump => {
                    format!("from {}", node_number(doc, from.id))
                }
                Some(from) => format!("from {}", ref_label(doc, from)),
                None => node_number(doc, node.id),
            },
            None => node_number(doc, node.id),
        },
        _ => node_label(doc, node),
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
        .bg(rgb(0x3a3a44))
        .border_2()
        .border_color(rgb(0x7d7d8c))
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

/// What hovering a jump shows of where it lands: who speaks there, the first line of what
/// they say, and which canvas it is on — the three things the box's one-line `↪ target`
/// has no room for, and the last of which is the reason to look before following it.
#[derive(Clone)]
struct JumpPreview {
    speaker: Option<(String, Rgba)>,
    line: String,
    flow: String,
}

impl JumpPreview {
    fn of(doc: &Doc, target: &Node) -> JumpPreview {
        let speaker = match (target.kind, target.speaker.trim()) {
            (NodeKind::Speech, name) if !name.is_empty() => {
                Some((name.to_string(), speaker_color(doc, name, 0x9ecbff)))
            }
            _ => None,
        };
        let line = match target.text.lines().map(str::trim).find(|l| !l.is_empty()) {
            Some(line) => line.to_string(),
            None => node_label(doc, target),
        };
        let flow = match target.flow.as_str() {
            "" => "(main)".to_string(),
            flow => flow.to_string(),
        };
        JumpPreview {
            speaker,
            line,
            flow: format!("on {flow}"),
        }
    }
}

impl Render for JumpPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(0x2b2b33))
            .border_1()
            .border_color(rgb(0xc3a6f7))
            .rounded_md()
            .shadow_md()
            .text_sm()
            .when_some(self.speaker.clone(), |this, (name, color)| {
                this.child(div().text_color(color).child(name))
            })
            .child(
                div()
                    .text_color(rgb(0xd6d6dd))
                    .line_clamp(3)
                    .child(self.line.clone()),
            )
            .child(div().text_color(rgb(0x6c6c78)).child(self.flow.clone()))
    }
}

/// `jump_target` is the label of the node a jump points at, `preview` what hovering it
/// shows, and `speaker` the colour of its
/// cast member, all resolved by the caller.
///
/// Every kind keeps the same `NODE_W`×`NODE_H` footprint on purpose: hit-testing is
/// arithmetic over those constants in `doc.rs`, so a box drawn smaller than it is clicked
/// would put the two out of step. Kinds differ in what they draw, never in how big.
fn node_box(
    node: &Node,
    view: View,
    selected: bool,
    playing: bool,
    jump_target: Option<String>,
    preview: Option<JumpPreview>,
    speaker: Rgba,
) -> impl IntoElement {
    let (x, y) = view.to_screen((node.x, node.y));
    div()
        .id(("node", node.id))
        // View-only, like everything a hover does: it never selects and never switches the
        // canvas, so a glance at where a jump lands costs nothing.
        .when_some(preview, |this, preview| {
            this.tooltip(move |_, cx| cx.new(|_| preview.clone()).into())
        })
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
        .map(|this| match node.kind {
            NodeKind::Speech => this
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .text_color(speaker)
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
                ),
            // A branch point says nothing, so its `text` is a label for it rather than a
            // line — an unnamed hub in a graph of hubs is not worth much.
            NodeKind::Hub => this.justify_center().items_center().child(
                div()
                    .text_color(rgb(0x9a9aa6))
                    .truncate()
                    .child(match node.text.trim() {
                        "" => "◇".to_string(),
                        label => format!("◇ {label}"),
                    }),
            ),
            NodeKind::Jump => this.justify_center().child(
                div()
                    .w_full()
                    .text_color(rgb(0xc3a6f7))
                    .truncate()
                    .child(match jump_target {
                        Some(target) => format!("↪ {target}"),
                        None => "↪ nowhere".into(),
                    }),
            ),
            // The writes as they read on the page, which is the whole point of logic being
            // a node: `⚙ paid = true` is legible without opening anything.
            NodeKind::Instruction => this
                .justify_center()
                .child(div().flex_none().text_color(rgb(0x8ce99a)).child("⚙"))
                .child(
                    div()
                        .w_full()
                        .text_color(rgb(0xd6d6dd))
                        .text_ellipsis()
                        .line_clamp(2)
                        .child(match node.effects.as_slice() {
                            [] => "(sets nothing)".to_string(),
                            effects => effects
                                .iter()
                                .map(|e| format!("{} {} {}", e.var, e.op.symbol(), e.value))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }),
                ),
            // The checks as articy writes them — `? paid == true` — with no parser behind
            // it; the inspector edits them with the same picker an instruction uses.
            NodeKind::Condition => this
                .justify_center()
                .child(div().flex_none().text_color(rgb(0xf7c96a)).child("?"))
                .child(
                    div()
                        .w_full()
                        .text_color(rgb(0xd6d6dd))
                        .text_ellipsis()
                        .line_clamp(2)
                        .child(match node.conditions.as_slice() {
                            [] => "(checks nothing)".to_string(),
                            conditions => conditions
                                .iter()
                                .map(|c| format!("{} {} {}", c.var, c.op.symbol(), c.want))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }),
                ),
            // Its own font, so a note never reads as a line somebody says: italic, muted,
            // and no speaker line to share the box with. Same footprint as every other
            // kind — hit-testing is arithmetic over `NODE_W`/`NODE_H`.
            NodeKind::Note => this
                .child(div().flex_none().text_color(rgb(0xf7c96a)).child("▤"))
                .child(
                    div()
                        .w_full()
                        // Its own face, not just its own colour: the bundled UI font has
                        // no italic, so `.italic()` alone renders identically to a line of
                        // dialogue. A serif is the one thing on the canvas that is not.
                        .font_family("Noto Serif")
                        .italic()
                        .text_color(rgb(0x9a9aa6))
                        .text_ellipsis()
                        .line_clamp(4)
                        .child(node.text.clone()),
                ),
        })
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
