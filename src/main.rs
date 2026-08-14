mod doc;
mod text_input;

use doc::{Doc, FILE_EXT, NODE_H, NODE_W, Node, PORT_R, in_port, out_port, with_default_extension};
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder, PathPromptOptions, Pixels,
    Point, PromptLevel, Render, Rgba, Window, WindowBounds, WindowOptions, actions, canvas,
    deferred, div, point, prelude::*, px, rgb, size,
};
use std::path::PathBuf;
use text_input::TextInput;

actions!(
    papugai,
    [DeleteNode, Undo, FocusCanvas, New, Save, SaveAs, Open]
);

/// Height of the menu bar. Mouse positions are window-relative, so this is also the
/// offset between window coordinates and the canvas pane below the bar.
const MENU_H: f32 = 30.0;

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
            ("Delete selection", "Del", |e, w, cx| {
                e.on_delete(&DeleteNode, w, cx)
            }),
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

struct Editor {
    doc: Doc,
    /// Canvas offset in pixels, applied to everything drawn.
    pan: (f32, f32),
    selected: Option<Selection>,
    drag: Option<Drag>,
    focus_handle: FocusHandle,
    speaker_input: Entity<TextInput>,
    text_input: Entity<TextInput>,
    label_input: Entity<TextInput>,
    /// What is currently loaded into the sidebar fields.
    edited: Option<Selection>,
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
}

/// Edge indices shift when edges are removed, so an edge selection is cleared
/// on every structural change.
#[derive(Clone, Copy, PartialEq)]
enum Selection {
    Node(u32),
    Edge(usize),
}

#[derive(Clone, Copy)]
enum Drag {
    /// `grab` is the cursor's offset inside the node, in document coordinates.
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

        Self {
            doc: Doc::sample(),
            pan: (0.0, 0.0),
            selected: None,
            drag: None,
            focus_handle,
            speaker_input,
            text_input,
            label_input,
            edited: None,
            text_dirty: false,
            path: None,
            status: None,
            title: String::new(),
            open_menu: None,
        }
    }

    fn file_name(&self) -> String {
        match &self.path {
            Some(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            None => format!("untitled.{FILE_EXT}"),
        }
    }

    fn on_new(&mut self, _: &New, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc.is_dirty() {
            return self.reset(cx);
        }

        // Unsaved edits would be lost, so confirm first.
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some(&format!("{} has unsaved edits.", self.file_name())),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.reset(cx)).ok();
            }
        })
        .detach();
    }

    /// Starts an empty, unnamed document.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.doc = Doc::default();
        self.drag = None;
        self.path = None;
        self.status = None;
        self.select(None, cx);
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

    fn on_open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
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
                Ok(doc) => {
                    this.doc = doc;
                    this.drag = None;
                    this.path = Some(path);
                    this.status = None;
                    this.select(None, cx);
                    cx.notify();
                }
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
    fn load_fields(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        let (speaker, text, label) = match selection {
            Some(Selection::Node(id)) => match self.doc.node(id) {
                Some(node) => (node.speaker.clone(), node.text.clone(), String::new()),
                None => Default::default(),
            },
            Some(Selection::Edge(index)) => match self.doc.edges.get(index) {
                Some(edge) => (String::new(), String::new(), edge.label.clone()),
                None => Default::default(),
            },
            None => Default::default(),
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
        match self.edited {
            Some(Selection::Node(id)) => {
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
            Some(Selection::Edge(index)) => {
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
            None => return,
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
        let (x, y) = Self::window_pos(pos);
        (x - self.pan.0, y - self.pan.1)
    }

    /// Screen position of a document point.
    fn screen_pos(&self, (x, y): (f32, f32)) -> Point<Pixels> {
        point(px(x + self.pan.0), px(y + self.pan.1))
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
                    self.selected == Some(Selection::Edge(index)),
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

    fn select(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        self.selected = selection;
        if self.edited != selection {
            self.load_fields(selection, cx);
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
            let node = self.doc.node(id).expect("hit_test returned a live node");
            let grab = (x - node.x, y - node.y);
            self.select(Some(Selection::Node(id)), cx);
            self.drag = Some(Drag::Node {
                id,
                grab,
                moved: false,
            });
            if ev.click_count == 2 {
                window.focus(&self.text_input.read(cx).focus_handle(cx));
            }
        } else if let Some(index) = self.doc.edge_at(x, y) {
            self.select(Some(Selection::Edge(index)), cx);
            self.drag = None;
        } else if ev.click_count == 2 {
            self.doc.checkpoint();
            let id = self.doc.add_node(x, y);
            self.select(Some(Selection::Node(id)), cx);
            self.drag = None;
            window.focus(&self.speaker_input.read(cx).focus_handle(cx));
        } else {
            self.select(None, cx);
            self.drag = Some(Drag::Pan {
                start_mouse: Self::window_pos(ev.position),
                start_pan: self.pan,
            });
        }
        cx.notify();
    }

    /// Completes an edge drag when released over a node; every other drag just ends.
    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let drag = self.drag.take();
        let Some(Drag::Edge { from, .. }) = drag else {
            return;
        };

        let (x, y) = self.doc_pos(ev.position);
        let Some(to) = self.doc.hit_test(x, y) else {
            cx.notify();
            return;
        };
        if from != to && !self.doc.has_edge(from, to) {
            self.doc.checkpoint();
            self.doc.add_edge(from, to);
            self.select(Some(Selection::Edge(self.doc.edges.len() - 1)), cx);
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
                let (x, y) = self.doc_pos(ev.position);
                if let Some(node) = self.doc.node_mut(id) {
                    node.x = x - grab.0;
                    node.y = y - grab.1;
                }
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
                let (x, y) = Self::window_pos(ev.position);
                self.pan = (
                    start_pan.0 + x - start_mouse.0,
                    start_pan.1 + y - start_mouse.1,
                );
            }
        }
        cx.notify();
    }

    fn on_delete(&mut self, _: &DeleteNode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.selected else {
            return;
        };
        self.doc.checkpoint();
        match selection {
            Selection::Node(id) => self.doc.remove_node(id),
            Selection::Edge(index) => self.doc.remove_edge(index),
        }
        self.select(None, cx);
        cx.notify();
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.undo() {
            // Undo can restore or drop whatever was selected, so the fields always reload.
            let selection = match self.selected {
                Some(Selection::Node(id)) if self.doc.node(id).is_some() => {
                    Some(Selection::Node(id))
                }
                _ => None,
            };
            self.selected = selection;
            self.load_fields(selection, cx);
            cx.notify();
        }
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
        let (pan, selected) = (self.pan, self.selected);

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
                    .child(self.canvas_pane(lines, pending, pan, selected, cx))
                    .child(self.sidebar(selected)),
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
        pan: (f32, f32),
        selected: Option<Selection>,
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
            .on_action(cx.listener(Self::on_focus_canvas))
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_open))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
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
                            if let Some(head) = arrow_head(to, from) {
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
            .children(
                self.doc
                    .nodes
                    .iter()
                    .map(|node| node_box(node, pan, selected == Some(Selection::Node(node.id)))),
            )
            .children(self.doc.nodes.iter().map(|node| port_dot(node, pan)))
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
                Some(
                    // Fixed width and centred content, so the label centres without measuring it.
                    div()
                        .absolute()
                        .left(mid.x - px(W / 2.0))
                        .top(mid.y - px(18.))
                        .w(px(W))
                        .flex()
                        .justify_center()
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

    /// Inspector for the selected node or edge.
    fn sidebar(&self, selected: Option<Selection>) -> impl IntoElement {
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
            .w(px(280.))
            .h_full()
            .p_3()
            .bg(rgb(0x26262c))
            .border_l_1()
            .border_color(rgb(0x3a3a44))
            .text_color(rgb(0xd6d6dd))
            .map(|this| match selected {
                Some(Selection::Node(id)) => this
                    .child(div().text_color(rgb(0x9ecbff)).child(format!("Node {id}")))
                    .child(field("Speaker", &self.speaker_input))
                    .child(field("Text", &self.text_input)),
                Some(Selection::Edge(index)) => {
                    let title = match self.doc.edges.get(index) {
                        Some(edge) => format!("Node {} → Node {}", edge.from, edge.to),
                        None => "Edge".into(),
                    };
                    this.child(div().text_color(rgb(0x9ecbff)).child(title))
                        .child(field("Choice", &self.label_input))
                }
                None => this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("Select a node or an edge to edit it."),
                ),
            })
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_color(rgb(0x6c6c78))
                    .when_some(self.status.clone(), |this, status| this.child(status))
                    .when(selected.is_some(), |this| {
                        this.child("Esc — back to canvas")
                    })
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

/// The handle an edge is dragged out of.
fn port_dot(node: &Node, pan: (f32, f32)) -> impl IntoElement {
    let (x, y) = out_port(node);
    div()
        .absolute()
        .left(px(x + pan.0 - PORT_R))
        .top(px(y + pan.1 - PORT_R))
        .size(px(PORT_R * 2.0))
        .rounded_full()
        .bg(rgb(0x3a3a44))
        .border_2()
        .border_color(rgb(0x7d7d8c))
}

fn node_box(node: &Node, pan: (f32, f32), selected: bool) -> impl IntoElement {
    div()
        .absolute()
        .left(px(node.x + pan.0))
        .top(px(node.y + pan.1))
        .w(px(NODE_W))
        .h(px(NODE_H))
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .bg(rgb(0x2b2b33))
        .border_1()
        .border_color(if selected {
            rgb(0x7aa2f7)
        } else {
            rgb(0x4a4a57)
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
fn arrow_head(tip: Point<Pixels>, from: Point<Pixels>) -> Option<Path<Pixels>> {
    const SIZE: f32 = 10.0;

    let (tip_x, tip_y) = (f32::from(tip.x), f32::from(tip.y));
    let (dx, dy) = (tip_x - f32::from(from.x), tip_y - f32::from(from.y));
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (base_x, base_y) = (tip_x - ux * SIZE, tip_y - uy * SIZE);
    // Perpendicular to the direction, half the head's width.
    let (wx, wy) = (-uy * SIZE * 0.4, ux * SIZE * 0.4);

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
            KeyBinding::new("ctrl-n", New, Some("Editor")),
            KeyBinding::new("ctrl-s", Save, Some("Editor")),
            KeyBinding::new("ctrl-shift-s", SaveAs, Some("Editor")),
            KeyBinding::new("ctrl-o", Open, Some("Editor")),
            KeyBinding::new("escape", FocusCanvas, Some("TextInput")),
        ]);
        text_input::bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Editor::new(window, cx)),
        )
        .unwrap();
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
