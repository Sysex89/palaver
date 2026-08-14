mod doc;

use doc::{Doc, NODE_H, NODE_W, Node};
use gpui::{
    App, Application, Bounds, Context, FocusHandle, KeyBinding, MouseButton, MouseDownEvent,
    MouseMoveEvent, Path, PathBuilder, Pixels, Point, Render, Window, WindowBounds, WindowOptions,
    actions, canvas, div, point, prelude::*, px, rgb, size,
};

actions!(papugai, [Delete, Undo]);

struct Editor {
    doc: Doc,
    /// Canvas offset in pixels, applied to everything drawn.
    pan: (f32, f32),
    selected: Option<u32>,
    drag: Option<Drag>,
    focus_handle: FocusHandle,
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
        Self {
            doc: Doc::sample(),
            pan: (0.0, 0.0),
            selected: None,
            drag: None,
            focus_handle,
        }
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

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = self.doc_pos(ev.position);

        if let Some(id) = self.doc.hit_test(x, y) {
            let node = self.doc.node(id).expect("hit_test returned a live node");
            self.selected = Some(id);
            self.drag = Some(Drag::Node {
                id,
                grab: (x - node.x, y - node.y),
                moved: false,
            });
        } else if ev.click_count == 2 {
            self.doc.checkpoint();
            self.selected = Some(self.doc.add_node(x, y));
            self.drag = None;
        } else {
            self.selected = None;
            self.drag = Some(Drag::Pan {
                start_mouse: Self::window_pos(ev.position),
                start_pan: self.pan,
            });
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

    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.take() else {
            return;
        };
        self.doc.checkpoint();
        self.doc.remove_node(id);
        cx.notify();
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.undo() {
            if self.selected.is_some_and(|id| self.doc.node(id).is_none()) {
                self.selected = None;
            }
            cx.notify();
        }
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.edge_lines();
        let (pan, selected) = (self.pan, self.selected);

        div()
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .bg(rgb(0x1e1e22))
            .text_sm()
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_undo))
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
        .child(div().text_color(rgb(0x9ecbff)).child(node.speaker.clone()))
        .child(div().text_color(rgb(0xd6d6dd)).child(node.text.clone()))
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
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("backspace", Delete, None),
            KeyBinding::new("ctrl-z", Undo, None),
        ]);

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
