mod doc;

use doc::{Doc, NODE_H, NODE_W, Node};
use gpui::{
    App, Application, Bounds, Context, Path, PathBuilder, Pixels, Point, Render, Window,
    WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, rgb, size,
};

struct Editor {
    doc: Doc,
}

impl Editor {
    /// Straight lines from the right edge of the source node to the left edge of the target.
    fn edge_lines(&self) -> Vec<(Point<Pixels>, Point<Pixels>)> {
        self.doc
            .edges
            .iter()
            .filter_map(|e| {
                let from = self.doc.node(e.from)?;
                let to = self.doc.node(e.to)?;
                Some((
                    point(px(from.x + NODE_W), px(from.y + NODE_H / 2.0)),
                    point(px(to.x), px(to.y + NODE_H / 2.0)),
                ))
            })
            .collect()
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let lines = self.edge_lines();

        div()
            .relative()
            .size_full()
            .bg(rgb(0x1e1e22))
            .text_sm()
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
            .children(self.doc.nodes.iter().map(node_box))
    }
}

fn node_box(node: &Node) -> impl IntoElement {
    div()
        .absolute()
        .left(px(node.x))
        .top(px(node.y))
        .w(px(NODE_W))
        .h(px(NODE_H))
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .bg(rgb(0x2b2b33))
        .border_1()
        .border_color(rgb(0x4a4a57))
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
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Editor { doc: Doc::sample() }),
        )
        .unwrap();
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
