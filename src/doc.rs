//! The dialog document: nodes, edges, and nothing that knows about gpui.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Extension of a saved dialog document.
pub const FILE_EXT: &str = "dln";

/// Gives the path the document extension unless it already has one.
pub fn with_default_extension(path: PathBuf) -> PathBuf {
    match path.extension() {
        Some(_) => path,
        None => path.with_extension(FILE_EXT),
    }
}

pub const NODE_W: f32 = 200.0;
pub const NODE_H: f32 = 90.0;
/// Radius of the out-port drawn on a node's right edge.
pub const PORT_R: f32 = 7.0;
/// How close the cursor must be to an edge line to hit it.
const EDGE_HIT_TOL: f32 = 6.0;

/// Where an edge leaves a node.
pub fn out_port(node: &Node) -> (f32, f32) {
    (node.x + NODE_W, node.y + NODE_H / 2.0)
}

/// Where an edge arrives at a node.
pub fn in_port(node: &Node) -> (f32, f32) {
    (node.x, node.y + NODE_H / 2.0)
}

const UNDO_DEPTH: usize = 50;

/// Everything an undo restores: the document data, minus the history itself.
type Snapshot = (Vec<Node>, Vec<Edge>, u32);

#[derive(Default, Serialize, Deserialize)]
pub struct Doc {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    next_id: u32,
    // ponytail: full snapshots, 50 deep; switch to an edit log if docs get big.
    #[serde(skip)]
    undo: Vec<Snapshot>,
    #[serde(skip)]
    dirty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub speaker: String,
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: u32,
    pub to: u32,
    /// The player-facing choice taken to follow this edge.
    pub label: String,
}

impl Doc {
    pub fn add_node(&mut self, x: f32, y: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(Node {
            id,
            x,
            y,
            speaker: String::new(),
            text: String::new(),
        });
        id
    }

    // ponytail: O(n) node lookup, add a HashMap index past ~2k nodes.
    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Topmost node containing the point, in document coordinates.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<u32> {
        self.nodes
            .iter()
            .rev()
            .find(|n| x >= n.x && x < n.x + NODE_W && y >= n.y && y < n.y + NODE_H)
            .map(|n| n.id)
    }

    /// Topmost node whose out-port contains the point. Check this before [`Doc::hit_test`],
    /// since the port overlaps the node box.
    pub fn port_hit_test(&self, x: f32, y: f32) -> Option<u32> {
        self.nodes
            .iter()
            .rev()
            .find(|n| {
                let (px, py) = out_port(n);
                (x - px).hypot(y - py) <= PORT_R
            })
            .map(|n| n.id)
    }

    /// Removes the node and every edge touching it.
    pub fn remove_node(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    pub fn has_edge(&self, from: u32, to: u32) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }

    /// Connects two nodes. False when the edge would be a self-loop or a duplicate.
    pub fn add_edge(&mut self, from: u32, to: u32) -> bool {
        if from == to || self.has_edge(from, to) {
            return false;
        }
        self.edges.push(Edge {
            from,
            to,
            label: String::new(),
        });
        true
    }

    pub fn remove_edge(&mut self, index: usize) {
        if index < self.edges.len() {
            self.edges.remove(index);
        }
    }

    /// Index of the first edge whose line passes near the point.
    pub fn edge_at(&self, x: f32, y: f32) -> Option<usize> {
        self.edges.iter().position(|e| {
            let (Some(from), Some(to)) = (self.node(e.from), self.node(e.to)) else {
                return false;
            };
            distance_to_segment((x, y), out_port(from), in_port(to)) <= EDGE_HIT_TOL
        })
    }

    /// Records the current state so the next mutation can be undone. Every mutation
    /// goes through here, which is also what makes it the right place to mark dirty.
    pub fn checkpoint(&mut self) {
        self.dirty = true;
        self.undo
            .push((self.nodes.clone(), self.edges.clone(), self.next_id));
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a document always serializes")
    }

    pub fn from_json(json: &str) -> serde_json::Result<Doc> {
        serde_json::from_str(json)
    }

    // ponytail: file IO runs on the foreground executor; fine for documents this size.
    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_json()).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Doc, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Doc::from_json(&json).map_err(|e| e.to_string())
    }

    /// Restores the last checkpoint. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some((nodes, edges, next_id)) = self.undo.pop() else {
            return false;
        };
        self.dirty = true;
        self.nodes = nodes;
        self.edges = edges;
        self.next_id = next_id;
        true
    }

    /// A small hardcoded conversation, including one branch.
    pub fn sample() -> Doc {
        let mut doc = Doc::default();
        let node = |doc: &mut Doc, x, y, speaker: &str, text: &str| {
            let id = doc.add_node(x, y);
            let n = doc.node_mut(id).unwrap();
            n.speaker = speaker.into();
            n.text = text.into();
            id
        };

        let greet = node(
            &mut doc,
            60.0,
            260.0,
            "Innkeeper",
            "You look lost, traveler.",
        );
        let ask = node(&mut doc, 360.0, 140.0, "Player", "Where am I?");
        let rude = node(&mut doc, 360.0, 380.0, "Player", "None of your business.");
        let answer = node(
            &mut doc,
            660.0,
            140.0,
            "Innkeeper",
            "Sern's Rest. Last inn before the pass.",
        );
        let sulk = node(&mut doc, 660.0, 380.0, "Innkeeper", "Suit yourself.");

        doc.edges = vec![
            Edge {
                from: greet,
                to: ask,
                label: "Ask".into(),
            },
            Edge {
                from: greet,
                to: rude,
                label: "Refuse".into(),
            },
            Edge {
                from: ask,
                to: answer,
                label: String::new(),
            },
            Edge {
                from: rude,
                to: sulk,
                label: String::new(),
            },
        ];
        doc
    }
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    // Projection of p onto the segment, clamped to its ends.
    let t = if len_sq == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    };
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_edges_resolve() {
        let doc = Doc::sample();
        for e in &doc.edges {
            assert!(doc.node(e.from).is_some(), "dangling from {}", e.from);
            assert!(doc.node(e.to).is_some(), "dangling to {}", e.to);
        }
        assert_eq!(
            doc.edges
                .iter()
                .filter(|e| e.from == doc.nodes[0].id)
                .count(),
            2
        );
    }

    #[test]
    fn hit_test_picks_topmost() {
        let mut doc = Doc::default();
        let under = doc.add_node(100.0, 100.0);
        let over = doc.add_node(150.0, 120.0);

        assert_eq!(doc.hit_test(110.0, 110.0), Some(under));
        assert_eq!(
            doc.hit_test(160.0, 130.0),
            Some(over),
            "later node is on top"
        );
        assert_eq!(
            doc.hit_test(100.0 + NODE_W, 110.0),
            None,
            "right edge is exclusive"
        );
        assert_eq!(doc.hit_test(0.0, 0.0), None);
    }

    #[test]
    fn remove_node_drops_its_edges() {
        let mut doc = Doc::sample();
        let victim = doc.nodes[1].id;
        let kept = doc
            .edges
            .iter()
            .filter(|e| e.from != victim && e.to != victim)
            .count();

        doc.remove_node(victim);

        assert!(doc.node(victim).is_none());
        assert_eq!(doc.edges.len(), kept);
    }

    #[test]
    fn undo_restores_previous_state() {
        let mut doc = Doc::sample();
        let (nodes, edges) = (doc.nodes.len(), doc.edges.len());
        let victim = doc.nodes[0].id;

        doc.checkpoint();
        doc.remove_node(victim);
        assert!(doc.undo());

        assert_eq!(doc.nodes.len(), nodes);
        assert_eq!(doc.edges.len(), edges);
        assert!(doc.node(victim).is_some());
        assert!(!doc.undo(), "history is empty again");
    }

    #[test]
    fn undo_depth_is_capped() {
        let mut doc = Doc::default();
        for _ in 0..UNDO_DEPTH + 10 {
            doc.checkpoint();
            doc.add_node(0.0, 0.0);
        }
        assert_eq!(doc.undo.len(), UNDO_DEPTH);
    }

    #[test]
    fn add_edge_rejects_self_loops_and_duplicates() {
        let mut doc = Doc::default();
        let a = doc.add_node(0.0, 0.0);
        let b = doc.add_node(400.0, 0.0);

        assert!(doc.add_edge(a, b));
        assert!(!doc.add_edge(a, b), "duplicate");
        assert!(!doc.add_edge(a, a), "self-loop");
        assert!(doc.add_edge(b, a), "opposite direction is a different edge");
        assert_eq!(doc.edges.len(), 2);
    }

    #[test]
    fn edge_at_finds_the_line() {
        let mut doc = Doc::default();
        let a = doc.add_node(0.0, 0.0);
        let b = doc.add_node(400.0, 0.0);
        doc.add_edge(a, b);

        // The line runs from a's right edge to b's left edge, both at NODE_H / 2.
        let midpoint = ((NODE_W + 400.0) / 2.0, NODE_H / 2.0);
        assert_eq!(doc.edge_at(midpoint.0, midpoint.1), Some(0));
        assert_eq!(doc.edge_at(midpoint.0, midpoint.1 + 50.0), None);
        assert_eq!(doc.edge_at(-100.0, NODE_H / 2.0), None, "before the start");
    }

    #[test]
    fn remove_edge_keeps_the_others() {
        let mut doc = Doc::sample();
        let (first, count) = (doc.edges[0].to, doc.edges.len());

        doc.remove_edge(0);

        assert_eq!(doc.edges.len(), count - 1);
        assert!(doc.edges.iter().all(|e| e.to != first || e.from != 0));
        doc.remove_edge(99); // out of range is a no-op
        assert_eq!(doc.edges.len(), count - 1);
    }

    #[test]
    fn port_hit_test_covers_the_right_edge() {
        let mut doc = Doc::default();
        let a = doc.add_node(10.0, 10.0);
        let (px, py) = out_port(doc.node(a).unwrap());

        assert_eq!(doc.port_hit_test(px, py), Some(a));
        assert_eq!(doc.port_hit_test(px - PORT_R + 1.0, py), Some(a));
        assert_eq!(doc.port_hit_test(px, py + PORT_R * 2.0), None);
    }

    #[test]
    fn json_round_trip_keeps_everything() {
        let original = Doc::sample();
        let mut restored = Doc::from_json(&original.to_json()).expect("round trip");

        assert_eq!(restored.nodes.len(), original.nodes.len());
        assert_eq!(restored.edges.len(), original.edges.len());
        assert_eq!(restored.nodes[3].text, original.nodes[3].text);
        assert_eq!(restored.nodes[3].x, original.nodes[3].x);
        assert_eq!(restored.edges[0].label, original.edges[0].label);

        // next_id survived, so new nodes cannot collide with loaded ones.
        let fresh = restored.add_node(0.0, 0.0);
        assert!(original.node(fresh).is_none());
    }

    #[test]
    fn default_extension_only_fills_a_gap() {
        let ext = |p: &str| with_default_extension(PathBuf::from(p));

        assert_eq!(ext("/tmp/scene"), PathBuf::from("/tmp/scene.dln"));
        assert_eq!(ext("/tmp/scene.dln"), PathBuf::from("/tmp/scene.dln"));
        assert_eq!(ext("/tmp/scene.json"), PathBuf::from("/tmp/scene.json"));
        assert_eq!(ext("/tmp/act 1.v2"), PathBuf::from("/tmp/act 1.v2"));
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(Doc::from_json("{ not json").is_err());
        assert!(Doc::from_json(r#"{"nodes": "wrong type"}"#).is_err());
    }

    #[test]
    fn dirty_tracks_edits_and_saves() {
        let mut doc = Doc::sample();
        assert!(!doc.is_dirty(), "a freshly built document is clean");

        doc.checkpoint();
        doc.add_node(0.0, 0.0);
        assert!(doc.is_dirty());

        let path = std::env::temp_dir().join("papugai-dirty-test.json");
        doc.save(&path).expect("save");
        assert!(!doc.is_dirty(), "saving clears dirty");

        let loaded = Doc::load(&path).expect("load");
        assert_eq!(loaded.nodes.len(), doc.nodes.len());
        assert!(!loaded.is_dirty());
        let _ = std::fs::remove_file(&path);

        doc.undo();
        assert!(doc.is_dirty(), "undo is an edit too");
    }

    #[test]
    fn ids_are_unique() {
        let mut doc = Doc::default();
        let ids: Vec<u32> = (0..5).map(|i| doc.add_node(i as f32, 0.0)).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
