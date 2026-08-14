mod doc;
mod text_input;

use doc::{Doc, NODE_H, NODE_W, Node};
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, Path, PathBuilder, Pixels, Point, Render, Window, WindowBounds,
    WindowOptions, actions, canvas, div, point, prelude::*, px, rgb, size,
};
use text_input::TextInput;

actions!(papugai, [DeleteNode, Undo, FocusCanvas]);

struct Editor {
    doc: Doc,
    /// Canvas offset in pixels, applied to everything drawn.
    pan: (f32, f32),
    selected: Option<u32>,
    drag: Option<Drag>,
    focus_handle: FocusHandle,
    speaker_input: Entity<TextInput>,
    text_input: Entity<TextInput>,
    /// Node whose content is currently loaded into the fields.
    edited: Option<u32>,
    /// Whether this editing session already pushed an undo checkpoint.
    text_dirty: bool,
}

#[derive(Clone, Copy)]
enum Drag {
    /// `grab` is the cursor's offset inside the node, in document coordinates.
    Node { id: u32, grab: (f32, f32), moved: bool },
    Pan { start_mouse: (f32, f32), start_pan: (f32, f32) },
}

impl Editor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        let speaker_input = cx.new(|cx| TextInput::new("Speaker", cx));
        let text_input = cx.new(|cx| TextInput::new("Line of dialog", cx));
        for input in [&speaker_input, &text_input] {
            cx.observe(input, |this, _, cx| this.sync_fields(cx)).detach();
        }

        Self {
            doc: Doc::sample(),
            pan: (0.0, 0.0),
            selected: None,
            drag: None,
            focus_handle,
            speaker_input,
            text_input,
            edited: None,
            text_dirty: false,
        }
    }

    /// Loads a node's content into the sidebar fields, starting a fresh editing session.
    fn load_fields(&mut self, id: Option<u32>, cx: &mut Context<Self>) {
        let (speaker, text) = match id.and_then(|id| self.doc.node(id)) {
            Some(node) => (node.speaker.clone(), node.text.clone()),
            None => (String::new(), String::new()),
        };
        self.speaker_input
            .update(cx, |input, cx| input.set_content(&speaker, cx));
        self.text_input
            .update(cx, |input, cx| input.set_content(&text, cx));
        self.edited = id;
        self.text_dirty = false;
    }

    /// Writes the fields back into the edited node. Runs on every input notification,
    /// so it must be a no-op when nothing actually changed.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.edited else { return };
        let speaker = self.speaker_input.read(cx).content().to_string();
        let text = self.text_input.read(cx).content().to_string();

        let Some(node) = self.doc.node(id) else { return };
        if node.speaker == speaker && node.text == text {
            return;
        }

        // ponytail: one undo checkpoint per node-editing session, not per keystroke.
        if !self.text_dirty {
            self.doc.checkpoint();
            self.text_dirty = true;
        }
        if let Some(node) = self.doc.node_mut(id) {
            node.speaker = speaker;
            node.text = text;
        }
        cx.notify();
    }

    fn window_pos(pos: Point<Pixels>) -> (f32, f32) {
        (f32::from(pos.x), f32::from(pos.y))
    }

    fn doc_pos(&self, pos: Point<Pixels>) -> (f32, f32) {
        let (x, y) = Self::window_pos(pos);
        (x - self.pan.0, y - self.pan.1)
    }

    /// Straight lines from the right edge of the source node to the left edge of the target.
    fn edge_lines(&self) -> Vec<(Point<Pixels>, Point<Pixels>)> {
        let (pan_x, pan_y) = self.pan;
        self.doc
            .edges
            .iter()
            .filter_map(|e| {
                let from = self.doc.node(e.from)?;
                let to = self.doc.node(e.to)?;
                Some((
                    point(px(from.x + NODE_W + pan_x), px(from.y + NODE_H / 2.0 + pan_y)),
                    point(px(to.x + pan_x), px(to.y + NODE_H / 2.0 + pan_y)),
                ))
            })
            .collect()
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let (x, y) = self.doc_pos(ev.position);

        if let Some(id) = self.doc.hit_test(x, y) {
            let node = self.doc.node(id).expect("hit_test returned a live node");
            let grab = (x - node.x, y - node.y);
            self.selected = Some(id);
            self.drag = Some(Drag::Node {
                id,
                grab,
                moved: false,
            });
            if self.edited != Some(id) {
                self.load_fields(Some(id), cx);
            }
            if ev.click_count == 2 {
                window.focus(&self.text_input.read(cx).focus_handle(cx));
            }
        } else if ev.click_count == 2 {
            self.doc.checkpoint();
            let id = self.doc.add_node(x, y);
            self.selected = Some(id);
            self.drag = None;
            self.load_fields(Some(id), cx);
            window.focus(&self.speaker_input.read(cx).focus_handle(cx));
        } else {
            self.selected = None;
            self.drag = Some(Drag::Pan {
                start_mouse: Self::window_pos(ev.position),
                start_pan: self.pan,
            });
            self.load_fields(None, cx);
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
                    self.drag = Some(Drag::Node { id, grab, moved: true });
                }
                let (x, y) = self.doc_pos(ev.position);
                if let Some(node) = self.doc.node_mut(id) {
                    node.x = x - grab.0;
                    node.y = y - grab.1;
                }
            }
            Drag::Pan {
                start_mouse,
                start_pan,
            } => {
                let (x, y) = Self::window_pos(ev.position);
                self.pan = (start_pan.0 + x - start_mouse.0, start_pan.1 + y - start_mouse.1);
            }
        }
        cx.notify();
    }

    fn on_delete(&mut self, _: &DeleteNode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.take() else {
            return;
        };
        self.doc.checkpoint();
        self.doc.remove_node(id);
        self.load_fields(None, cx);
        cx.notify();
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.undo() {
            if self.selected.is_some_and(|id| self.doc.node(id).is_none()) {
                self.selected = None;
            }
            self.load_fields(self.selected, cx);
            cx.notify();
        }
    }

    fn on_focus_canvas(&mut self, _: &FocusCanvas, window: &mut Window, _: &mut Context<Self>) {
        window.focus(&self.focus_handle);
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.edge_lines();
        let (pan, selected) = (self.pan, self.selected);

        div()
            .flex()
            .flex_row()
            .size_full()
            .text_sm()
            .child(self.canvas_pane(lines, pan, selected, cx))
            .child(self.sidebar(selected))
    }
}

impl Editor {
    /// The graph itself. Owns the mouse and key handling, so sidebar clicks never reach it.
    fn canvas_pane(
        &self,
        lines: Vec<(Point<Pixels>, Point<Pixels>)>,
        pan: (f32, f32),
        selected: Option<u32>,
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        // Paint coordinates are window-absolute, canvas bounds are not at the origin.
                        for (from, to) in lines {
                            let (from, to) = (from + bounds.origin, to + bounds.origin);
                            let mut line = PathBuilder::stroke(px(1.5));
                            line.move_to(from);
                            line.line_to(to);
                            if let Ok(line) = line.build() {
                                window.paint_path(line, rgb(0x7d7d8c));
                            }
                            if let Some(head) = arrow_head(to, from) {
                                window.paint_path(head, rgb(0x7d7d8c));
                            }
                        }
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .children(
                self.doc
                    .nodes
                    .iter()
                    .map(|node| node_box(node, pan, selected == Some(node.id))),
            )
    }

    /// Inspector for the selected node.
    fn sidebar(&self, selected: Option<u32>) -> impl IntoElement {
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
            .when_some(selected, |this, id| {
                this.child(
                    div()
                        .text_color(rgb(0x9ecbff))
                        .child(format!("Node {id}")),
                )
                .child(field("Speaker", &self.speaker_input))
                .child(field("Text", &self.text_input))
                .child(
                    div()
                        .mt_auto()
                        .text_color(rgb(0x6c6c78))
                        .child("Esc — back to canvas"),
                )
            })
            .when(selected.is_none(), |this| {
                this.child(
                    div()
                        .text_color(rgb(0x6c6c78))
                        .child("Select a node to edit it."),
                )
            })
    }
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
