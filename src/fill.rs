//! What the placeholder generator is asked, and what is kept of its answer. Imports neither
//! gpui nor the engine: the engine (`llama.rs`, behind `--features generate`) only ever sees
//! the string [`Prompt::text`] builds, and the document only ever sees the line [`clean`]
//! returns. So everything here is testable in a default build, the way `doc.rs` is.
//!
//! A placeholder is bland, on-topic and one line long — enough to play a branching graph
//! and see whether the shape works, never enough to be mistaken for the writing pass.

use crate::doc::{Doc, Line, NodeId, NodeKind};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

/// The most tokens one line may run to. A line of dialogue that needs more is not a
/// placeholder.
pub const MAX_TOKENS: usize = 48;
/// Low enough to stay on topic, high enough that a reroll says something different.
pub const TEMPERATURE: f32 = 0.4;
/// Generations tried, each with the next seed, before an empty or runaway answer is given
/// up on and reported.
pub const ATTEMPTS: u64 = 3;
/// Longer than this and the model has run on into narration rather than a line.
const MAX_CHARS: usize = 200;

const SYSTEM: &str = "You fill in one missing line of a branching conversation in a video \
game. Reply with the missing line only: one short sentence, plain, in the speaker's voice, \
about what the scene is about. Do not bring up anything the scene has not. No stage \
directions, no quotation marks, no speaker name.";

/// Worked examples, shown as earlier exchanges before the real one: a small model copies
/// the shape of an answer far more reliably than it follows a description of one. Set
/// somewhere no document is likely to be, so their subject has nothing to leak into, and
/// written as dull as a placeholder should be.
const EXAMPLES: [(&str, &str); 2] = [
    (
        "Scene:\n\
         Smith: Back again? What broke this time?\n\
         (The player picks: \"My sword.\")\n\
         Smith: ???\n\
         (The player can then pick: \"How much?\" or \"Never mind.\")\n\n\
         Write Smith's line that replaces ???. It answers the player's choice \"My sword.\", \
         and leads into what comes after it.",
        "Leave it on the bench. I'll have it straight by morning.",
    ),
    (
        "Scene:\n\
         Smith: Leave it on the bench. I'll have it straight by morning.\n\
         (The player picks: ???)\n\
         Smith: Five silver, same as always.\n\n\
         Write the menu choice that replaces ???: a few words the player picks. It answers \
         \"Leave it on the bench. I'll have it straight by morning.\", and leads into what \
         comes after it.",
        "How much will it cost?",
    ),
];

/// The most turns of what came before that the prompt quotes. A small model drifts off on
/// whatever it read first, and the lines just before are the ones a line has to answer.
const MAX_BEFORE: usize = 10;
/// How far a walk through plumbing goes looking for the next thing a player sees.
const MAX_WALK: usize = 16;

/// One step of the scene, as the prompt quotes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub who: String,
    pub line: String,
    /// A choice label: picked from a menu, not said. Quoted as a pick, or a menu entry
    /// followed by the line it leads to reads as the player saying the same thing twice.
    pub picked: bool,
}

impl Turn {
    fn said(who: &str, line: &str) -> Turn {
        Turn {
            who: who.trim().to_string(),
            line: line.trim().to_string(),
            picked: false,
        }
    }

    fn pick(label: &str) -> Turn {
        Turn {
            who: PLAYER.to_string(),
            line: label.trim().to_string(),
            picked: true,
        }
    }
}

/// Everything the engine is told about one line, before it becomes text.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub premise: String,
    pub cast: Vec<String>,
    /// Root to the line, in the order a player reads it, cut to the last [`MAX_BEFORE`].
    pub before: Vec<Turn>,
    /// What the player meets after the line: the reply a choice leads to, or what follows a
    /// line — the next line, or the choices it offers. The line has to lead into it, and
    /// it is what keeps a small model on the scene's subject rather than its own.
    pub after: Vec<Turn>,
    /// Who says the line asked for: a speaker's name, `""` for a speech node nobody is
    /// assigned to, or [`PLAYER`] for a choice.
    pub who: String,
    /// A choice label rather than a line of dialogue. Not read off `who`: a cast may well
    /// have a character called "Player" who speaks ordinary lines.
    pub choice: bool,
    /// More than one path arrives here, and the line has to follow any of them.
    pub converges: bool,
    pub seed: u64,
}

/// Who a choice label is said by.
pub const PLAYER: &str = "Player";

/// What the generator would be asked for `line`, or `None` when there is nothing it may
/// write there: a node that is not a speech node (a hub's label is optional by design, and
/// every other kind holds no line), a choice out of a node the player does not stop at, or
/// a line that is gone. Whether a *human* line is in the way is the caller's question.
///
/// Siblings are never in each other's prompt: the context before is one path to the line,
/// and the context after is what *this* line leads to.
pub fn prompt(doc: &Doc, line: Line) -> Option<Prompt> {
    let (who, before, after, converges) = match line {
        Line::Node(id) => {
            let node = doc.node(id).filter(|n| n.kind == NodeKind::Speech)?;
            let incoming = doc.edges.iter().filter(|e| e.to == id).count();
            (
                node.speaker.trim().to_string(),
                context_into(doc, id),
                follow(doc, id),
                incoming > 1,
            )
        }
        Line::Edge(from, to) => {
            doc.node(from).filter(|n| n.player_text().is_some())?;
            doc.edge_between(from, to)?;
            (
                PLAYER.to_string(),
                context(doc, from, None),
                arrive(doc, to),
                false,
            )
        }
    };
    let skip = before.len().saturating_sub(MAX_BEFORE);
    Some(Prompt {
        premise: doc.premise.trim().to_string(),
        cast: doc.characters.iter().map(|c| c.name.clone()).collect(),
        before: before.into_iter().skip(skip).collect(),
        after,
        who,
        choice: matches!(line, Line::Edge(..)),
        converges,
        seed: doc.seed_of(line),
    })
}

/// What follows node `id` once it has been read: the choices it offers when there are
/// several, or the one way on — its label, if it has one, and what that leads to.
fn follow(doc: &Doc, id: NodeId) -> Vec<Turn> {
    let exits: Vec<_> = doc
        .edges
        .iter()
        .filter(|e| e.from == id && doc.node(e.to).is_some())
        .collect();
    let labelled: Vec<Turn> = exits
        .iter()
        .filter(|e| !e.label.trim().is_empty())
        .map(|e| Turn::pick(&e.label))
        .collect();
    match exits.as_slice() {
        _ if labelled.len() > 1 => labelled,
        [edge] => {
            let mut turns = labelled;
            turns.extend(arrive(doc, edge.to));
            turns
        }
        _ => Vec::new(),
    }
}

/// What a player meets on arriving at `id`: walks through plumbing the way play does
/// (taking the first way on, since a condition is not evaluated here) to the first node the
/// player stops at — its line, or a hub's choices.
fn arrive(doc: &Doc, id: NodeId) -> Vec<Turn> {
    let mut current = id;
    let mut seen = std::collections::HashSet::from([current]);
    for _ in 0..MAX_WALK {
        let Some(node) = doc.node(current) else {
            return Vec::new();
        };
        match node.kind {
            NodeKind::Speech if node.text.trim().is_empty() => return Vec::new(),
            NodeKind::Speech => return vec![Turn::said(&node.speaker, &node.text)],
            NodeKind::Hub => {
                return doc
                    .edges
                    .iter()
                    .filter(|e| e.from == current && !e.label.trim().is_empty())
                    .map(|e| Turn::pick(&e.label))
                    .collect();
            }
            NodeKind::Note => return Vec::new(),
            _ => {}
        }
        let Some(next) = doc.edges.iter().find(|e| e.from == current).map(|e| e.to) else {
            return Vec::new();
        };
        if !seen.insert(next) {
            return Vec::new();
        }
        current = next;
    }
    Vec::new()
}

/// The turns a player has read on arriving at `id`: the path to its parent, then the choice
/// that led here. Empty for a node nothing leads to.
fn context_into(doc: &Doc, id: NodeId) -> Vec<Turn> {
    let paths = shortest_paths(doc);
    let Some(&Some(parent)) = paths.get(&id) else {
        return Vec::new();
    };
    context(doc, parent, Some(id))
}

/// The turns along the shortest path from a root to `last`, inclusive, then the choice from
/// `last` to `next` when there is one. Plumbing — instructions, conditions, jumps — is
/// walked through silently, the way play walks through it; a condition is not evaluated,
/// since the path already says which pin it left by.
fn context(doc: &Doc, last: NodeId, next: Option<NodeId>) -> Vec<Turn> {
    let paths = shortest_paths(doc);
    let mut chain = vec![last];
    let mut seen = std::collections::HashSet::from([last]);
    while let Some(&Some(parent)) = paths.get(chain.last().unwrap()) {
        if !seen.insert(parent) {
            break;
        }
        chain.push(parent);
    }
    chain.reverse();
    if let Some(next) = next {
        chain.push(next);
    }

    let mut turns = Vec::new();
    for (i, &id) in chain.iter().enumerate() {
        let Some(node) = doc.node(id) else { continue };
        // The target itself is the line being asked for, not context.
        let is_target = next == Some(id);
        if !is_target && node.kind == NodeKind::Speech && !node.text.trim().is_empty() {
            turns.push(Turn::said(&node.speaker, &node.text));
        }
        let Some(&to) = chain.get(i + 1) else { break };
        let label = doc
            .edge_between(id, to)
            .map(|e| doc.edges[e].label.trim())
            .unwrap_or("");
        if doc.pauses_play(id) && !label.is_empty() {
            turns.push(Turn::pick(label));
        }
    }
    turns
}

/// Every node's parent on a shortest path from a root (`None` for a root), by
/// breadth-first search from all roots at once, roots and edges in document order — so
/// the context is the most direct way into a line, never a loop back round to it, and the
/// same document always gives the same answer.
fn shortest_paths(doc: &Doc) -> HashMap<NodeId, Option<NodeId>> {
    let mut parent: HashMap<NodeId, Option<NodeId>> = HashMap::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for root in doc.roots() {
        parent.insert(root, None);
        queue.push_back(root);
    }
    while let Some(id) = queue.pop_front() {
        for edge in doc.edges.iter().filter(|e| e.from == id) {
            if doc.node(edge.to).is_some() && !parent.contains_key(&edge.to) {
                parent.insert(edge.to, Some(id));
                queue.push_back(edge.to);
            }
        }
    }
    parent
}

/// The blank the model fills, as the scene shows it.
const BLANK: &str = "???";

impl Prompt {
    /// The prompt as Qwen's ChatML, with the empty think block its template writes when
    /// thinking is off — without it a Qwen3-family model spends the whole token budget
    /// reasoning in a `<think>` block and the line never comes.
    ///
    /// The scene is a script with a blank in it — the lines before, the blank, the lines
    /// after — and the ask names the line the blank answers. A small model told only to
    /// "write the next line" wandered off to the moon and the wine; one shown where the
    /// line has to land, and what it has to answer, mostly stays put.
    pub fn text(&self) -> String {
        let mut user = String::new();
        if !self.premise.is_empty() {
            user += &format!("Setting: {}\n", self.premise);
        }
        if !self.cast.is_empty() {
            user += &format!("Characters: {}.\n", self.cast.join(", "));
        }
        user += "\nScene:\n";
        if self.before.is_empty() {
            user += "(The conversation starts here.)\n";
        }
        for turn in &self.before {
            user += &quote(turn);
        }
        user += &match self.choice {
            true => format!("(The player picks: {BLANK})\n"),
            false => format!("{}: {BLANK}\n", speaker(&self.who)),
        };
        match self.after.as_slice() {
            [] => {}
            // Several picks after a line are the menu it offers, not a sequence.
            turns if turns.len() > 1 && turns.iter().all(|t| t.picked) => {
                let options: Vec<String> =
                    turns.iter().map(|t| format!("\"{}\"", t.line)).collect();
                user += &format!("(The player can then pick: {})\n", options.join(" or "));
            }
            turns => {
                for turn in turns {
                    user += &quote(turn);
                }
            }
        }

        user += "\n";
        user += &match self.choice {
            true => {
                format!("Write the menu choice that replaces {BLANK}: a few words the player picks")
            }
            false => match self.who.as_str() {
                "" => format!("Write the line that replaces {BLANK}"),
                who => format!("Write {who}'s line that replaces {BLANK}"),
            },
        };
        // The line it answers, named outright: the single strongest pull back on topic.
        user += &match self.before.last() {
            Some(turn) if turn.picked => {
                format!(". It answers the player's choice \"{}\"", turn.line)
            }
            Some(turn) if turn.who != self.who || self.choice => {
                format!(". It answers \"{}\"", turn.line)
            }
            _ => String::new(),
        };
        if !self.after.is_empty() {
            user += ", and leads into what comes after it";
        }
        user += ".";
        if self.converges {
            user += " Several paths lead to it, so it must make sense after any of them.";
        }
        let mut text = format!("<|im_start|>system\n{SYSTEM}<|im_end|>\n");
        for (ask, answer) in EXAMPLES {
            text += &format!(
                "<|im_start|>user\n{ask}<|im_end|>\n\
                 <|im_start|>assistant\n<think>\n\n</think>\n\n{answer}<|im_end|>\n"
            );
        }
        text + &format!(
            "<|im_start|>user\n{}<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n",
            user.trim()
        )
    }
}

/// One turn as a line of the script.
fn quote(turn: &Turn) -> String {
    match turn.picked {
        true => format!("(The player picks: \"{}\")\n", turn.line),
        false => format!("{}: {}\n", speaker(&turn.who), turn.line),
    }
}

/// A blank speaker still has to be quoted as someone.
fn speaker(who: &str) -> &str {
    if who.is_empty() { "Someone" } else { who }
}

/// What is kept of the model's answer: the first line, without a speaker name it echoed
/// back or the quotation marks it was told not to use. `None` for nothing, or for a
/// runaway that is not a line any more — the caller retries with the next seed.
pub fn clean(raw: &str, who: &str) -> Option<String> {
    let mut line = raw.trim().lines().next().unwrap_or("").trim();
    for name in [who, PLAYER, "Someone"] {
        if name.is_empty() {
            continue;
        }
        if let Some(rest) = strip_prefix_ci(line, name)
            && let Some(rest) = rest.trim_start().strip_prefix(':')
        {
            line = rest.trim();
            break;
        }
    }
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\''), ('‘', '’')] {
        if let Some(inner) = line
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        {
            line = inner.trim();
            break;
        }
    }
    let count = line.chars().count();
    (count > 0 && count <= MAX_CHARS).then(|| line.to_string())
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// The model file to load: `$PALAVER_MODEL` when set, then the first `*.gguf` in
/// `$XDG_DATA_HOME/palaver/models/` (`~/.local/share/...` when unset), then in `models/`
/// beside the executable, which is where a release archive puts it. Never downloaded: the
/// editor makes no network connections.
pub fn find_model() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PALAVER_MODEL") {
        return Some(PathBuf::from(path));
    }
    model_dirs().into_iter().find_map(|dir| first_gguf(&dir))
}

/// Where [`find_model`] looks, for the message that says where to put one.
pub fn model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(data) = data {
        dirs.push(data.join("palaver/models"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        dirs.push(dir.join("models"));
    }
    dirs
}

/// Sorted, so which file wins does not depend on the order the directory lists them in. A
/// vision projector (`mmproj-*`) sits beside some models and is not one.
fn first_gguf(dir: &std::path::Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "gguf"))
        .filter(|p| {
            !p.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mmproj"))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Doc, NodeKind};

    /// Innkeeper greets; the player picks one of two options; each leads to its own reply.
    fn branching() -> (Doc, [NodeId; 4]) {
        let mut doc = Doc::default();
        let greet = doc.add_node(0.0, 0.0);
        let room = doc.add_node(200.0, 0.0);
        let drink = doc.add_node(200.0, 200.0);
        let after = doc.add_node(400.0, 0.0);
        for (id, text) in [
            (greet, "Evening. Room or a drink?"),
            (room, ""),
            (drink, ""),
        ] {
            let node = doc.node_mut(id).unwrap();
            node.speaker = "Innkeeper".into();
            node.text = text.into();
        }
        doc.node_mut(after).unwrap().speaker = "Innkeeper".into();
        doc.add_edge(greet, room);
        doc.add_edge(greet, drink);
        doc.add_edge(room, after);
        doc.edges[0].label = "A room, please.".into();
        doc.edges[1].label = "Just a drink.".into();
        (doc, [greet, room, drink, after])
    }

    #[test]
    fn a_reply_follows_the_option_that_was_chosen_and_never_its_sibling() {
        let (doc, [_, room, drink, _]) = branching();
        let to_room = prompt(&doc, Line::Node(room)).unwrap();
        let last = to_room.before.last().unwrap();
        assert_eq!(
            (last.who.as_str(), last.line.as_str()),
            (PLAYER, "A room, please.")
        );
        assert!(!to_room.text().contains("Just a drink."));

        let to_drink = prompt(&doc, Line::Node(drink)).unwrap();
        assert_eq!(to_drink.before.last().unwrap().line, "Just a drink.");
        assert!(!to_drink.text().contains("A room, please."));
    }

    #[test]
    fn every_option_out_of_a_node_shares_one_context_ending_at_its_line() {
        let (mut doc, [greet, room, drink, _]) = branching();
        doc.edges[0].label.clear();
        doc.edges[1].label.clear();
        let a = prompt(&doc, Line::Edge(greet, room)).unwrap();
        let b = prompt(&doc, Line::Edge(greet, drink)).unwrap();
        assert_eq!(a.before, b.before);
        assert_eq!(a.before.last().unwrap().line, "Evening. Room or a drink?");
        assert!(a.choice);
    }

    #[test]
    fn an_option_is_asked_for_in_the_light_of_the_reply_it_leads_to() {
        let (mut doc, [greet, room, _, _]) = branching();
        doc.node_mut(room).unwrap().text = "Top of the stairs, mind the cat.".into();
        let asked = prompt(&doc, Line::Edge(greet, room)).unwrap();
        assert_eq!(
            asked.after.last().unwrap().line,
            "Top of the stairs, mind the cat."
        );
    }

    #[test]
    fn a_character_called_player_speaks_a_line_not_a_choice() {
        let (mut doc, [_, room, ..]) = branching();
        doc.node_mut(room).unwrap().speaker = PLAYER.into();
        let asked = prompt(&doc, Line::Node(room)).unwrap();
        assert!(!asked.choice);
        let ask = request(&asked);
        assert!(ask.contains("Player's line"));
        assert!(!ask.contains("menu choice"));
    }

    /// The real request, after the worked examples, which mention both kinds of line.
    fn request(prompt: &Prompt) -> String {
        let text = prompt.text();
        let at = text.rfind("<|im_start|>user").unwrap();
        text[at..].to_string()
    }

    #[test]
    fn the_worked_examples_come_before_the_request_and_are_answered() {
        let (doc, [_, room, ..]) = branching();
        let text = prompt(&doc, Line::Node(room)).unwrap().text();
        assert_eq!(text.matches("<|im_start|>user").count(), EXAMPLES.len() + 1);
        for (_, answer) in EXAMPLES {
            assert!(text.contains(&format!("</think>\n\n{answer}<|im_end|>")));
        }
    }

    #[test]
    fn plumbing_leaves_no_turn() {
        let (mut doc, [_, room, _, after]) = branching();
        doc.node_mut(room).unwrap().kind = NodeKind::Instruction;
        let asked = prompt(&doc, Line::Node(after)).unwrap();
        let lines: Vec<&str> = asked.before.iter().map(|t| t.line.as_str()).collect();
        assert_eq!(lines, ["Evening. Room or a drink?", "A room, please."]);
    }

    #[test]
    fn only_speech_and_player_read_choices_are_targets() {
        let (mut doc, [greet, room, drink, _]) = branching();
        doc.node_mut(drink).unwrap().kind = NodeKind::Hub;
        assert!(prompt(&doc, Line::Node(drink)).is_none());
        doc.node_mut(greet).unwrap().kind = NodeKind::Condition;
        assert!(prompt(&doc, Line::Edge(greet, room)).is_none());
        let note = doc.add_node(0.0, 400.0);
        doc.node_mut(note).unwrap().kind = NodeKind::Note;
        assert!(prompt(&doc, Line::Node(note)).is_none());
    }

    #[test]
    fn the_first_line_has_no_context_and_says_so() {
        let (doc, [greet, ..]) = branching();
        let asked = prompt(&doc, Line::Node(greet)).unwrap();
        assert!(asked.before.is_empty());
        assert!(asked.text().contains("The conversation starts here"));
        assert!(asked.text().contains("Innkeeper's line"));
    }

    #[test]
    fn a_node_seeds_with_its_id_until_rerolled() {
        let (mut doc, [greet, ..]) = branching();
        assert_eq!(prompt(&doc, Line::Node(greet)).unwrap().seed, greet);
        doc.write_placeholder(Line::Node(greet), "Hello.", crate::doc::reroll(greet));
        assert_eq!(
            prompt(&doc, Line::Node(greet)).unwrap().seed,
            crate::doc::reroll(greet)
        );
    }

    #[test]
    fn the_prompt_carries_the_premise_and_turns_thinking_off() {
        let (mut doc, [_, room, ..]) = branching();
        doc.premise = "A tavern on the mountain road.".into();
        let text = prompt(&doc, Line::Node(room)).unwrap().text();
        assert!(text.contains("Setting: A tavern on the mountain road."));
        assert!(text.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn a_loop_back_to_a_node_is_not_its_context() {
        let (mut doc, [_, room, _, after]) = branching();
        // `after` jumps back to the greeting: the greeting is still a root's line, and its
        // context is still empty rather than the loop that returns to it.
        let back = doc.add_node(600.0, 0.0);
        doc.add_edge(after, back);
        doc.node_mut(back).unwrap().kind = NodeKind::Jump;
        doc.add_edge(back, room);
        let asked = prompt(&doc, Line::Node(room)).unwrap();
        assert_eq!(asked.before.len(), 2);
        assert!(asked.converges);
    }

    #[test]
    fn clean_keeps_one_bare_line() {
        assert_eq!(
            clean("  Hello there.  ", "Innkeeper").as_deref(),
            Some("Hello there.")
        );
        assert_eq!(
            clean("Innkeeper: Hello.", "Innkeeper").as_deref(),
            Some("Hello.")
        );
        assert_eq!(
            clean("innkeeper : Hello.", "Innkeeper").as_deref(),
            Some("Hello.")
        );
        assert_eq!(clean("\"Hello.\"", "Innkeeper").as_deref(), Some("Hello."));
        assert_eq!(clean("“Hello.”", "").as_deref(), Some("Hello."));
        assert_eq!(clean("First.\nSecond.", PLAYER).as_deref(), Some("First."));
        assert_eq!(clean("Player: Sure.", PLAYER).as_deref(), Some("Sure."));
        assert_eq!(clean("   \n", "Innkeeper"), None);
        assert_eq!(clean(&"a".repeat(MAX_CHARS + 1), "Innkeeper"), None);
        // A name that is only the start of a word is not an echoed speaker.
        assert_eq!(
            clean("Innkeepers rarely sleep.", "Innkeeper").as_deref(),
            Some("Innkeepers rarely sleep.")
        );
    }

    #[test]
    fn a_line_is_asked_for_with_the_choices_it_offers_after_it() {
        let (doc, [greet, ..]) = branching();
        let asked = prompt(&doc, Line::Node(greet)).unwrap();
        let after: Vec<&str> = asked.after.iter().map(|t| t.line.as_str()).collect();
        assert_eq!(after, ["A room, please.", "Just a drink."]);
        assert!(
            asked
                .text()
                .contains("(The player can then pick: \"A room, please.\" or \"Just a drink.\")")
        );
    }

    #[test]
    fn a_line_with_one_way_on_is_asked_for_with_what_it_leads_to() {
        let (mut doc, [_, room, _, after]) = branching();
        doc.node_mut(after).unwrap().text = "Breakfast is at dawn.".into();
        let asked = prompt(&doc, Line::Node(room)).unwrap();
        assert_eq!(
            asked.after,
            vec![Turn::said("Innkeeper", "Breakfast is at dawn.")]
        );
        assert!(
            asked
                .text()
                .contains("It answers the player's choice \"A room, please.\"")
        );
    }

    #[test]
    fn what_comes_after_a_choice_is_its_own_reply_never_a_siblings() {
        let (mut doc, [greet, room, drink, _]) = branching();
        doc.node_mut(room).unwrap().text = "Top of the stairs.".into();
        doc.node_mut(drink).unwrap().text = "Ale or cider?".into();
        let text = prompt(&doc, Line::Edge(greet, room)).unwrap().text();
        assert!(text.contains("Top of the stairs."));
        assert!(!text.contains("Ale or cider?"));
        assert!(!text.contains("Just a drink."));
    }

    #[test]
    fn plumbing_after_a_line_is_walked_through() {
        let (mut doc, [_, room, _, after]) = branching();
        let set = doc.add_node(300.0, 0.0);
        doc.node_mut(set).unwrap().kind = NodeKind::Instruction;
        doc.edges.retain(|e| !(e.from == room && e.to == after));
        doc.add_edge(room, set);
        doc.add_edge(set, after);
        doc.node_mut(after).unwrap().text = "Breakfast is at dawn.".into();
        let asked = prompt(&doc, Line::Node(room)).unwrap();
        assert_eq!(asked.after.last().unwrap().line, "Breakfast is at dawn.");
    }

    #[test]
    fn only_the_last_turns_before_are_quoted() {
        let mut doc = Doc::default();
        let mut last = doc.add_node(0.0, 0.0);
        doc.node_mut(last).unwrap().text = "Line 0".into();
        for i in 1..=MAX_BEFORE + 5 {
            let next = doc.add_node(0.0, 0.0);
            doc.node_mut(next).unwrap().text = format!("Line {i}");
            doc.add_edge(last, next);
            last = next;
        }
        let blank = doc.add_node(0.0, 0.0);
        doc.add_edge(last, blank);
        let asked = prompt(&doc, Line::Node(blank)).unwrap();
        assert_eq!(asked.before.len(), MAX_BEFORE);
        assert_eq!(
            asked.before.last().unwrap().line,
            format!("Line {}", MAX_BEFORE + 5)
        );
    }
}
