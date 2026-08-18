//! The dialog document — nodes, edges — and the canvas viewport that looks at it.
//! Nothing here knows about gpui, which is what keeps it testable.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};

/// What a node answers to. Random rather than counted, so two branches of the same file can
/// each add a node and still merge — see [`Doc::fresh_id`]. The alias is here to make that
/// widening auditable rather than a find-and-replace on the bare type: `main.rs` also holds
/// colours as a plain integer, and a blind sweep would have widened one of those too.
pub type NodeId = u64;

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
type Snapshot = (Vec<Node>, Vec<Edge>, Vec<Variable>);

#[derive(Default, Serialize, Deserialize)]
pub struct Doc {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// `default` so `.dln` files written before variables existed keep loading.
    #[serde(default)]
    pub variables: Vec<Variable>,
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
    /// Writes to the document's variables when the flow passes through it. Logic is graph
    /// structure here rather than a hidden property of a line of dialogue: you can see it,
    /// select it, move it and search it like any other node.
    Instruction,
    /// Reads the document's variables and sends the flow down its true or false branch.
    /// Those two pins are a `bool` on the outgoing [`Edge`] rather than a second out-port,
    /// so the port geometry and the mouse map need to know nothing about this kind.
    Condition,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub x: f32,
    pub y: f32,
    pub speaker: String,
    pub text: String,
    /// A stable external handle: what a game build looks this node up by, since `id` is
    /// reissued by an undo. Never auto-generated and never auto-renamed, and empty is the
    /// ordinary state — an un-aliased node is not incomplete.
    #[serde(default)]
    pub alias: String,
    /// Marked as a deliberate ending, so having no outgoing edges is not a fault.
    /// `default` so `.dln` files written before this field keep loading.
    #[serde(default)]
    pub end: bool,
    #[serde(default)]
    pub kind: NodeKind,
    /// What this node writes to the variables — the *content* of a [`NodeKind::Instruction`],
    /// and meaningless on every other kind, the way `speaker` is meaningless on a hub.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// What this node checks — the *content* of a [`NodeKind::Condition`]. Deliberately not
    /// folded into `effects` despite the identical shape: checks stored in a field called
    /// "effects" would be a name that lies, and the two share their *editor* instead.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// A write an [`NodeKind::Instruction`] node performs when the flow passes through it. At
/// most one per variable per node — [`Doc::set_effect`] upserts rather than appends — so
/// the order they are stored in can never change the outcome.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Effect {
    pub var: String,
    pub value: Value,
}

/// A check a [`NodeKind::Condition`] node makes. Every one on a node must hold for the flow
/// to take its true branch. One per variable, like an effect.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Condition {
    pub var: String,
    pub want: Value,
}

impl Condition {
    /// A variable nothing declares never holds, so a broken file takes the false branch
    /// rather than deciding anything. [`Doc::problems`] reports it separately.
    pub fn holds(&self, vars: &VarState) -> bool {
        vars.get(&self.var) == Some(&self.want)
    }
}

/// What a variable holds. One variant today, but an enum from the start: serde tags it as
/// `{"Bool": true}`, so adding the int and string the roadmap wants is a pure addition that
/// leaves every existing `.dln` loading unchanged.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
}

/// A global flag the dialogue reads and writes.
///
/// Keyed by `name` rather than by an id — unlike the plan for entities — because the name
/// is what a `.dln` diff and a merge have to stay readable through, and there are few
/// enough variables that [`Doc::add_variable`] can simply enforce uniqueness. A rename is
/// then a find-and-replace over whatever references it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    /// The value a play-through starts from.
    pub default: Value,
}

/// Splits a variable name into the set it belongs to and the rest: everything before the
/// first `.`, which the variables pane renders as a group header.
///
/// A display convention, not schema. The whole dotted name is still the key, so grouping
/// changes nothing about uniqueness, renames or what a `.dln` holds — which is the entire
/// reason sets cost nothing today. A name with no `.`, or with nothing on one side of it,
/// belongs to no set.
pub fn split_set(name: &str) -> (Option<&str>, &str) {
    match name.split_once('.') {
        Some((set, rest)) if !set.is_empty() && !rest.is_empty() => (Some(set), rest),
        _ => (None, name),
    }
}

/// Every variable's value at one point in a play-through. Never stored on the editor —
/// see [`Doc::vars_along`], which recomputes it from the path instead.
pub type VarState = HashMap<String, Value>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// The player-facing choice taken to follow this edge.
    pub label: String,
    /// Which pin of a [`NodeKind::Condition`] this leaves by, and meaningless from any
    /// other kind. `default` so `.dln` files written before conditions keep loading — they
    /// have no condition nodes, so the value never matters to them.
    #[serde(default)]
    pub branch: bool,
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
    Unreachable(NodeId),
    /// No outgoing edges — the conversation stops here.
    DeadEnd(NodeId),
    /// Nothing in it: no dialogue on a speech node, no writes on an instruction.
    Empty(NodeId),
    /// An effect or a check naming a variable nothing declares. Only reachable through a
    /// bad file — removing a variable takes both with it.
    UnknownVar(NodeId, String),
    /// A condition node with nothing wired to one of its pins, so that branch stops the
    /// conversation. The `bool` is which pin.
    UnwiredBranch(NodeId, bool),
    /// Two nodes answering to the same alias, so the handle names neither of them. Only
    /// reachable through a bad file or a bad merge — nothing in the editor generates one.
    DuplicateAlias(NodeId, String),
    /// A second edge between the same pair of nodes, so the player is offered one choice
    /// twice. [`Doc::add_edge`] refuses one, so only a merge can make it — which is the
    /// gap random ids opened by making a `.dln` auto-merge legal again.
    DuplicateEdge(usize),
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
    pub fn node(&self) -> Option<NodeId> {
        match self {
            Problem::Unreachable(id)
            | Problem::DeadEnd(id)
            | Problem::Empty(id)
            | Problem::UnknownVar(id, _)
            | Problem::UnwiredBranch(id, _)
            | Problem::DuplicateAlias(id, _) => Some(*id),
            _ => None,
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Problem::DanglingEdge(_)
            | Problem::NoStart
            | Problem::UnknownVar(..)
            | Problem::DuplicateAlias(..) => Severity::Error,
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
    pub fn start_of(&self, from: NodeId) -> NodeId {
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
    pub fn choices(&self, id: NodeId) -> Vec<(NodeId, &str)> {
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
        let roots: Vec<NodeId> = self
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
        let mut seen: HashSet<NodeId> = roots.iter().copied().collect();
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
        // An empty node is one with none of whatever it exists to hold: dialogue for a
        // speech node, writes for an instruction. A hub and a jump hold neither by
        // definition, the way an `end` node is childless by definition.
        found.extend(
            self.nodes
                .iter()
                .filter(|n| match n.kind {
                    NodeKind::Speech => n.text.trim().is_empty(),
                    NodeKind::Instruction => n.effects.is_empty(),
                    NodeKind::Condition => n.conditions.is_empty(),
                    NodeKind::Hub | NodeKind::Jump => false,
                })
                .map(|n| Problem::Empty(n.id)),
        );

        // A condition with a pin wired to nothing stops half of every play-through dead.
        // `DeadEnd` only catches one with no edges at all, so this is the gap.
        found.extend(
            self.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Condition && !n.conditions.is_empty())
                .flat_map(|n| {
                    [true, false]
                        .into_iter()
                        .filter(|branch| {
                            !self
                                .edges
                                .iter()
                                .any(|e| e.from == n.id && e.branch == *branch)
                        })
                        .map(|branch| Problem::UnwiredBranch(n.id, branch))
                }),
        );

        found.extend(self.nodes.iter().flat_map(|n| {
            let effects = n.effects.iter().map(|e| &e.var);
            let conditions = n.conditions.iter().map(|c| &c.var);
            effects
                .chain(conditions)
                .filter(|var| self.variable(var).is_none())
                .map(|var| Problem::UnknownVar(n.id, var.clone()))
        }));

        // Empties are skipped, or every un-aliased node would collide with every other —
        // a blank alias is the ordinary state, not an unfinished one. Raised on both
        // offending nodes so either row jumps to a copy.
        // ponytail: O(n²) over aliases; index them if a document ever gets large enough
        // to notice, which the rules above would feel first.
        found.extend(
            self.nodes
                .iter()
                .filter(|n| !n.alias.trim().is_empty())
                .filter(|n| {
                    self.nodes
                        .iter()
                        .any(|other| other.id != n.id && other.alias.trim() == n.alias.trim())
                })
                .map(|n| Problem::DuplicateAlias(n.id, n.alias.trim().to_string())),
        );

        // Only the second and later copy is named, so the row points at the edge to delete
        // rather than at the one that was there first.
        found.extend(
            self.edges
                .iter()
                .enumerate()
                .filter(|(index, edge)| {
                    self.edges[..*index]
                        .iter()
                        .any(|e| e.from == edge.from && e.to == edge.to)
                })
                .map(|(index, _)| Problem::DuplicateEdge(index)),
        );

        // Stable, so rule order survives inside each severity band.
        found.sort_by_key(|p| p.severity());
        found
    }

    /// An id nothing in this document already answers to. Random rather than counted: a
    /// counter is shared state, so two branches that each add a node mint the same id and
    /// git merges that cleanly into a document whose edges point at the wrong nodes — the
    /// one corruption a `.dln` could suffer in silence.
    ///
    /// `RandomState` is OS-seeded per instance, so hashing anything at all gives a
    /// well-distributed value; the retry is against the birthday case, not the hash.
    fn fresh_id(&self) -> NodeId {
        loop {
            let id = RandomState::new().hash_one(self.nodes.len());
            if self.node(id).is_none() {
                return id;
            }
        }
    }

    pub fn add_node(&mut self, x: f32, y: f32) -> NodeId {
        let id = self.fresh_id();
        self.nodes.push(Node {
            id,
            x,
            y,
            speaker: String::new(),
            text: String::new(),
            alias: String::new(),
            end: false,
            kind: NodeKind::Speech,
            effects: Vec::new(),
            conditions: Vec::new(),
        });
        id
    }

    pub fn is_jump(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.kind == NodeKind::Jump)
    }

    pub fn is_instruction(&self, id: NodeId) -> bool {
        self.node(id)
            .is_some_and(|n| n.kind == NodeKind::Instruction)
    }

    pub fn is_condition(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.kind == NodeKind::Condition)
    }

    /// Whether every check on a condition node holds against `vars`. A node with no checks
    /// holds vacuously, which is why an empty one is reported by [`Doc::problems`] rather
    /// than silently sending everything down one branch.
    pub fn condition_holds(&self, id: NodeId, vars: &VarState) -> bool {
        self.node(id)
            .is_some_and(|node| node.conditions.iter().all(|c| c.holds(vars)))
    }

    /// Whether the player actually stops here. The single definition of it, so the walk
    /// below and the panel that renders the result cannot disagree about which nodes are
    /// authoring plumbing. A missing node counts as visible, so a broken graph halts
    /// rather than walking off the end of it.
    pub fn pauses_play(&self, id: NodeId) -> bool {
        match self.node(id) {
            Some(node) => matches!(node.kind, NodeKind::Speech | NodeKind::Hub),
            None => true,
        }
    }

    /// The branch a condition sends the flow down, given the variables as they stand.
    /// None when that pin has nothing wired to it, which stops the walk where it is.
    fn condition_branch(&self, id: NodeId, vars: &VarState) -> Option<NodeId> {
        let holds = self.condition_holds(id, vars);
        self.edges
            .iter()
            .find(|e| e.from == id && e.branch == holds && self.node(e.to).is_some())
            .map(|e| e.to)
    }

    /// Walks on from `id` through everything the player never sees — a jump, an
    /// instruction — appending each node passed to `path`, and stopping on the first one
    /// that [`Doc::pauses_play`]. The invisible nodes stay in the path deliberately: it is
    /// what lets [`Doc::vars_along`] recompute the variables rather than store them.
    ///
    /// Guarded against a cycle the same way [`Doc::start_of`] is, and stops where it
    /// stands when a jump has no target.
    fn walk_on(&self, path: &mut Vec<NodeId>) {
        let mut seen: HashSet<NodeId> = path.iter().copied().collect();
        while let Some(&current) = path.last() {
            if self.pauses_play(current) {
                return;
            }
            // The only place a condition is ever evaluated. `vars_along` is recomputed for
            // each one met rather than carried along, which keeps the path the single copy
            // of the play state — and it is a walk of a few dozen nodes.
            let next = if self.is_condition(current) {
                self.condition_branch(current, &self.vars_along(path))
            } else {
                self.choices(current).first().map(|(to, _)| *to)
            };
            let Some(next) = next else {
                return;
            };
            if !seen.insert(next) {
                return;
            }
            path.push(next);
        }
    }

    /// The path a play-through opens on: `seed`, plus whatever it walks through to reach
    /// the first node the player sees.
    pub fn start_walk(&self, seed: NodeId) -> Vec<NodeId> {
        let mut path = vec![seed];
        self.walk_on(&mut path);
        path
    }

    /// Takes the choice leading to `to` and walks on from there.
    pub fn advance(&self, path: &mut Vec<NodeId>, to: NodeId) {
        path.push(to);
        self.walk_on(path);
    }

    /// Steps back to the previous node the player actually saw, dropping the invisible run
    /// walked through to reach the current one. False when there is no earlier one — which
    /// is also what hides the Back button.
    pub fn step_back(&self, path: &mut Vec<NodeId>) -> bool {
        // Searched by position, not by id: a conversation that loops back to a node it has
        // already visited would otherwise step back to the wrong occurrence of it.
        let Some(before) = path.len().checked_sub(1) else {
            return false;
        };
        let Some(at) = path[..before].iter().rposition(|&id| self.pauses_play(id)) else {
            return false;
        };
        path.truncate(at + 1);
        true
    }

    // ponytail: O(n) node lookup, add a HashMap index past ~2k nodes.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Topmost node containing the point, in document coordinates.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        self.nodes
            .iter()
            .rev()
            .find(|n| x >= n.x && x < n.x + NODE_W && y >= n.y && y < n.y + NODE_H)
            .map(|n| n.id)
    }

    /// Topmost node whose out-port contains the point. Check this before [`Doc::hit_test`],
    /// since the port overlaps the node box.
    pub fn port_hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
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
    pub fn find(&self, query: &str) -> Vec<NodeId> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.nodes
            .iter()
            .filter(|n| {
                n.speaker.to_lowercase().contains(&query)
                    || n.text.to_lowercase().contains(&query)
                    || n.alias.to_lowercase().contains(&query)
            })
            .map(|n| n.id)
            .collect()
    }

    /// Ids of every node whose box touches the rectangle spanned by two document points,
    /// given in either corner order. Touching is enough — a marquee need not swallow a
    /// node whole to catch it.
    pub fn nodes_in_rect(&self, a: (f32, f32), b: (f32, f32)) -> Vec<NodeId> {
        let (min_x, max_x) = (a.0.min(b.0), a.0.max(b.0));
        let (min_y, max_y) = (a.1.min(b.1), a.1.max(b.1));
        self.nodes
            .iter()
            .filter(|n| n.x < max_x && n.x + NODE_W > min_x && n.y < max_y && n.y + NODE_H > min_y)
            .map(|n| n.id)
            .collect()
    }

    /// Shifts every listed node by the same amount. Unknown ids are skipped.
    pub fn move_nodes(&mut self, ids: &[NodeId], dx: f32, dy: f32) {
        for node in self.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
            node.x += dx;
            node.y += dy;
        }
    }

    /// Removes the node and every edge touching it.
    pub fn remove_node(&mut self, id: NodeId) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    pub fn has_edge(&self, from: NodeId, to: NodeId) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }

    /// Connects two nodes. False when the edge would be a self-loop or a duplicate, or
    /// would be a second target for a jump — a jump points at exactly one node, and a
    /// second edge would leave the canvas badge naming one of several.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId) -> bool {
        if from == to || self.has_edge(from, to) {
            return false;
        }
        if self.is_jump(from) && self.edges.iter().any(|e| e.from == from) {
            return false;
        }
        // The first edge out of a condition is its true branch: the happy path is what gets
        // drawn first, and the edge inspector flips it either way.
        let branch = self.is_condition(from) && !self.edges.iter().any(|e| e.from == from);
        self.edges.push(Edge {
            from,
            to,
            label: String::new(),
            branch,
        });
        true
    }

    /// Adds a node connected to `from`, to its right and below any children it already
    /// has, so a fan of choices stacks downward. None when `from` is gone, and then
    /// nothing is added.
    // ponytail: only avoids siblings; it can land on an unrelated node elsewhere in the
    // graph. Scan every node for a collision if that starts to bite.
    pub fn branch_from(&mut self, from: NodeId) -> Option<NodeId> {
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

    // ponytail: O(n) like the node lookup, and there are far fewer variables than nodes.
    pub fn variable(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name == name)
    }

    /// Whether [`Doc::add_variable`] would accept this name. Separate so the view can ask
    /// *before* it checkpoints: a refused add must not leave an empty undo step behind.
    pub fn variable_name_free(&self, name: &str) -> bool {
        let name = name.trim();
        !name.is_empty() && self.variable(name).is_none()
    }

    /// Declares a flag, starting out false. False when the name is blank or already taken —
    /// names are the key, so a duplicate would make every reference ambiguous.
    pub fn add_variable(&mut self, name: &str) -> bool {
        if !self.variable_name_free(name) {
            return false;
        }
        self.variables.push(Variable {
            name: name.trim().to_string(),
            default: Value::Bool(false),
        });
        true
    }

    /// Removes the flag and every effect that writes it, returning how many effects went
    /// with it — an effect naming a variable nothing declares has no meaning, so leaving
    /// them behind is not an option. It is one checkpoint's worth of change, so a single
    /// undo brings the variable and all of its effects back together.
    pub fn remove_variable(&mut self, name: &str) -> usize {
        self.variables.retain(|v| v.name != name);
        let mut dropped = 0;
        for node in &mut self.nodes {
            let before = node.effects.len() + node.conditions.len();
            node.effects.retain(|e| e.var != name);
            node.conditions.retain(|c| c.var != name);
            dropped += before - node.effects.len() - node.conditions.len();
        }
        dropped
    }

    /// Renames the flag and every effect and check that names it, returning how many
    /// references moved. The mirror of [`Doc::remove_variable`], and possible at all
    /// because the name *is* the key: a rename is a find-and-replace, and there is nowhere
    /// else for a reference to hide. Any third field that ever stores a variable name has
    /// to join both functions.
    ///
    /// Returns 0 without touching anything when nothing is called `from`. The view checks
    /// [`Doc::variable_name_free`] for `to` before it checkpoints, the same way the add
    /// does, so a refused rename leaves no empty undo step.
    pub fn rename_variable(&mut self, from: &str, to: &str) -> usize {
        let to = to.trim();
        let Some(variable) = self.variables.iter_mut().find(|v| v.name == from) else {
            return 0;
        };
        variable.name = to.to_string();

        let mut moved = 0;
        for node in &mut self.nodes {
            let effects = node.effects.iter_mut().map(|e| &mut e.var);
            let conditions = node.conditions.iter_mut().map(|c| &mut c.var);
            for var in effects.chain(conditions).filter(|var| *var == from) {
                var.clear();
                var.push_str(to);
                moved += 1;
            }
        }
        moved
    }

    /// What this node writes to `var`, if anything.
    pub fn effect_of(&self, id: NodeId, var: &str) -> Option<Value> {
        self.node(id)?
            .effects
            .iter()
            .find(|e| e.var == var)
            .map(|e| e.value)
    }

    /// Sets — or with `None`, clears — what this node writes to `var`. An upsert, which is
    /// what keeps at most one effect per variable per node and makes their stored order
    /// irrelevant.
    pub fn set_effect(&mut self, id: NodeId, var: &str, value: Option<Value>) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        node.effects.retain(|e| e.var != var);
        if let Some(value) = value {
            node.effects.push(Effect {
                var: var.to_string(),
                value,
            });
        }
    }

    /// What this node checks `var` against, if anything.
    pub fn condition_of(&self, id: NodeId, var: &str) -> Option<Value> {
        self.node(id)?
            .conditions
            .iter()
            .find(|c| c.var == var)
            .map(|c| c.want)
    }

    /// The [`Doc::set_effect`] of the checking side, with the same upsert rule.
    pub fn set_condition(&mut self, id: NodeId, var: &str, want: Option<Value>) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        node.conditions.retain(|c| c.var != var);
        if let Some(want) = want {
            node.conditions.push(Condition {
                var: var.to_string(),
                want,
            });
        }
    }

    /// Every declared variable at the value a play-through starts it from.
    pub fn initial_vars(&self) -> VarState {
        self.variables
            .iter()
            .map(|v| (v.name.clone(), v.default))
            .collect()
    }

    /// The variables as they stand after walking `path`: the declared defaults, then every
    /// instruction along the way applied in order.
    ///
    /// Recomputed from the path rather than carried beside it, which is what makes it
    /// impossible for the two to drift — stepping back through the path steps the
    /// variables back with it for free.
    pub fn vars_along(&self, path: &[NodeId]) -> VarState {
        let mut vars = self.initial_vars();
        let instructions = path
            .iter()
            .filter_map(|id| self.node(*id))
            .filter(|n| n.kind == NodeKind::Instruction);
        for effect in instructions.flat_map(|n| &n.effects) {
            // Only a declared variable. Inventing one here would paper over the broken file
            // that `problems` is about to report.
            if let Some(slot) = vars.get_mut(&effect.var) {
                *slot = effect.value;
            }
        }
        vars
    }

    /// Sets the value a play-through starts this variable from. Unknown names are ignored.
    pub fn set_default(&mut self, name: &str, value: Value) {
        if let Some(variable) = self.variables.iter_mut().find(|v| v.name == name) {
            variable.default = value;
        }
    }

    fn snapshot(&self) -> Snapshot {
        (
            self.nodes.clone(),
            self.edges.clone(),
            self.variables.clone(),
        )
    }

    fn restore(&mut self, (nodes, edges, variables): Snapshot) {
        self.nodes = nodes;
        self.edges = edges;
        self.variables = variables;
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

    /// A small hardcoded conversation. Deliberately shows one of every node kind: the
    /// opening branch is plain dialogue, the innkeeper's answer opens onto a *hub* of
    /// follow-up questions, each answered question returns through a single *jump* rather
    /// than an edge dragged back across the canvas, asking about the pass runs an
    /// *instruction* that remembers you did, and the goodbye runs a *condition* on it.
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
                branch: false,
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
        // What the conversation remembers about itself. One global list, declared here the
        // same way the variables panel declares one.
        doc.add_variable("asked_about_pass");

        // Logic is graph structure: the instruction that records the question and the
        // condition that reads it back are both nodes you can see on the canvas.
        let noted = doc.add_node(1560.0, 160.0);
        doc.node_mut(noted).unwrap().kind = NodeKind::Instruction;
        doc.set_effect(noted, "asked_about_pass", Some(Value::Bool(true)));

        let check = doc.add_node(1260.0, 300.0);
        doc.node_mut(check).unwrap().kind = NodeKind::Condition;
        doc.set_condition(check, "asked_about_pass", Some(Value::Bool(true)));

        let warned = node(
            &mut doc,
            1560.0,
            300.0,
            "Innkeeper",
            "Mind the drifts on the road out.",
        );
        let farewell = node(&mut doc, 1560.0, 440.0, "Innkeeper", "Safe travels, then.");

        // One jump serves every branch that returns to the hub, so the canvas carries no
        // long edge doubling back across it.
        let back = doc.add_node(1860.0, 60.0);
        doc.node_mut(back).unwrap().kind = NodeKind::Jump;

        edge(&mut doc, greet, ask, "Ask");
        edge(&mut doc, greet, rude, "Refuse");
        edge(&mut doc, ask, answer, "");
        edge(&mut doc, rude, sulk, "");
        edge(&mut doc, answer, more, "");
        edge(&mut doc, more, rooms, "A room for the night?");
        edge(&mut doc, more, pass, "Is the pass open?");
        edge(&mut doc, more, check, "That's all.");
        edge(&mut doc, rooms, back, "Ask something else");
        edge(&mut doc, pass, noted, "Ask something else");
        edge(&mut doc, noted, back, "");
        edge(&mut doc, back, more, "");
        // Through `add_edge`, so the first pin out of the condition is its true branch —
        // the rule the editor relies on when an author drags the happy path first.
        doc.add_edge(check, warned);
        doc.add_edge(check, farewell);
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
        assert_eq!(
            doc.start_walk(jump.id).last(),
            Some(&hub.id),
            "the jump returns to it"
        );
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
        assert_ne!(
            doc.add_node(0.0, 0.0),
            added,
            "ids are minted, not counted, so nothing reissues one history handed back"
        );
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

    /// The whole reason ids stopped being counted: a counter is shared state, so two
    /// branches of the same file each mint the same id and git merges that into a document
    /// whose edges point at the wrong nodes.
    #[test]
    fn ids_are_minted_not_counted() {
        let mut doc = Doc::default();
        let ids: Vec<NodeId> = (0..20).map(|i| doc.add_node(i as f32, 0.0)).collect();

        let unique: HashSet<NodeId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "no collisions within a document");
        assert!(
            ids.windows(2).any(|w| w[1] != w[0] + 1),
            "and not a counter wearing a hat"
        );
    }

    /// Replaces the old caveat that ids were unstable across a history step. There is no
    /// counter left to roll back, so an undone id is retired rather than handed out again.
    #[test]
    fn an_undone_id_is_never_reissued() {
        let mut doc = Doc::default();
        doc.checkpoint();
        let gone = doc.add_node(0.0, 0.0);
        assert!(doc.undo());

        assert_ne!(doc.add_node(0.0, 0.0), gone);
    }

    /// The compatibility claim: every `.dln` written before this change carries a
    /// `next_id`, and serde ignoring it is the whole migration.
    #[test]
    fn a_document_with_a_next_id_still_loads() {
        let doc = Doc::from_json(
            r#"{"nodes":[{"id":0,"x":0.0,"y":0.0,"speaker":"","text":"hi"}],
                "edges":[],"next_id":1}"#,
        )
        .expect("loads");

        assert_eq!(doc.node(0).map(|n| n.text.as_str()), Some("hi"));
    }

    /// `add_edge` refuses one, so only a merge can make it — which is the gap random ids
    /// opened by making a `.dln` auto-merge legal again.
    #[test]
    fn a_duplicated_edge_is_a_warning() {
        let mut doc = Doc::default();
        let a = spoken(&mut doc, 0.0, 0.0);
        let b = spoken(&mut doc, 400.0, 0.0);
        doc.add_edge(a, b);
        let copy = doc.edges[0].clone(); // What a line-wise merge does.
        doc.edges.push(copy);

        let problems = doc.problems();
        let dupes: Vec<&Problem> = problems
            .iter()
            .filter(|p| matches!(p, Problem::DuplicateEdge(_)))
            .collect();

        assert_eq!(
            dupes,
            vec![&Problem::DuplicateEdge(1)],
            "the copy is named, not the edge that was there first"
        );
        assert_eq!(dupes[0].severity(), Severity::Warning);
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
    fn spoken(doc: &mut Doc, x: f32, y: f32) -> NodeId {
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
            branch: false,
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
    fn the_walk_passes_through_jumps_and_instructions() {
        let mut doc = Doc::default();
        let first = doc.add_node(0.0, 0.0);
        let second = doc.add_node(300.0, 0.0);
        let sets = doc.add_node(450.0, 0.0);
        let landing = spoken(&mut doc, 600.0, 0.0);
        doc.node_mut(first).unwrap().kind = NodeKind::Jump;
        doc.node_mut(second).unwrap().kind = NodeKind::Jump;
        doc.node_mut(sets).unwrap().kind = NodeKind::Instruction;
        doc.add_edge(first, second);
        doc.add_edge(second, sets);
        doc.add_edge(sets, landing);

        assert_eq!(
            doc.start_walk(first),
            vec![first, second, sets, landing],
            "every node passed stays in the path, so the variables can be replayed from it"
        );
        assert_eq!(
            doc.start_walk(landing),
            vec![landing],
            "a node the player sees is where the walk stops"
        );
    }

    #[test]
    fn the_walk_terminates_in_a_cycle() {
        let mut doc = Doc::default();
        let first = doc.add_node(0.0, 0.0);
        let second = doc.add_node(300.0, 0.0);
        doc.node_mut(first).unwrap().kind = NodeKind::Jump;
        doc.node_mut(second).unwrap().kind = NodeKind::Jump;
        doc.add_edge(first, second);
        doc.add_edge(second, first);

        assert_eq!(
            doc.start_walk(first),
            vec![first, second],
            "stops rather than spinning"
        );

        let orphan = doc.add_node(600.0, 0.0);
        doc.node_mut(orphan).unwrap().kind = NodeKind::Jump;
        assert_eq!(
            doc.start_walk(orphan),
            vec![orphan],
            "a jump with no target"
        );
    }

    #[test]
    fn advance_walks_on_from_the_choice_taken() {
        let mut doc = Doc::default();
        let here = spoken(&mut doc, 0.0, 0.0);
        let sets = doc.add_node(300.0, 0.0);
        let there = spoken(&mut doc, 600.0, 0.0);
        doc.node_mut(sets).unwrap().kind = NodeKind::Instruction;
        doc.add_edge(sets, there);

        let mut path = doc.start_walk(here);
        doc.advance(&mut path, sets);

        assert_eq!(path, vec![here, sets, there]);
    }

    #[test]
    fn step_back_lands_on_the_previous_visible_node() {
        let mut doc = Doc::default();
        let here = spoken(&mut doc, 0.0, 0.0);
        let sets = doc.add_node(300.0, 0.0);
        let there = spoken(&mut doc, 600.0, 0.0);
        doc.node_mut(sets).unwrap().kind = NodeKind::Instruction;

        let mut path = vec![here, sets, there];
        assert!(doc.step_back(&mut path));
        assert_eq!(path, vec![here], "the instruction went back with it");

        assert!(!doc.step_back(&mut path), "nothing before the first node");
        assert_eq!(path, vec![here], "and it left the path alone");
    }

    /// A conversation that returns to a node it has already played must step back to the
    /// occurrence it just came from, not to the first one in the path.
    #[test]
    fn step_back_counts_positions_not_ids() {
        let mut doc = Doc::default();
        let hub = spoken(&mut doc, 0.0, 0.0);
        let away = spoken(&mut doc, 300.0, 0.0);

        let mut path = vec![hub, away, hub, away];
        assert!(doc.step_back(&mut path));
        assert_eq!(path, vec![hub, away, hub]);
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

        // Ids survive the trip, so the edges still point where they did.
        assert_eq!(restored.nodes[3].id, original.nodes[3].id);
        let fresh = restored.add_node(0.0, 0.0);
        assert!(original.node(fresh).is_none(), "and a new one is its own");
    }

    #[test]
    fn variables_round_trip() {
        let mut original = Doc::sample();
        original.add_variable("has_key");
        original.add_variable("paid");
        original.set_default("paid", Value::Bool(true));

        let restored = Doc::from_json(&original.to_json()).expect("round trip");

        assert_eq!(restored.variables, original.variables);
        assert_eq!(
            restored.variable("paid").map(|v| v.default),
            Some(Value::Bool(true))
        );
    }

    /// Variables arrived after the format did, so every `.dln` written before them must
    /// still open — with no variables and no complaint.
    #[test]
    fn a_document_without_variables_loads() {
        let doc = Doc::from_json(r#"{"nodes": [], "edges": [], "next_id": 0}"#).expect("loads");
        assert!(doc.variables.is_empty());
    }

    #[test]
    fn a_variable_name_is_unique_and_not_blank() {
        let mut doc = Doc::default();

        assert!(doc.add_variable("has_key"));
        assert!(!doc.add_variable("has_key"), "already declared");
        assert!(!doc.add_variable("   "), "a blank name keys nothing");
        assert!(doc.add_variable("  paid  "), "trimmed, not rejected");

        assert!(doc.variable("paid").is_some(), "stored under the trim");
        assert_eq!(doc.variables.len(), 2);

        // The view checkpoints only when this says yes, so the two must never disagree.
        for name in ["has_key", "  paid  ", "", "   "] {
            assert!(!doc.variable_name_free(name), "{name:?} is not free");
        }
        assert!(doc.variable_name_free("door_open"));
    }

    #[test]
    fn set_default_reads_back() {
        let mut doc = Doc::default();
        doc.add_variable("paid");

        assert_eq!(
            doc.variable("paid").map(|v| v.default),
            Some(Value::Bool(false)),
            "a fresh flag starts false"
        );

        doc.set_default("paid", Value::Bool(true));
        assert_eq!(
            doc.variable("paid").map(|v| v.default),
            Some(Value::Bool(true))
        );

        doc.set_default("nonexistent", Value::Bool(true));
        assert_eq!(doc.variables.len(), 1, "an unknown name declares nothing");
    }

    #[test]
    fn split_set_groups_on_the_first_dot() {
        assert_eq!(split_set("paid"), (None, "paid"));
        assert_eq!(split_set("chapter1.paid"), (Some("chapter1"), "paid"));
        assert_eq!(
            split_set("a.b.c"),
            (Some("a"), "b.c"),
            "one level: the rest is the name, dots and all"
        );

        // Nothing on one side of the dot is a name, not a set.
        assert_eq!(split_set(".paid"), (None, ".paid"));
        assert_eq!(split_set("chapter1."), (None, "chapter1."));
        assert_eq!(split_set(""), (None, ""));
    }

    /// Builds an instruction node that writes `var`.
    fn sets(doc: &mut Doc, var: &str, value: bool) -> NodeId {
        let id = doc.add_node(0.0, 0.0);
        doc.node_mut(id).unwrap().kind = NodeKind::Instruction;
        doc.set_effect(id, var, Some(Value::Bool(value)));
        id
    }

    #[test]
    fn set_effect_upserts_and_clears() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let node = sets(&mut doc, "paid", true);

        doc.set_effect(node, "paid", Some(Value::Bool(false)));
        assert_eq!(
            doc.node(node).unwrap().effects.len(),
            1,
            "one write per variable per node, so their order can never matter"
        );
        assert_eq!(doc.effect_of(node, "paid"), Some(Value::Bool(false)));

        doc.set_effect(node, "paid", None);
        assert_eq!(doc.effect_of(node, "paid"), None);
        assert!(doc.node(node).unwrap().effects.is_empty());
    }

    #[test]
    fn vars_along_replays_the_path() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        doc.add_variable("greeted");
        doc.set_default("greeted", Value::Bool(true));

        let start = spoken(&mut doc, 0.0, 0.0);
        let pay = sets(&mut doc, "paid", true);
        let refund = sets(&mut doc, "paid", false);

        let value = |vars: &VarState, name: &str| vars.get(name).copied();

        let vars = doc.vars_along(&[start]);
        assert_eq!(
            value(&vars, "greeted"),
            Some(Value::Bool(true)),
            "starts from the declared default, not from false"
        );
        assert_eq!(value(&vars, "paid"), Some(Value::Bool(false)));

        let vars = doc.vars_along(&[start, pay]);
        assert_eq!(value(&vars, "paid"), Some(Value::Bool(true)));

        let vars = doc.vars_along(&[start, pay, refund]);
        assert_eq!(
            value(&vars, "paid"),
            Some(Value::Bool(false)),
            "later writes win"
        );

        // Stepping back is what makes this worth recomputing rather than storing.
        let vars = doc.vars_along(&[start, pay]);
        assert_eq!(value(&vars, "paid"), Some(Value::Bool(true)));
    }

    #[test]
    fn vars_along_ignores_effects_that_cannot_run() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let speech = spoken(&mut doc, 0.0, 0.0);
        doc.set_effect(speech, "paid", Some(Value::Bool(true)));

        assert_eq!(
            doc.vars_along(&[speech]).get("paid"),
            Some(&Value::Bool(false)),
            "effects belong to an instruction; play never runs one hidden on a line"
        );

        let ghost = sets(&mut doc, "nonexistent", true);
        assert!(
            !doc.vars_along(&[ghost]).contains_key("nonexistent"),
            "an undeclared variable is not invented here — `problems` reports it"
        );
    }

    #[test]
    fn removing_a_variable_takes_its_effects() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let one = sets(&mut doc, "paid", true);
        let two = sets(&mut doc, "paid", false);

        doc.checkpoint();
        assert_eq!(
            doc.remove_variable("paid"),
            2,
            "reports what it cascaded to"
        );
        assert!(doc.node(one).unwrap().effects.is_empty());
        assert!(doc.node(two).unwrap().effects.is_empty());

        assert!(doc.undo(), "one step, so one undo");
        assert!(doc.variable("paid").is_some());
        assert_eq!(doc.effect_of(one, "paid"), Some(Value::Bool(true)));
        assert_eq!(doc.effect_of(two, "paid"), Some(Value::Bool(false)));
    }

    /// The point of the rename: a typo can be fixed without re-ticking every node, which
    /// deleting and re-declaring would cost. Both reference sites have to move, and the
    /// count has to cover both.
    #[test]
    fn renaming_a_variable_moves_its_effects_and_checks() {
        let mut doc = Doc::default();
        doc.add_variable("pade");
        let writes = sets(&mut doc, "pade", true);
        let reads = checks(&mut doc, "pade", true);

        assert_eq!(
            doc.rename_variable("pade", "  paid  "),
            2,
            "trimmed on the way in"
        );
        assert!(doc.variable("paid").is_some());
        assert!(doc.variable("pade").is_none(), "the old key is gone");
        assert_eq!(doc.effect_of(writes, "paid"), Some(Value::Bool(true)));
        assert_eq!(doc.condition_of(reads, "paid"), Some(Value::Bool(true)));
        assert!(
            doc.problems()
                .iter()
                .all(|p| !matches!(p, Problem::UnknownVar(..))),
            "a reference site the rename missed would show up here"
        );
    }

    /// A string that changed is not the same as a flag that still works — assert through
    /// the walk, which is what the author actually notices.
    #[test]
    fn a_renamed_variable_still_drives_the_play_walk() {
        let mut doc = Doc::default();
        doc.add_variable("pade");
        let start = spoken(&mut doc, 0.0, 0.0);
        let writes = sets(&mut doc, "pade", true);
        doc.add_edge(start, writes);
        doc.rename_variable("pade", "paid");

        let mut path = doc.start_walk(start);
        doc.advance(&mut path, writes);
        assert_eq!(
            doc.vars_along(&path).get("paid"),
            Some(&Value::Bool(true)),
            "the instruction still writes the flag it was pointed at"
        );
    }

    #[test]
    fn renaming_a_flag_nothing_declares_does_nothing() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let node = sets(&mut doc, "paid", true);

        assert_eq!(doc.rename_variable("ghost", "spectre"), 0);
        assert!(doc.variable("spectre").is_none());
        assert_eq!(doc.effect_of(node, "paid"), Some(Value::Bool(true)));
    }

    /// The view gates on this before it checkpoints, so a refused rename must be refusable
    /// without touching the document — including a rename to the name it already has.
    #[test]
    fn the_rename_gate_refuses_a_taken_or_blank_name() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        doc.add_variable("has_key");

        assert!(!doc.variable_name_free("has_key"), "already taken");
        assert!(!doc.variable_name_free("paid"), "its own name is taken too");
        assert!(!doc.variable_name_free("  "), "a blank name keys nothing");
        assert!(doc.variable_name_free("asked"));
    }

    /// Both nodes are named, so whichever row the author clicks jumps to a copy of the
    /// clash. An error, not a warning: the handle now names neither node, which is the
    /// `DanglingEdge`/`UnknownVar` category of a bad file or a bad merge.
    #[test]
    fn two_nodes_sharing_an_alias_are_both_an_error() {
        let mut doc = Doc::default();
        let first = spoken(&mut doc, 0.0, 0.0);
        let second = spoken(&mut doc, 400.0, 0.0);
        doc.node_mut(first).unwrap().alias = "intro".into();
        doc.node_mut(second).unwrap().alias = "  intro  ".into();

        let problems = doc.problems();
        let clashes: Vec<&Problem> = problems
            .iter()
            .filter(|p| matches!(p, Problem::DuplicateAlias(..)))
            .collect();

        assert_eq!(
            clashes.iter().filter_map(|p| p.node()).collect::<Vec<_>>(),
            vec![first, second],
            "named on both sides, and matched past the whitespace"
        );
        assert!(clashes.iter().all(|p| p.severity() == Severity::Error));
    }

    /// The rule that makes an empty alias the ordinary state rather than an unfinished
    /// one — without the skip, every un-aliased node collides with every other.
    #[test]
    fn un_aliased_nodes_never_clash() {
        let mut doc = Doc::default();
        spoken(&mut doc, 0.0, 0.0);
        let blank = spoken(&mut doc, 400.0, 0.0);
        doc.node_mut(blank).unwrap().alias = "   ".into();

        assert!(
            doc.problems()
                .iter()
                .all(|p| !matches!(p, Problem::DuplicateAlias(..))),
            "a blank alias names nothing, so it can clash with nothing"
        );
    }

    #[test]
    fn an_undeclared_effect_is_an_error() {
        let mut doc = Doc::default();
        let start = spoken(&mut doc, 0.0, 0.0);
        let ghost = sets(&mut doc, "paid", true);
        doc.add_edge(start, ghost);
        doc.node_mut(start).unwrap().end = true;
        doc.node_mut(ghost).unwrap().end = true;

        let problems = doc.problems();
        let unknown = Problem::UnknownVar(ghost, "paid".into());
        assert!(problems.contains(&unknown), "got {problems:?}");
        assert_eq!(unknown.severity(), Severity::Error);
        assert_eq!(unknown.node(), Some(ghost), "the row clicks through");

        doc.add_variable("paid");
        assert!(
            !doc.problems().contains(&unknown),
            "declaring it settles it"
        );
    }

    /// Builds a condition node checking `var`.
    fn checks(doc: &mut Doc, var: &str, want: bool) -> NodeId {
        let id = doc.add_node(0.0, 0.0);
        doc.node_mut(id).unwrap().kind = NodeKind::Condition;
        doc.set_condition(id, var, Some(Value::Bool(want)));
        id
    }

    #[test]
    fn a_check_needs_the_variable_to_exist_and_match() {
        let vars = VarState::from([("paid".to_string(), Value::Bool(true))]);
        let check = |var: &str, want: bool| {
            Condition {
                var: var.into(),
                want: Value::Bool(want),
            }
            .holds(&vars)
        };

        assert!(check("paid", true));
        assert!(!check("paid", false), "the wrong value");
        assert!(!check("nonexistent", true), "an undeclared variable");
        assert!(
            !check("nonexistent", false),
            "undeclared never holds, either way round — a broken file takes the false branch"
        );
    }

    #[test]
    fn the_walk_follows_the_branch_the_variables_choose() {
        let mut doc = Doc::default();
        doc.add_variable("paid");

        let start = spoken(&mut doc, 0.0, 0.0);
        let pay = sets(&mut doc, "paid", true);
        let gate = checks(&mut doc, "paid", true);
        let yes = spoken(&mut doc, 0.0, 0.0);
        let no = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(gate, yes);
        doc.add_edge(gate, no);
        doc.add_edge(pay, gate);

        let mut path = doc.start_walk(start);
        doc.advance(&mut path, gate);
        assert_eq!(path, vec![start, gate, no], "the default is false");

        // The instruction runs earlier on the same path, so the condition must see the
        // value it wrote rather than the declared default.
        let mut path = doc.start_walk(start);
        doc.advance(&mut path, pay);
        assert_eq!(
            path,
            vec![start, pay, gate, yes],
            "live state, not defaults"
        );
    }

    #[test]
    fn an_unwired_branch_stops_the_walk_on_the_condition() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let gate = checks(&mut doc, "paid", true);
        let yes = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(gate, yes);

        assert_eq!(
            doc.start_walk(gate),
            vec![gate],
            "it stops rather than taking the only edge it can see"
        );
    }

    #[test]
    fn step_back_crosses_a_condition() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let start = spoken(&mut doc, 0.0, 0.0);
        // Checks that `paid` is false, which it is by default, so this takes the true pin.
        let gate = checks(&mut doc, "paid", false);
        let landing = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(gate, landing);
        doc.add_edge(start, gate);

        let mut path = doc.start_walk(start);
        doc.advance(&mut path, gate);
        assert_eq!(path, vec![start, gate, landing]);

        assert!(doc.step_back(&mut path));
        assert_eq!(path, vec![start], "the condition went back with it");
    }

    #[test]
    fn the_first_edge_out_of_a_condition_is_the_true_branch() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let gate = checks(&mut doc, "paid", true);
        let yes = spoken(&mut doc, 0.0, 0.0);
        let no = spoken(&mut doc, 0.0, 0.0);

        doc.add_edge(gate, yes);
        doc.add_edge(gate, no);

        let plain = spoken(&mut doc, 0.0, 0.0);
        let onward = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(plain, onward);

        let branch = |to| doc.edges.iter().find(|e| e.to == to).map(|e| e.branch);
        assert_eq!(branch(yes), Some(true), "the happy path is drawn first");
        assert_eq!(branch(no), Some(false));
        assert_eq!(
            branch(onward),
            Some(false),
            "meaningless off any other kind, so it stays false"
        );
    }

    #[test]
    fn a_pin_with_nothing_on_it_is_a_warning() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let gate = checks(&mut doc, "paid", true);
        let yes = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(gate, yes);

        let problems = doc.problems();
        assert!(
            problems.contains(&Problem::UnwiredBranch(gate, false)),
            "got {problems:?}"
        );
        assert!(!problems.contains(&Problem::UnwiredBranch(gate, true)));
        assert_eq!(
            Problem::UnwiredBranch(gate, false).severity(),
            Severity::Warning
        );

        let no = spoken(&mut doc, 0.0, 0.0);
        doc.add_edge(gate, no);
        assert!(
            !doc.problems()
                .iter()
                .any(|p| matches!(p, Problem::UnwiredBranch(..))),
            "both pins wired"
        );
    }

    #[test]
    fn a_condition_that_checks_nothing_is_empty() {
        let mut doc = Doc::default();
        let bare = doc.add_node(0.0, 0.0);
        doc.node_mut(bare).unwrap().kind = NodeKind::Condition;

        let problems = doc.problems();
        assert!(problems.contains(&Problem::Empty(bare)));
        assert!(
            !problems
                .iter()
                .any(|p| matches!(p, Problem::UnwiredBranch(..))),
            "an empty condition is one fault, not three"
        );
    }

    #[test]
    fn removing_a_variable_takes_its_checks_too() {
        let mut doc = Doc::default();
        doc.add_variable("paid");
        let writes = sets(&mut doc, "paid", true);
        let reads = checks(&mut doc, "paid", true);

        doc.checkpoint();
        assert_eq!(doc.remove_variable("paid"), 2, "both sides counted");
        assert!(doc.node(reads).unwrap().conditions.is_empty());

        assert!(doc.undo());
        assert_eq!(doc.effect_of(writes, "paid"), Some(Value::Bool(true)));
        assert_eq!(doc.condition_of(reads, "paid"), Some(Value::Bool(true)));
    }

    #[test]
    fn an_undeclared_check_is_an_error() {
        let mut doc = Doc::default();
        let gate = checks(&mut doc, "paid", true);

        let unknown = Problem::UnknownVar(gate, "paid".into());
        assert!(doc.problems().contains(&unknown));

        doc.add_variable("paid");
        assert!(!doc.problems().contains(&unknown));
    }

    #[test]
    fn conditions_round_trip() {
        let mut original = Doc::default();
        original.add_variable("paid");
        let gate = checks(&mut original, "paid", true);
        let landing = spoken(&mut original, 0.0, 0.0);
        original.add_edge(gate, landing);

        let restored = Doc::from_json(&original.to_json()).expect("round trip");

        assert_eq!(restored.condition_of(gate, "paid"), Some(Value::Bool(true)));
        assert!(restored.edges[0].branch, "the pin survived");
    }

    #[test]
    fn an_edge_without_a_branch_loads() {
        let doc = Doc::from_json(
            r#"{"nodes":[{"id":0,"x":0.0,"y":0.0,"speaker":"","text":"Hm."},
                         {"id":1,"x":0.0,"y":0.0,"speaker":"","text":"Ah."}],
                "edges":[{"from":0,"to":1,"label":""}],"next_id":2}"#,
        )
        .expect("loads");
        assert!(!doc.edges[0].branch);
        assert!(doc.node(0).unwrap().conditions.is_empty());
    }

    /// The sample is the first thing anyone sees, so it has to keep demonstrating that a
    /// conversation can remember something and read it back.
    #[test]
    fn sample_gates_a_choice() {
        let doc = Doc::sample();
        let of_kind = |kind| {
            doc.nodes
                .iter()
                .find(|n| n.kind == kind)
                .expect("in sample")
        };

        let noted = of_kind(NodeKind::Instruction);
        let gate = of_kind(NodeKind::Condition);
        let var = &noted.effects[0].var;
        assert_eq!(
            doc.condition_of(gate.id, var),
            Some(Value::Bool(true)),
            "the condition reads what the instruction writes"
        );

        // Reaching the goodbye without asking about the pass takes the other branch.
        let start = doc.start_of(gate.id);
        let plain = doc.start_walk(start);
        assert!(
            !doc.vars_along(&plain)
                .get(var)
                .is_some_and(|v| *v == Value::Bool(true)),
            "nothing has set it yet"
        );

        let ends: Vec<NodeId> = doc
            .edges
            .iter()
            .filter(|e| e.from == gate.id)
            .map(|e| e.to)
            .collect();
        assert_eq!(
            ends.len(),
            2,
            "both pins wired, so neither play-through stops"
        );
    }

    #[test]
    fn an_instruction_that_sets_nothing_is_empty() {
        let mut doc = Doc::default();
        let bare = doc.add_node(0.0, 0.0);
        doc.node_mut(bare).unwrap().kind = NodeKind::Instruction;

        assert!(doc.problems().contains(&Problem::Empty(bare)));

        doc.add_variable("paid");
        doc.set_effect(bare, "paid", Some(Value::Bool(true)));
        assert!(!doc.problems().contains(&Problem::Empty(bare)));
    }

    #[test]
    fn effects_round_trip() {
        let mut original = Doc::default();
        original.add_variable("paid");
        let node = sets(&mut original, "paid", true);

        let restored = Doc::from_json(&original.to_json()).expect("round trip");

        assert_eq!(restored.effect_of(node, "paid"), Some(Value::Bool(true)));
    }

    #[test]
    fn a_node_without_effects_loads() {
        let doc = Doc::from_json(
            r#"{"nodes":[{"id":0,"x":0.0,"y":0.0,"speaker":"","text":"Hm."}],
                "edges":[],"next_id":1}"#,
        )
        .expect("loads");
        assert!(doc.node(0).unwrap().effects.is_empty());
    }

    /// Variables are document state, so they ride in the snapshot with everything else.
    /// This is the test that catches them being left out of it.
    #[test]
    fn undo_restores_a_deleted_variable() {
        let mut doc = Doc::default();
        doc.add_variable("has_key");
        doc.set_default("has_key", Value::Bool(true));

        doc.checkpoint();
        doc.remove_variable("has_key");
        assert!(doc.variable("has_key").is_none());

        assert!(doc.undo());
        assert_eq!(
            doc.variable("has_key").map(|v| v.default),
            Some(Value::Bool(true)),
            "the default came back with it"
        );

        assert!(doc.redo());
        assert!(doc.variable("has_key").is_none());
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
        let ids: Vec<NodeId> = (0..5).map(|i| doc.add_node(i as f32, 0.0)).collect();
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
        let expected: Vec<NodeId> = doc
            .nodes
            .iter()
            .filter(|n| n.speaker.contains("Innkeeper") || n.text.contains("inn"))
            .map(|n| n.id)
            .collect();

        assert!(hits.len() > 1, "matched both fields, not just one");
        assert_eq!(hits, expected, "ids come back in document order");
    }

    /// An alias is the handle a game build holds, so search has to reach one the author
    /// never typed into the dialogue itself.
    #[test]
    fn find_matches_an_alias() {
        let mut doc = Doc::default();
        let id = spoken(&mut doc, 0.0, 0.0);
        doc.node_mut(id).unwrap().alias = "blacksmith_intro".into();

        assert_eq!(
            doc.find("BLACKSMITH"),
            vec![id],
            "case-insensitive, like the rest"
        );
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
