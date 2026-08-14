//! The dialog document: nodes, edges, and nothing that knows about gpui.

pub const NODE_W: f32 = 200.0;
pub const NODE_H: f32 = 90.0;

const UNDO_DEPTH: usize = 50;

/// Everything an undo restores: the document data, minus the history itself.
type Snapshot = (Vec<Node>, Vec<Edge>, u32);

#[derive(Default)]
pub struct Doc {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    next_id: u32,
    // ponytail: full snapshots, 50 deep; switch to an edit log if docs get big.
    undo: Vec<Snapshot>,
}

#[derive(Clone)]
pub struct Node {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub speaker: String,
    pub text: String,
}

#[derive(Clone)]
pub struct Edge {
    pub from: u32,
    pub to: u32,
    /// The player-facing choice taken to follow this edge. Not drawn yet.
    #[allow(dead_code)]
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

    /// Removes the node and every edge touching it.
    pub fn remove_node(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    /// Records the current state so the next mutation can be undone.
    pub fn checkpoint(&mut self) {
        self.undo
            .push((self.nodes.clone(), self.edges.clone(), self.next_id));
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Restores the last checkpoint. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some((nodes, edges, next_id)) = self.undo.pop() else {
            return false;
        };
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

        let greet = node(&mut doc, 60.0, 260.0, "Innkeeper", "You look lost, traveler.");
        let ask = node(&mut doc, 360.0, 140.0, "Player", "Where am I?");
        let rude = node(&mut doc, 360.0, 380.0, "Player", "None of your business.");
        let answer = node(&mut doc, 660.0, 140.0, "Innkeeper", "Sern's Rest. Last inn before the pass.");
        let sulk = node(&mut doc, 660.0, 380.0, "Innkeeper", "Suit yourself.");

        doc.edges = vec![
            Edge { from: greet, to: ask, label: "Ask".into() },
            Edge { from: greet, to: rude, label: "Refuse".into() },
            Edge { from: ask, to: answer, label: String::new() },
            Edge { from: rude, to: sulk, label: String::new() },
        ];
        doc
    }
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
        assert_eq!(doc.edges.iter().filter(|e| e.from == doc.nodes[0].id).count(), 2);
    }

    #[test]
    fn hit_test_picks_topmost() {
        let mut doc = Doc::default();
        let under = doc.add_node(100.0, 100.0);
        let over = doc.add_node(150.0, 120.0);

        assert_eq!(doc.hit_test(110.0, 110.0), Some(under));
        assert_eq!(doc.hit_test(160.0, 130.0), Some(over), "later node is on top");
        assert_eq!(doc.hit_test(100.0 + NODE_W, 110.0), None, "right edge is exclusive");
        assert_eq!(doc.hit_test(0.0, 0.0), None);
    }

    #[test]
    fn remove_node_drops_its_edges() {
        let mut doc = Doc::sample();
        let victim = doc.nodes[1].id;
        let kept = doc.edges.iter().filter(|e| e.from != victim && e.to != victim).count();

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
    fn ids_are_unique() {
        let mut doc = Doc::default();
        let ids: Vec<u32> = (0..5).map(|i| doc.add_node(i as f32, 0.0)).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
