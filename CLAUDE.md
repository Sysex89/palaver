# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`papugai` — a node-graph dialog editor in the spirit of articy:draft: branching dialogue graphs of speaker/text nodes connected by labelled choice edges.

Working today: pan and zoom, node drag, edge drag from a port, marquee multi-select and group move, select/delete node or edge, sidebar text editing, search and jump-to-node, Tab to quick-create a connected node, a validation problem list, marking a node as an ending, hub and jump nodes, snapshot undo/redo, new/open/save/save-as of `.dln` JSON files, autosave with crash recovery, and play mode.

The intended end state is bigger than that — see **Roadmap** at the bottom. Everything outside that section describes code that exists; the roadmap is the only forward-looking part of this file.

## Stack

- Rust, edition 2024 (verified on rustc 1.97).
- GUI: **GPUI** (`gpui` 0.2.2), the framework Zed is built on. Its API still moves between releases — the version is pinned, and when an API doesn't match the docs, read the source in `~/.cargo/registry/src/*/gpui-*/` rather than guessing.
- On Linux GPUI renders through Vulkan/Blade and needs X11/Wayland + Vulkan dev packages present; a build failure in `blade-*` or `gpui` almost always means a missing system lib, not a code bug.

## Commands

```sh
cargo run                 # run the app
cargo build --release
cargo test                # all tests (doc.rs only; GUI code is untested by design)
cargo test <name>         # single test by substring, e.g. cargo test undo_restores
cargo test -- --nocapture # see println!/dbg! output
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The tree is clean under both `cargo fmt --check` and clippy at `-D warnings`; keep it that way.

## Layout

Three files, no `lib.rs`:

- `src/doc.rs` — the document model. `Doc { nodes, edges, next_id }` plus geometry constants (`NODE_W/H`, `PORT_R`), hit-testing, undo and JSON IO. **Imports no `gpui`** and must stay that way; it is the only testable layer (43 unit tests at the bottom of the file). It also holds `View`, the canvas pan/zoom transform — not document state, and never serialized, but pure arithmetic that belongs where it can be tested.
- `src/main.rs` — `Editor`, the single `Render` view: menu bar, canvas pane, sidebar, all mouse/key handling.
- `src/text_input.rs` — a single-line `TextInput`, **vendored from gpui 0.2.2's `examples/input.rs`**; the header comment lists the local deltas. Treat it as third-party: on a gpui upgrade, re-vendor and re-apply the deltas rather than hand-patching. It holds the crate's only hand-written `impl Element` (`TextElement`, `text_input.rs:544`). `text_input::bind_keys(cx)` must be called at startup or the fields ignore every key.

## How the canvas is actually drawn

Hybrid, deliberately:

- Node boxes, port dots and edge labels are **absolutely-positioned `div`s** built in `Editor::canvas_pane`. Follow this when adding chrome — do not port them to a custom element.
- Only the edge lines and arrowheads go through gpui's `canvas()` helper + `PathBuilder` (`src/main.rs:605`), because there is no div-based way to draw a diagonal line.
- Hit-testing is **not** GPUI's; it is arithmetic in `doc.rs` (`hit_test`, `port_hit_test`, `edge_at`, `nodes_in_rect`) run from `Editor::on_mouse_down`. Ports overlap node boxes, so `port_hit_test` must be checked first (`doc.rs:100`). `hit_test` iterates in reverse so the topmost (last-painted) node wins.

### The mouse map

`on_mouse_down` is an ordered if-chain and the order *is* the specification; adding a gesture means picking its rung. Alt is checked before any hit test, so Alt+left pans over anything. Then: out-port → edge drag; node + shift → toggle it in the set, no drag; node already selected → keep the set and drag the group; node → select just it and drag; edge → select it; empty + double-click → create; empty → marquee.

**Middle-drag pans**, via `on_pan_down`, which the Alt+left branch also calls — the two gestures share one function so they cannot drift apart. `on_mouse_move` is button-agnostic, so panning needed nothing there, but every button that can start a drag must register `on_mouse_up` *and* `on_mouse_up_out`.

A plain click on empty canvas is a zero-size marquee that touches nothing, so clearing the selection falls out of the marquee commit in `on_mouse_up` rather than being its own branch.

### Coordinate spaces (the most common source of bugs)

Three of them:

1. **Window** — what mouse events carry.
2. **Canvas pane** — window minus `MENU_H`, since the pane sits under the menu bar. With `SIDEBAR_W`, these two constants are what bound the pane.
3. **Document** — `screen = doc * zoom + pan`. Everything in `doc.rs` is in this space.

`View::to_doc`/`to_screen` own step 3; `Editor::window_pos` owns step 2, and `doc_pos`/`screen_pos` compose the two. Go through those helpers, never the raw arithmetic. Separately, inside the `canvas()` paint closure coordinates are window-absolute, so `bounds.origin` is added back.

`pan` is in **screen** pixels, which is why `Drag::Pan` is the same arithmetic at every zoom. Lengths that start in document units (`NODE_W`, `PORT_R`, label widths, font sizes) need `View::scale`. Hit tolerances (`PORT_R`, `EDGE_HIT_TOL`) are deliberately *not* scaled — they live in document space, so targets get smaller on screen as you zoom out.

## Invariants

- **Checkpoint before you mutate.** `Doc` never checkpoints itself — the view calls `doc.checkpoint()` (`doc.rs:154`) *before* each mutation. It pushes a full snapshot (50 deep) *and* sets the dirty flag; it is the single choke point for both, and `dirty` is only cleared in `save()`. A mutation that skips it is both un-undoable and silently unsaved. `undo()` and `redo()` also set dirty — stepping through history counts as an edit.
- **`checkpoint()` clears the redo stack**, since a fresh edit branches history. `undo`/`redo` are symmetric: pop one stack, push the current snapshot onto the other. Only `undo` is capped at `UNDO_DEPTH`; the redo stack needs no cap of its own because it can only ever hold snapshots `undo` popped.
- **Checkpoints are placed to avoid empty undo steps.** Typing coalesces: `Editor::checkpoint_once` + the `text_dirty` flag give one checkpoint per editing session, and `text_dirty` resets in `load_fields` when the selection changes. Node drag checkpoints on the first `MouseMove`, not on mouse-down, so a plain click leaves no history entry.
- **`Selection` is `None | Nodes(Vec<u32>) | Edge(usize)`** — plural for nodes, singular for edges. `Selection::Edge(usize)` is a bare index into `doc.edges`, invalidated by any edge removal; the mitigation is convention: every structural change calls `select(Selection::None, cx)`, and `after_history` re-validates the node set against the document and drops anything else. Node ids survive a history step, so a node set does not need the same treatment — only the ids that vanished are filtered out.
- **`Selection::only_node()` is the gate on every single-node path** — `load_fields`, `sync_fields`, `on_branch`. A multi-selection returns `None` there, so it has no inspector fields to write and cannot half-edit a group. New per-node commands go through it too, not through `nodes()[0]`.
- **A group drag anchors the grabbed node and moves the rest by its delta** (`Drag::Node` in `on_mouse_move`), rather than storing a per-node offset. That way the node under the cursor never drifts, and the maths is the same for one node as for twenty.
- **A start is relative to the selection, not a property of the document.** `Doc::start_of(id)` walks incoming edges back to a node nothing links to, so a document holds as many independent conversations as the author draws and F5 plays whichever one the selection is standing in (Shift+F5 starts at the selected node itself). This is why `Problem::ManyStarts` was deleted: several roots is now the intended shape. `NoStart` stays, since a document where nothing can begin is still broken. `start_of` guards against cycles with a visited set and returns the node it started from when there is no top to find.
- **Play state is one `Option<Vec<u32>>`** on `Editor` — the path walked so far, current node last, which is all Back (`pop`) and Restart (`truncate(1)`) need. While it is `Some`, `play_panel` *replaces* the inspector arm in `sidebar` so the sidebar is never an editor and a player at once, and `replace_doc` clears it because node ids belong to the document that left. Play follows the canvas with `Editor::show` (centre only) rather than `jump_to`, deliberately: advancing must not rewrite the selection play was started from. The panel guards on `doc.node(current)`, since the node can be deleted from the canvas mid-play.
- **A `Jump`'s target is an ordinary `Edge`.** `NodeKind` is `Speech | Hub | Jump`, and the whole trick of it is that a jump introduces no second kind of link: `choices`, `problems` reachability, `start_of` and `nodes_in_rect` need no jump awareness because the edge is real, and the target is authored by dragging from the out-port like any other. The kind only changes how that edge is *drawn* — `edge_lines` and `edge_labels` both skip an edge leaving a jump (via `Doc::is_jump`, shared so the two cannot drift), and `node_box` names the target instead. A jump has exactly one target: `add_edge` refuses a second, and `kind_picker` drops all but the first outgoing edge when converting a node to one, because an edge that is neither drawn nor reachable would be a lie about the graph.
- **Kinds change what a node draws, never how big it is.** Hit-testing is arithmetic over `NODE_W`/`NODE_H` in `doc.rs`, so a hub drawn smaller than it is clicked would put the two out of step. Every kind keeps the same footprint.
- **`end: bool` stays orthogonal to `NodeKind`, deliberately.** An ending has text — "Farewell, traveller." is a `Speech` node that happens to terminate — so folding `End` into the enum would make "is an ending" and "is dialogue" mutually exclusive. `Hub` and `Jump` are exempt from `Problem::Empty` for the same reason `end` exempts a node from `DeadEnd`: neither is missing dialogue, because neither has any. **Only a `Speech` node has a speaker.** A hub's `text` is an optional label for it ("Anything else?"), a jump uses neither field, and `Node::speaker` never means anything but who is talking — which is what keeps **entities** from having to special-case a kind that nobody speaks at.
- **Play stops on a hub and walks through a jump.** A hub is a branch point, so `play_panel` shows its choices with no dialogue line above them and needs no special case to do it. A jump is an authoring convenience the player never experiences, so `start_play` and `play_advance` both push `doc.resolve_jump(..)` and a jump never enters the `play` path — which is what keeps Back and Restart working on speech nodes only. `resolve_jump` guards against a jump cycle with a visited set, like `start_of`.
- **`Problem` carries no message text.** `Doc::problems` returns an enum; the wording lives in `problem_text` in `main.rs`, which keeps view strings out of the model and lets the tests assert on variants instead of phrasing. Its `Severity` split is load-bearing, not decoration: a leaf node is an ordinary way for a conversation to end, so only a broken document (`DanglingEdge`, `NoStart`) is an `Error`. Anything new that would fire on a healthy graph is a `Warning`. `Node::end` is the opt-out: a node marked as an ending is skipped by the `DeadEnd` rule, so the list only names leaves the author has not vouched for. Keeping the wording in the view also means a variant can read differently per node kind without the model growing a variant for it — `DeadEnd` on a jump is worded "Jump with no target".
- **The three inspector inputs are observed as a group; `search_input` is not.** The `sync_fields` loop in `Editor::new` exists to write field content *into* the document — a search query is not document content, so it gets its own observer that only calls `cx.notify()`. Adding a non-document field to that loop would write it into the selected node.
- **`sync_fields` runs on every keystroke** (via `cx.observe` on the three `Entity<TextInput>` handles, wired in `Editor::new`), so it must early-return when the content is unchanged — otherwise `set_content` → notify → sync loops and spams checkpoints. It is paired with `load_fields`, which pushes doc content *into* the fields; `self.edited` tracks which selection the fields currently hold. There is no `cx.emit`/`cx.subscribe` anywhere in the crate.
- **`render` sets the window title**, but only when it changed (`src/main.rs:514`) — render runs constantly, so keep any other side effect out of it.
- **Key contexts partition the keyboard**: `"Editor"` on the canvas pane, `"TextInput"` on the field. `delete`/`backspace` mean "delete node" in one and "delete character" in the other; `escape` is bound in both, but to different actions and never at the same time: `FocusCanvas` in `TextInput` scope, `StopPlay` in `Editor` scope (a no-op when nothing is playing).
- `next_id` is serialized *and* part of the snapshot, so a loaded document can't collide with new nodes. It is monotonic only in the absence of undo: an undo rolls `next_id` back with everything else, so the next `add_node` reissues the id the undone node had. Ids are unique within a document at any instant, but not stable across a history step — don't cache one outside the `Doc`.
- The `.dln` file is pretty-printed serde JSON of `Doc`; `undo` and `dirty` are `#[serde(skip)]`, so loading starts with empty history and clean state. `with_default_extension` only appends `.dln` when the path has *no* extension, so "Save As `scene.json`" stays `.json`.
- A fresh `Editor` starts on `Doc::sample()` (a 5-node innkeeper conversation), not an empty document.

## Conventions

- **Adding a command touches four places**: the `actions!` list, the `MENUS` table, an `.on_action(cx.listener(...))` line in `canvas_pane`, and `cx.bind_keys` in `main()`. Menu items are `fn` pointers to the same handlers the keybindings call, so every action is reachable both ways.
- Menu dropdowns are `deferred` so they paint above the canvas, and they `stop_propagation` on mouse-down — otherwise the canvas's own mouse-down closes the menu before the item click lands. Drags register `on_mouse_up_out` alongside `on_mouse_up` so a release outside the pane still clears `self.drag`.
- File dialogs (`cx.prompt_for_paths`, `cx.prompt_for_new_path`) are driven with `cx.spawn(...).detach()`; the real return shape is a triple-nested `Ok(Ok(Some(..)))`. A failed load leaves the current document untouched. All IO errors are stringified (`Result<_, String>`) — there is no `anyhow`.
- **The crash copy is one fixed file and `Doc::write_backup` must never become `save`.** A timer in `Editor::new` writes `autosave_path()` (`temp_dir()/papugai-recovery.dln`) every `AUTOSAVE_EVERY` while `is_dirty()`, skipping a tick mid-drag, and stopping for the session after one failure (`autosave_off`) so a read-only temp directory does not report every 30s. `write_backup` deliberately leaves `dirty` set — clearing it would drop the `*` from the title and disarm the close guard — and it writes a `.tmp` then renames, because a kill mid-write is the event the file exists for. The backup holds the origin path beside the document (`Backup`), so recovery restores the file association too; nothing is ever written next to the user's document. `Editor::offer_recovery` runs once from `main()` and only an explicit "Discard" deletes the file — a cancelled prompt resolves to `Err` and must leave it alone.
- **`Editor::replace_doc` is the only way to swap the document**, and the only place `clear_backup()` is called on a swap. `reset` (File→New), the Open success arm, and recovery all go through it, so none of them can forget the backup, the view reset or `select(Selection::None)`. The two non-swap clear sites are `save_to` on success and the confirmed branch of `confirm_close`.
- **Anything that discards the document must go through `Editor::confirm_discard` first.** It resolves to `true` on "Discard", and all three paths that replace the document — File→New, File→Open, and the window-close hook — go through it. Any fourth one must too. Recovery is the one exemption and pays for it in kind: `offer_recovery` returns without replacing anything if the document went dirty while its prompt was up, because a gpui prompt does not hold the mouse on Linux and the canvas stays clickable underneath. The close path is a two-pass veto: `on_window_should_close` is synchronous but `window.prompt` is not, so the first pass returns `false` and spawns the prompt, "Discard" sets `Editor::closing` and calls `window.remove_window()`, and the second pass returns `true`. `on_window_closed` fires after the fact and cannot veto — it only quits.
- `// ponytail:` comments mark deliberate shortcuts with their ceiling and upgrade path (O(n) node lookup, snapshot undo, sync file IO). Respect the ceiling before optimising; keep the format when adding one.
- GUI code can't be asserted on in `cargo test`. Any new document logic goes in `doc.rs` with a unit test; `main.rs` stays a thin view over it.

## Roadmap — intent, not code

None of this exists yet. Grep before assuming otherwise. Roughly smallest-first; every one of them now costs a schema change.

**Ride-along with the first schema change: random node ids.** `next_id` is a shared counter, so two branches that each add a node mint the same id, and git merges that cleanly into a document with duplicate ids and edges pointing at the wrong node. It is the only *silent* corruption `.dln` files have; `.gitattributes` currently blocks the auto-merge outright, and this is what lets that be lifted. Replace the counter with `RandomState::new().hash_one(..)` (stdlib, OS-seeded, no dependency) and a `while self.node(id).is_some()` retry, widening `Node::id` to `u64`. Cheap because JSON does not distinguish `u32` from `u64` and an unknown `next_id` field is ignored on load, so existing files keep opening with no migration. It also deletes the caveat in **Invariants** about ids not being stable across a history step — with no counter to roll back, `restore` leaves ids alone. Two casualties to handle: the `Node {id}` fallback label (`main.rs:1556`) must switch to the node's index or it prints a 19-digit number, and a merge can then produce a duplicate edge that `add_edge` would have refused, which wants a `Problem::DuplicateEdge` warning. Do **not** also sort nodes by id on write — paint order is insertion order and `hit_test` reverse-iterates it, so sorting silently reshuffles z-order on reload.

**Entities** — speakers as first-class records rather than the free-text `Node::speaker` string. An `entities` vec on `Doc` with its own id space; `Node::speaker` becomes an optional id, keeping the string as a fallback for un-migrated files. Watch the name collision with `gpui::Entity<T>` — call the document type `Character` or `Actor`, or `doc.rs` types become unreadable at the `main.rs` call sites. Only `NodeKind::Speech` carries a speaker, so the other kinds need no story here.

**Variables** — global flags/counters the dialogue reads and writes. A `variables` vec on `Doc`, typed (bool/int/string), plus per-node effects. Pure model work, fully unit-testable, but inert on its own — it only becomes visible through **conditions** and **play mode**.

**Conditions** on edges — a guard expression gating whether a choice is offered. Extends `Edge` with an optional expression, needs an evaluator over the variable set (`fn eval(&self, vars: &VarState) -> bool`) with tests, and a canvas affordance so a guarded edge reads differently.

**Flow fragments** — nested subgraphs, turning a `Doc` into a tree of graphs rather than one flat node list. Deliberately last: it exists to solve "this graph is too big", and *Jump* nodes plus search solve that far more cheaply. Still the largest change when it comes — it touches ids (unique per fragment or globally?), the `Selection` enum, `View` state (one per fragment), and the JSON format. Design it before starting; snapshot undo over the whole tree stays fine.

### Out of scope

**Templates** — articy:draft's central idea is user-defined property schemas: every entity and node type gets an editable template with its own fields, constraints and read-only properties. Our fixed `Node { speaker, text }` is the opposite of that, and adopting it means a dynamic property bag plus a schema editor — a bigger project than this entire roadmap combined. Named here so nobody half-builds it by accident while adding a field.

Also considered and rejected as out of proportion to a local single-file editor: localisation views and translation, multi-user collaboration and version control, AI and voice synthesis, game-engine import plugins, the vector Location editor, the Word-like Document view, and the plugin/macro SDK.

When a roadmap item ships, move its description into the sections above and delete it from here.
