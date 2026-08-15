//! The dialog document — nodes, edges — and the canvas viewport that looks at it.
//! Nothing here knows about gpui, which is what keeps it testable.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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

/// The one crash copy, written by [`Doc::write_backup`] while a document is dirty.
//
// ponytail: a fixed path, so a second instance of the app shares it and will clobber it.
// Suffix the name with `std::process::id()` and offer the newest dead pid's file when
// running two copies at once stops being hypothetical.
pub fn autosave_path() -> PathBuf {
    std::env::temp_dir().join("papugai-recovery.dln")
}

/// A crashed session's document and the file it came from, so recovery restores the file
/// association and not just the content.
#[derive(Deserialize)]
pub struct Backup {
    pub path: Option<PathBuf>,
    pub doc: Doc,
}

/// The same shape borrowed, so taking a backup does not clone the document.
#[derive(Serialize)]
struct BackupRef<'a> {
    path: Option<&'a Path>,
    doc: &'a Doc,
}

/// The crash copy a previous run left behind, if there is a readable one.
pub fn read_backup() -> Option<Backup> {
    let json = std::fs::read_to_string(autosave_path()).ok()?;
    serde_json::from_str(&json).ok()
}

/// Removes the crash copy. Silent: not having one is the normal case.
pub fn clear_backup() {
    let _ = std::fs::remove_file(autosave_path());
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

/// Middle of a node, which is what the canvas centres on when jumping to one.
pub fn node_center(node: &Node) -> (f32, f32) {
    (node.x + NODE_W / 2.0, node.y + NODE_H / 2.0)
}

/// Horizontal gap between a node and one branched off it.
pub const GAP_X: f32 = 100.0;
/// Vertical gap between siblings sharing a parent.
pub const GAP_Y: f32 = 30.0;

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 3.0;

/// Where the canvas is looking: `screen = doc * zoom + pan`, with `pan` in screen
/// pixels so dragging the canvas is the same arithmetic at every zoom level.
///
/// Not part of the document and never serialized — it is view state that happens to
/// live here because it is pure arithmetic worth unit-testing.
#[derive(Clone, Copy)]
pub struct View {
    pub pan: (f32, f32),
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn to_doc(self, (x, y): (f32, f32)) -> (f32, f32) {
        ((x - self.pan.0) / self.zoom, (y - self.pan.1) / self.zoom)
    }

    pub fn to_screen(self, (x, y): (f32, f32)) -> (f32, f32) {
        (x * self.zoom + self.pan.0, y * self.zoom + self.pan.1)
    }

    /// A document-space length in screen pixels.
    pub fn scale(self, len: f32) -> f32 {
        len * self.zoom
    }

    /// Moves the viewport so `doc` lands at `screen`, leaving zoom alone.
    pub fn center_on(&mut self, doc: (f32, f32), screen: (f32, f32)) {
        self.pan = (screen.0 - doc.0 * self.zoom, screen.1 - doc.1 * self.zoom);
    }

    /// Zooms by `factor`, keeping whatever sits under `anchor` (in screen coordinates)
    /// in place: note what is there, change zoom, put it back. Because the put-back runs
    /// after the clamp, the anchor still holds when the factor hits a limit.
    pub fn zoom_at(&mut self, factor: f32, anchor: (f32, f32)) {
        let under = self.to_doc(anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center_on(under, anchor);
    }
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
    /// Only ever holds snapshots [`Doc::undo`] popped, so it needs no depth cap of its own.
    #[serde(skip)]
    redo: Vec<Snapshot>,
    #[serde(skip)]
    dirty: bool,
}

/// What a node *is*. Deliberately not where `Node::end` lives: an ending has text
/// ("Farewell, traveller.") — it is a `Speech` node that happens to terminate, so the two
/// stay orthogonal rather than folding into one enum.
#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
pub enum NodeKind {
    /// A line of dialogue: the only kind before this field existed, and so the `default`
    /// every un-migrated `.dln` node loads as.
    #[default]
    Speech,
    /// A branch point with no dialogue of its own.
    Hub,
    /// A link to a distant node, so the graph stays legible without an edge dragged all
    /// the way across it. Its target is an ordinary [`Edge`] — this kind only changes how
    /// that edge is *drawn*, which is why nothing else in this file has to know about it.
    Jump,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub speaker: String,
    pub text: String,
    /// Marked as a deliberate ending, so having no outgoing edges is not a fault.
    /// `default` so `.dln` files written before this field keep loading.
    #[serde(default)]
    pub end: bool,
    #[serde(default)]
    pub kind: NodeKind,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: u32,
    pub to: u32,
    /// The player-facing choice taken to follow this edge.
    pub label: String,
}

/// Something wrong with the document, as found by [`Doc::problems`]. Carries no message
/// text — the wording is view text and lives in `main.rs`, which also keeps these
/// assertable in tests without being brittle about phrasing.
#[derive(Debug, PartialEq)]
pub enum Problem {
    /// An edge pointing at a node that is not there. Only reachable through a bad file.
    DanglingEdge(usize),
    /// Nodes exist but every one has an incoming edge, so nothing can start.
    NoStart,
    /// Not reachable from any start node.
    Unreachable(u32),
    /// No outgoing edges — the conversation stops here.
    DeadEnd(u32),
    /// No dialogue text.
    Empty(u32),
}

/// A leaf node is an ordinary way for a conversation to end, so most findings are
/// warnings; only a broken document is an error. Without the split the list cries wolf.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Error,
    Warning,
}

impl Problem {
    /// The node to jump to, when the problem has one.
    pub fn node(&self) -> Option<u32> {
        match self {
            Problem::Unreachable(id) | Problem::DeadEnd(id) | Problem::Empty(id) => Some(*id),
            _ => None,
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Problem::DanglingEdge(_) | Problem::NoStart => Severity::Error,
            _ => Severity::Warning,
        }
    }
}

impl Doc {
    /// Where the conversation containing `from` begins: incoming edges walked back until a
    /// node nothing links to. Returns `from` itself when it is already a top, or when the
    /// walk meets a cycle. A start is relative to the node you are standing on rather than a
    /// property of the document, which is what lets one document hold several independent
    /// dialogues.
    //
    // ponytail: follows the first incoming edge in document order, so a node two branches
    // rejoin at walks back through whichever was drawn first. Both lead to the same
    // conversation; revisit if that stops being true.
    pub fn start_of(&self, from: u32) -> u32 {
        let mut current = from;
        let mut seen = HashSet::from([current]);
        while let Some(edge) = self.edges.iter().find(|e| e.to == current) {
            if !seen.insert(edge.from) || self.node(edge.from).is_none() {
                break;
            }
            current = edge.from;
        }
        current
    }

    /// The choices offered at a node: where each one leads and how it is labelled, in
    /// document order. Edges pointing at a node that is gone are skipped — play must never
    /// offer a dead link.
    pub fn choices(&self, id: u32) -> Vec<(u32, &str)> {
        self.edges
            .iter()
            .filter(|e| e.from == id && self.node(e.to).is_some())
            .map(|e| (e.to, e.label.as_str()))
            .collect()
    }

    /// Every fault in the document, errors first. An empty document has none at all.
    // ponytail: rebuilt from scratch on every call, which is once per frame. A cache
    // would need invalidating on every edit, undo and load; revisit past a few thousand nodes.
    pub fn problems(&self) -> Vec<Problem> {
        let mut found = Vec::new();
        if self.nodes.is_empty() {
            return found;
        }

        for (index, edge) in self.edges.iter().enumerate() {
            if self.node(edge.from).is_none() || self.node(edge.to).is_none() {
                found.push(Problem::DanglingEdge(index));
            }
        }

        // A start node is one nothing links to. Several of them is not a fault: a document
        // holds as many independent conversations as the author draws, and play mode starts
        // from the top of whichever one the selection is standing in.
        let roots: Vec<u32> = self
            .nodes
            .iter()
            .filter(|n| !self.edges.iter().any(|e| e.to == n.id))
            .map(|n| n.id)
            .collect();
        if roots.is_empty() {
            found.push(Problem::NoStart);
        }

        // Walking from every root at once keeps this meaningful while the start is still
        // ambiguous, and an island cycle stays unreachable either way.
        let mut seen: HashSet<u32> = roots.iter().copied().collect();
        let mut queue = roots;
        while let Some(id) = queue.pop() {
            for edge in self.edges.iter().filter(|e| e.from == id) {
                if self.node(edge.to).is_some() && seen.insert(edge.to) {
                    queue.push(edge.to);
                }
            }
        }
        found.extend(
            self.nodes
                .iter()
                .filter(|n| !seen.contains(&n.id))
                .map(|n| Problem::Unreachable(n.id)),
        );

        found.extend(
            self.nodes
                .iter()
                .filter(|n| !n.end && !self.edges.iter().any(|e| e.from == n.id))
                .map(|n| Problem::DeadEnd(n.id)),
        );
        // Only dialogue is missing anything when it has no text: a hub and a jump are
        // textless by definition, the way an `end` node is childless by definition.
        found.extend(
            self.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Speech && n.text.trim().is_empty())
                .map(|n| Problem::Empty(n.id)),
        );

        // Stable, so rule order survives inside each severity band.
        found.sort_by_key(|p| p.severity());
        found
    }

    pub fn add_node(&mut self, x: f32, y: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(Node {
            id,
            x,
            y,
            speaker: String::new(),
            text: String::new(),
            end: false,
            kind: NodeKind::Speech,
        });
        id
    }

    pub fn is_jump(&self, id: u32) -> bool {
        self.node(id).is_some_and(|n| n.kind == NodeKind::Jump)
    }

    /// Where a play-through actually lands when sent to `id`: a jump is an authoring
    /// convenience the player never sees, so walk through it to the node it points at.
    /// Guarded against a jump cycle the same way [`Doc::start_of`] is, and stops on the
    /// last good node when a jump has no target.
    pub fn resolve_jump(&self, id: u32) -> u32 {
        let mut current = id;
        let mut seen = HashSet::from([current]);
        while self.is_jump(current) {
            let Some(next) = self.choices(current).first().map(|(to, _)| *to) else {
                break;
            };
            if !seen.insert(next) {
                break;
            }
            current = next;
        }
        current
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

    /// Nodes whose speaker or text contains `query`, ignoring case, in document order.
    /// A blank query matches nothing rather than everything, so an idle search box is free.
    // ponytail: node text only; extend to edge labels if choices need finding too.
    pub fn find(&self, query: &str) -> Vec<u32> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.nodes
            .iter()
            .filter(|n| {
                n.speaker.to_lowercase().contains(&query) || n.text.to_lowercase().contains(&query)
            })
            .map(|n| n.id)
            .collect()
    }

    /// Ids of every node whose box touches the rectangle spanned by two document points,
    /// given in either corner order. Touching is enough — a marquee need not swallow a
    /// node whole to catch it.
    pub fn nodes_in_rect(&self, a: (f32, f32), b: (f32, f32)) -> Vec<u32> {
        let (min_x, max_x) = (a.0.min(b.0), a.0.max(b.0));
        let (min_y, max_y) = (a.1.min(b.1), a.1.max(b.1));
        self.nodes
            .iter()
            .filter(|n| n.x < max_x && n.x + NODE_W > min_x && n.y < max_y && n.y + NODE_H > min_y)
            .map(|n| n.id)
            .collect()
    }

    /// Shifts every listed node by the same amount. Unknown ids are skipped.
    pub fn move_nodes(&mut self, ids: &[u32], dx: f32, dy: f32) {
        for node in self.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
            node.x += dx;
            node.y += dy;
        }
    }

    /// Removes the node and every edge touching it.
    pub fn remove_node(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    pub fn has_edge(&self, from: u32, to: u32) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }

    /// Connects two nodes. False when the edge would be a self-loop or a duplicate, or
    /// would be a second target for a jump — a jump points at exactly one node, and a
    /// second edge would leave the canvas badge naming one of several.
    pub fn add_edge(&mut self, from: u32, to: u32) -> bool {
        if from == to || self.has_edge(from, to) {
            return false;
        }
        if self.is_jump(from) && self.edges.iter().any(|e| e.from == from) {
            return false;
        }
        self.edges.push(Edge {
            from,
            to,
            label: String::new(),
        });
        true
    }

    /// Adds a node connected to `from`, to its right and below any children it already
    /// has, so a fan of choices stacks downward. None when `from` is gone, and then
    /// nothing is added.
    // ponytail: only avoids siblings; it can land on an unrelated node elsewhere in the
    // graph. Scan every node for a collision if that starts to bite.
    pub fn branch_from(&mut self, from: u32) -> Option<u32> {
        let parent = self.node(from)?;
        let (x, mut y) = (parent.x + NODE_W + GAP_X, parent.y);
        let lowest_child = self
            .edges
            .iter()
            .filter(|e| e.from == from)
            .filter_map(|e| self.node(e.to))
            .map(|n| n.y)
            .fold(f32::NEG_INFINITY, f32::max);
        if lowest_child.is_finite() {
            y = lowest_child + NODE_H + GAP_Y;
        }

        let id = self.add_node(x, y);
        // A fresh id is neither a self-loop nor a duplicate, so this cannot fail.
        self.add_edge(from, id);
        Some(id)
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

    fn snapshot(&self) -> Snapshot {
        (self.nodes.clone(), self.edges.clone(), self.next_id)
    }

    fn restore(&mut self, (nodes, edges, next_id): Snapshot) {
        self.nodes = nodes;
        self.edges = edges;
        self.next_id = next_id;
    }

    /// Records the current state so the next mutation can be undone. Every mutation
    /// goes through here, which is also what makes it the right place to mark dirty.
    pub fn checkpoint(&mut self) {
        self.dirty = true;
        // A fresh edit branches history, so whatever was undone is no longer reachable.
        self.redo.clear();
        self.undo.push(self.snapshot());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Trailing newline so a committed `.dln` is a well-formed text file and git stops
    /// reporting "\ No newline at end of file" on every diff.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a document always serializes") + "\n"
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

    /// Writes a crash copy. Deliberately *not* [`Doc::save`]: `dirty` stays set, so the
    /// title keeps its `*` and the close guard keeps asking.
    pub fn write_backup(&self, origin: Option<&Path>) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&BackupRef {
            path: origin,
            doc: self,
        })
        .map_err(|e| e.to_string())?;
        // Written beside the target and renamed over it. A kill mid-write is the event this
        // file exists for, and a plain write would truncate the last good copy first.
        let temp = autosave_path().with_extension("tmp");
        std::fs::write(&temp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, autosave_path()).map_err(|e| e.to_string())
    }

    /// Restores the last checkpoint. False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(restored) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.dirty = true;
        self.restore(restored);
        true
    }

    /// Replays the last undone change. False when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(restored) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.dirty = true;
        self.restore(restored);
        true
    }

    /// A small hardcoded conversation. Deliberately shows one of each node kind: the
    /// opening branch is plain dialogue, the innkeeper's answer opens onto a *hub* of
    /// follow-up questions, and each answered question returns through a single *jump*
    /// rather than an edge dragged all the way back across the canvas.
    pub fn sample() -> Doc {
        let mut doc = Doc::default();
        let node = |doc: &mut Doc, x, y, speaker: &str, text: &str| {
            let id = doc.add_node(x, y);
            let n = doc.node_mut(id).unwrap();
            n.speaker = speaker.into();
            n.text = text.into();
            id
        };
        let edge = |doc: &mut Doc, from, to, label: &str| {
            doc.edges.push(Edge {
                from,
                to,
                label: label.into(),
            });
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

        // The hub is the point the conversation keeps coming back to. Nobody speaks at
        // one, so it has a label and no speaker.
        let more = doc.add_node(960.0, 140.0);
        let n = doc.node_mut(more).unwrap();
        n.kind = NodeKind::Hub;
        n.text = "Anything else?".into();

        let rooms = node(
            &mut doc,
            1260.0,
            20.0,
            "Innkeeper",
            "Two coppers a night, and the fire stays lit.",
        );
        let pass = node(
            &mut doc,
            1260.0,
            160.0,
            "Innkeeper",
            "Snowed shut since Tuesday. Nobody's crossing.",
        );
        let farewell = node(&mut doc, 1260.0, 300.0, "Innkeeper", "Safe travels, then.");

        // One jump serves every branch that returns to the hub, so the canvas carries no
        // long edge doubling back across it.
        let back = doc.add_node(1560.0, 90.0);
        doc.node_mut(back).unwrap().kind = NodeKind::Jump;

        edge(&mut doc, greet, ask, "Ask");
        edge(&mut doc, greet, rude, "Refuse");
        edge(&mut doc, ask, answer, "");
        edge(&mut doc, rude, sulk, "");
        edge(&mut doc, answer, more, "");
        edge(&mut doc, more, rooms, "A room for the night?");
        edge(&mut doc, more, pass, "Is the pass open?");
        edge(&mut doc, more, farewell, "That's all.");
        edge(&mut doc, rooms, back, "Ask something else");
        edge(&mut doc, pass, back, "Ask something else");
        edge(&mut doc, back, more, "");
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

    /// The sample is the first thing anyone sees, so it has to keep demonstrating both
    /// kinds: a hub several questions hang off, and a jump that returns to it.
    #[test]
    fn sample_shows_a_hub_and_a_jump() {
        let doc = Doc::sample();
        let of_kind = |kind| {
            doc.nodes
                .iter()
                .filter(|n| n.kind == kind)
                .collect::<Vec<_>>()
        };

        let [hub] = of_kind(NodeKind::Hub)[..] else {
            panic!("the sample lost its hub");
        };
        let [jump] = of_kind(NodeKind::Jump)[..] else {
            panic!("the sample lost its jump");
        };

        assert!(doc.choices(hub.id).len() > 1, "a hub is a branch point");
        assert!(hub.speaker.is_empty(), "nobody speaks at a hub");
        assert_eq!(doc.resolve_jump(jump.id), hub.id, "the jump returns to it");
        assert!(
            doc.edges.iter().filter(|e| e.to == jump.id).count() > 1,
            "one jump serves several branches, which is the point of it"
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
    fn redo_replays_an_undone_change() {
        let mut doc = Doc::sample();
        let nodes = doc.nodes.len();

        doc.checkpoint();
        let added = doc.add_node(10.0, 10.0);
        assert!(doc.undo());
        assert_eq!(doc.nodes.len(), nodes, "the node is gone after undo");

        assert!(doc.redo());
        assert_eq!(doc.nodes.len(), nodes + 1);
        assert!(doc.node(added).is_some());
        assert_eq!(doc.add_node(0.0, 0.0), added + 1, "next_id came back too");
        assert!(!doc.redo(), "nothing left to replay");
    }

    #[test]
    fn redo_is_empty_until_something_is_undone() {
        let mut doc = Doc::sample();
        doc.checkpoint();
        doc.add_node(10.0, 10.0);

        assert!(!doc.redo(), "an edit alone gives nothing to redo");
    }

    #[test]
    fn a_new_edit_discards_the_redo_stack() {
        let mut doc = Doc::sample();
        let nodes = doc.nodes.len();

        doc.checkpoint();
        doc.add_node(10.0, 10.0);
        assert!(doc.undo());

        doc.checkpoint();
        doc.add_node(20.0, 20.0);

        assert!(
            !doc.redo(),
            "the branch that added the first node is unreachable"
        );
        assert_eq!(doc.nodes.len(), nodes + 1);
        assert_eq!(doc.nodes.last().unwrap().x, 20.0, "the new edit survived");
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

    /// A node with text, so it is not flagged empty by accident.
    fn spoken(doc: &mut Doc, x: f32, y: f32) -> u32 {
        let id = doc.add_node(x, y);
        doc.node_mut(id).unwrap().text = "…".into();
        id
    }

    #[test]
    fn sample_has_no_errors() {
        let problems = Doc::sample().problems();

        assert!(
            problems.iter().all(|p| p.severity() == Severity::Warning),
            "a well-formed document raises nothing red: {problems:?}"
        );
        assert!(
            problems.iter().any(|p| matches!(p, Problem::DeadEnd(_))),
            "its two leaves are still reported, as warnings"
        );
    }

    #[test]
    fn dangling_edge_is_an_error() {
        let mut doc = Doc::sample();
        doc.edges.push(Edge {
            from: doc.nodes[0].id,
            to: 999,
            label: String::new(),
        });

        let problems = doc.problems();
        let index = doc.edges.len() - 1;
        assert!(problems.contains(&Problem::DanglingEdge(index)));
        assert_eq!(problems[0].severity(), Severity::Error, "errors sort first");
    }

    #[test]
    fn unreachable_nodes_are_reported() {
        let mut doc = Doc::default();
        let start = spoken(&mut doc, 0.0, 0.0);
        let reached = spoken(&mut doc, 300.0, 0.0);
        doc.add_edge(start, reached);

        // An island cycle: neither node is a root, so nothing walks into it.
        let (a, b) = (spoken(&mut doc, 0.0, 400.0), spoken(&mut doc, 300.0, 400.0));
        doc.add_edge(a, b);
        doc.add_edge(b, a);

        let problems = doc.problems();
        assert!(problems.contains(&Problem::Unreachable(a)));
        assert!(problems.contains(&Problem::Unreachable(b)));
        assert!(!problems.contains(&Problem::Unreachable(reached)));
        assert!(!problems.contains(&Problem::Unreachable(start)));
    }

    #[test]
    fn no_start_is_reported() {
        let mut doc = Doc::default();
        let a = spoken(&mut doc, 0.0, 0.0);
        let b = spoken(&mut doc, 300.0, 0.0);
        doc.add_edge(a, b);
        doc.add_edge(b, a);

        assert!(doc.problems().contains(&Problem::NoStart));
    }

    #[test]
    fn start_of_walks_back_to_the_top() {
        let mut doc = Doc::default();
        let a = spoken(&mut doc, 0.0, 0.0);
        let b = spoken(&mut doc, 300.0, 0.0);
        let c = spoken(&mut doc, 600.0, 0.0);
        doc.add_edge(a, b);
        doc.add_edge(b, c);

        assert_eq!(doc.start_of(c), a);
        assert_eq!(doc.start_of(b), a);
        assert_eq!(doc.start_of(a), a, "a top is its own start");
    }

    #[test]
    fn two_conversations_keep_their_own_starts() {
        let mut doc = Doc::default();
        let (a, b) = (spoken(&mut doc, 0.0, 0.0), spoken(&mut doc, 300.0, 0.0));
        let (x, y) = (spoken(&mut doc, 0.0, 400.0), spoken(&mut doc, 300.0, 400.0));
        doc.add_edge(a, b);
        doc.add_edge(x, y);

        assert_eq!(doc.start_of(b), a);
        assert_eq!(doc.start_of(y), x, "the other conversation has its own top");
        // Several starts is the intended shape of a document, not a fault.
        assert!(
            !doc.problems()
                .iter()
                .any(|p| p.severity() == Severity::Error)
        );
    }

    #[test]
    fn start_of_terminates_in_a_cycle() {
        let mut doc = Doc::default();
        let a = spoken(&mut doc, 0.0, 0.0);
        let b = spoken(&mut doc, 300.0, 0.0);
        doc.add_edge(a, b);
        doc.add_edge(b, a);

        assert!(
            [a, b].contains(&doc.start_of(b)),
            "a cycle has no top to find"
        );
    }

    #[test]
    fn choices_are_document_order_and_skip_dead_links() {
        let mut doc = Doc::default();
        let from = spoken(&mut doc, 0.0, 0.0);
        let first = spoken(&mut doc, 300.0, 0.0);
        let second = spoken(&mut doc, 300.0, 200.0);
        let gone = spoken(&mut doc, 300.0, 400.0);
        doc.add_edge(from, first);
        doc.add_edge(from, second);
        doc.add_edge(from, gone);
        doc.edges[0].label = "Yes".into();
        doc.edges[1].label = "No".into();
        // Straight at the vec: `remove_node` would take the edge with it.
        doc.nodes.retain(|n| n.id != gone);

        assert_eq!(doc.choices(from), vec![(first, "Yes"), (second, "No")]);
        assert!(doc.choices(first).is_empty());
    }

    #[test]
    fn empty_text_and_dead_ends_are_reported() {
        let mut doc = Doc::default();
        let start = spoken(&mut doc, 0.0, 0.0);
        let blank = doc.add_node(300.0, 0.0);
        doc.add_edge(start, blank);

        let problems = doc.problems();
        assert!(problems.contains(&Problem::Empty(blank)));
        assert!(!problems.contains(&Problem::Empty(start)));
        assert!(problems.contains(&Problem::DeadEnd(blank)));
        assert!(!problems.contains(&Problem::DeadEnd(start)));
    }

    #[test]
    fn an_end_node_is_not_a_dead_end() {
        let mut doc = Doc::default();
        let start = spoken(&mut doc, 0.0, 0.0);
        let last = spoken(&mut doc, 300.0, 0.0);
        doc.add_edge(start, last);
        assert!(doc.problems().contains(&Problem::DeadEnd(last)));

        doc.node_mut(last).unwrap().end = true;
        assert!(!doc.problems().contains(&Problem::DeadEnd(last)));
    }

    #[test]
    fn textless_kinds_are_not_empty_nodes() {
        let mut doc = Doc::default();
        let start = spoken(&mut doc, 0.0, 0.0);
        let hub = doc.add_node(300.0, 0.0);
        let jump = doc.add_node(600.0, 0.0);
        doc.add_edge(start, hub);
        doc.add_edge(hub, jump);
        doc.add_edge(jump, start);
        assert!(
            doc.problems().contains(&Problem::Empty(hub)),
            "a speech node with no text is still a fault"
        );

        doc.node_mut(hub).unwrap().kind = NodeKind::Hub;
        doc.node_mut(jump).unwrap().kind = NodeKind::Jump;
        let problems = doc.problems();
        assert!(!problems.contains(&Problem::Empty(hub)));
        assert!(!problems.contains(&Problem::Empty(jump)));
        assert!(!problems.contains(&Problem::Empty(start)));
    }

    #[test]
    fn a_jump_takes_only_one_target() {
        let mut doc = Doc::default();
        let jump = doc.add_node(0.0, 0.0);
        let first = spoken(&mut doc, 300.0, 0.0);
        let second = spoken(&mut doc, 300.0, 200.0);
        doc.node_mut(jump).unwrap().kind = NodeKind::Jump;

        assert!(doc.add_edge(jump, first));
        assert!(!doc.add_edge(jump, second), "a jump points at one node");
        assert_eq!(doc.choices(jump), vec![(first, "")]);
    }

    #[test]
    fn resolve_jump_walks_to_the_node_behind_it() {
        let mut doc = Doc::default();
        let first = doc.add_node(0.0, 0.0);
        let second = doc.add_node(300.0, 0.0);
        let landing = spoken(&mut doc, 600.0, 0.0);
        doc.node_mut(first).unwrap().kind = NodeKind::Jump;
        doc.node_mut(second).unwrap().kind = NodeKind::Jump;
        doc.add_edge(first, second);
        doc.add_edge(second, landing);

        assert_eq!(doc.resolve_jump(first), landing, "through a chain of jumps");
        assert_eq!(
            doc.resolve_jump(landing),
            landing,
            "speech resolves to itself"
        );
    }

    #[test]
    fn resolve_jump_terminates_in_a_cycle() {
        let mut doc = Doc::default();
        let first = doc.add_node(0.0, 0.0);
        let second = doc.add_node(300.0, 0.0);
        doc.node_mut(first).unwrap().kind = NodeKind::Jump;
        doc.node_mut(second).unwrap().kind = NodeKind::Jump;
        doc.add_edge(first, second);
        doc.add_edge(second, first);

        assert_eq!(
            doc.resolve_jump(first),
            second,
            "stops rather than spinning"
        );

        let orphan = doc.add_node(600.0, 0.0);
        doc.node_mut(orphan).unwrap().kind = NodeKind::Jump;
        assert_eq!(doc.resolve_jump(orphan), orphan, "a jump with no target");
    }

    #[test]
    fn a_node_without_a_kind_loads_as_speech() {
        let doc = Doc::from_json(
            r#"{"nodes":[{"id":0,"x":1.0,"y":2.0,"speaker":"Innkeeper","text":"Hm."}],
                "edges":[],"next_id":1}"#,
        )
        .expect("a pre-NodeKind document still loads");
        assert_eq!(doc.node(0).unwrap().kind, NodeKind::Speech);
        assert!(!doc.node(0).unwrap().end);
    }

    #[test]
    fn an_empty_document_has_no_problems() {
        assert!(
            Doc::default().problems().is_empty(),
            "nothing to start is not a fault when there is nothing at all"
        );
    }

    #[test]
    fn nodes_in_rect_catches_touched_nodes() {
        let mut doc = Doc::default();
        let first = doc.add_node(0.0, 0.0);
        let second = doc.add_node(300.0, 0.0);
        doc.add_node(0.0, 900.0); // far below the band

        // Starts inside the first node and stops just past the second's left edge.
        assert_eq!(
            doc.nodes_in_rect((10.0, 10.0), (310.0, 20.0)),
            vec![first, second]
        );
        assert!(
            doc.nodes_in_rect((NODE_W + 10.0, 10.0), (290.0, 20.0))
                .is_empty(),
            "the gap between two nodes touches neither"
        );
    }

    #[test]
    fn nodes_in_rect_ignores_corner_order() {
        let doc = Doc::sample();
        let corner = ((0.0, 0.0), (700.0, 300.0));

        let dragged_down = doc.nodes_in_rect(corner.0, corner.1);
        assert!(!dragged_down.is_empty(), "the band caught something");
        assert_eq!(doc.nodes_in_rect(corner.1, corner.0), dragged_down);
    }

    #[test]
    fn move_nodes_shifts_the_whole_set() {
        let mut doc = Doc::default();
        let a = doc.add_node(0.0, 0.0);
        let b = doc.add_node(100.0, 50.0);
        let untouched = doc.add_node(500.0, 500.0);

        doc.move_nodes(&[a, b, 999], 10.0, -5.0);

        assert_eq!(
            (doc.node(a).unwrap().x, doc.node(a).unwrap().y),
            (10.0, -5.0)
        );
        assert_eq!(
            (doc.node(b).unwrap().x, doc.node(b).unwrap().y),
            (110.0, 45.0),
            "the set moved rigidly"
        );
        assert_eq!(doc.node(untouched).unwrap().x, 500.0);
    }

    #[test]
    fn branch_from_places_the_new_node_to_the_right() {
        let mut doc = Doc::default();
        let parent = doc.add_node(100.0, 200.0);

        let child = doc.branch_from(parent).expect("parent is live");

        let node = doc.node(child).unwrap();
        assert_eq!(node.x, 100.0 + NODE_W + GAP_X);
        assert_eq!(node.y, 200.0, "the first child is level with its parent");
        assert!(doc.has_edge(parent, child));
    }

    #[test]
    fn branch_from_stacks_siblings() {
        let mut doc = Doc::default();
        let parent = doc.add_node(0.0, 0.0);

        let first = doc.branch_from(parent).unwrap();
        let second = doc.branch_from(parent).unwrap();

        let (a, b) = (doc.node(first).unwrap(), doc.node(second).unwrap());
        assert_eq!(a.x, b.x, "siblings share a column");
        assert_eq!(b.y, a.y + NODE_H + GAP_Y, "the second sits below the first");
        assert_eq!(doc.edges.iter().filter(|e| e.from == parent).count(), 2);
    }

    #[test]
    fn branch_from_ignores_a_missing_node() {
        let mut doc = Doc::sample();
        let (nodes, edges) = (doc.nodes.len(), doc.edges.len());

        assert!(doc.branch_from(999).is_none());
        assert_eq!(doc.nodes.len(), nodes, "nothing was added");
        assert_eq!(doc.edges.len(), edges);
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

    // Touches the real `autosave_path()`, so it is the only test allowed to — a second one
    // would race it. Everything about the backup is asserted here.
    #[test]
    fn a_backup_round_trips_without_clearing_dirty() {
        let mut doc = Doc::sample();
        doc.checkpoint();
        let added = doc.add_node(10.0, 20.0);

        let origin = PathBuf::from("/tmp/papugai-backup-test.dln");
        doc.write_backup(Some(&origin)).expect("write backup");
        assert!(
            doc.is_dirty(),
            "a backup is not a save: dirty must survive it"
        );
        assert!(
            !autosave_path().with_extension("tmp").exists(),
            "the temp file is renamed away, not left behind"
        );

        let backup = read_backup().expect("read backup");
        assert_eq!(backup.path, Some(origin));
        assert_eq!(backup.doc.nodes.len(), doc.nodes.len());
        assert!(backup.doc.node(added).is_some());
        assert!(!backup.doc.is_dirty(), "a read document starts clean");

        clear_backup();
        assert!(read_backup().is_none());
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

    /// Both coordinates within a pixel of each other, which is all the canvas cares about.
    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn view_round_trips_a_point() {
        let view = View {
            pan: (-120.0, 45.0),
            zoom: 1.75,
        };
        let doc_point = (310.0, 220.0);

        let there = view.to_screen(doc_point);
        assert!(there != doc_point, "the transform is doing something");
        assert!(close(view.to_doc(there), doc_point));
    }

    #[test]
    fn zoom_keeps_the_anchor_fixed() {
        let mut view = View::default();
        let anchor = (400.0, 300.0);
        let under_cursor = view.to_doc(anchor);

        view.zoom_at(1.5, anchor);

        assert_eq!(view.zoom, 1.5);
        assert!(
            close(view.to_screen(under_cursor), anchor),
            "the point under the cursor stayed put"
        );
    }

    #[test]
    fn zoom_is_clamped() {
        let mut view = View::default();
        for _ in 0..50 {
            view.zoom_at(1.5, (0.0, 0.0));
        }
        assert_eq!(view.zoom, MAX_ZOOM);

        for _ in 0..50 {
            view.zoom_at(0.5, (0.0, 0.0));
        }
        assert_eq!(view.zoom, MIN_ZOOM);
    }

    #[test]
    fn find_matches_speaker_and_text() {
        let doc = Doc::sample();

        // "Innkeeper" is a speaker; "inn before the pass" is inside another node's text.
        let hits = doc.find("inn");
        let expected: Vec<u32> = doc
            .nodes
            .iter()
            .filter(|n| n.speaker.contains("Innkeeper") || n.text.contains("inn"))
            .map(|n| n.id)
            .collect();

        assert!(hits.len() > 1, "matched both fields, not just one");
        assert_eq!(hits, expected, "ids come back in document order");
    }

    #[test]
    fn find_ignores_case() {
        let doc = Doc::sample();
        assert_eq!(doc.find("INNKEEPER"), doc.find("innkeeper"));
        assert!(!doc.find("INNKEEPER").is_empty());
    }

    #[test]
    fn find_on_an_empty_query_returns_nothing() {
        let doc = Doc::sample();
        assert!(doc.find("").is_empty());
        assert!(doc.find("   ").is_empty(), "whitespace is not a match-all");
        assert!(doc.find("no such speaker").is_empty());
    }

    #[test]
    fn center_on_puts_the_point_where_asked() {
        let mut view = View {
            pan: (37.0, -12.0),
            zoom: 2.5,
        };
        let target = (410.0, 260.0);

        view.center_on((120.0, 340.0), target);

        assert_eq!(view.zoom, 2.5, "centring does not rescale");
        assert!(close(view.to_screen((120.0, 340.0)), target));
    }

    #[test]
    fn clamped_zoom_still_holds_the_anchor() {
        let mut view = View::default();
        let anchor = (250.0, 180.0);
        let under_cursor = view.to_doc(anchor);

        // Way past the ceiling, so pan must be recomputed from the clamped zoom.
        view.zoom_at(100.0, anchor);

        assert_eq!(view.zoom, MAX_ZOOM);
        assert!(close(view.to_screen(under_cursor), anchor));
    }
}
