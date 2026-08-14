//! The dialog document: nodes, edges, and nothing that knows about gpui.

pub const NODE_W: f32 = 200.0;
pub const NODE_H: f32 = 90.0;

#[derive(Clone, Default)]
pub struct Doc {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    next_id: u32,
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
    fn ids_are_unique() {
        let mut doc = Doc::default();
        let ids: Vec<u32> = (0..5).map(|i| doc.add_node(i as f32, 0.0)).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
