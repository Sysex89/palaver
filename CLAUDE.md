# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`papugai` — a node-graph dialog editor in the spirit of articy:draft: branching dialogue graphs of speaker/text nodes connected by labelled choice edges.

Working today: pan and zoom, node drag, edge drag from a port, marquee multi-select and group move, select/delete node or edge, sidebar text editing, search and jump-to-node, Tab to quick-create a connected node, snapshot undo/redo, new/open/save/save-as of `.dln` JSON files.

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

- `src/doc.rs` — the document model. `Doc { nodes, edges, next_id }` plus geometry constants (`NODE_W/H`, `PORT_R`), hit-testing, undo and JSON IO. **Imports no `gpui`** and must stay that way; it is the only testable layer (31 unit tests at the bottom of the file). It also holds `View`, the canvas pan/zoom transform — not document state, and never serialized, but pure arithmetic that belongs where it can be tested.
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
- **The three inspector inputs are observed as a group; `search_input` is not.** The `sync_fields` loop in `Editor::new` exists to write field content *into* the document — a search query is not document content, so it gets its own observer that only calls `cx.notify()`. Adding a non-document field to that loop would write it into the selected node.
- **`sync_fields` runs on every keystroke** (via `cx.observe` on the three `Entity<TextInput>` handles, wired in `Editor::new`), so it must early-return when the content is unchanged — otherwise `set_content` → notify → sync loops and spams checkpoints. It is paired with `load_fields`, which pushes doc content *into* the fields; `self.edited` tracks which selection the fields currently hold. There is no `cx.emit`/`cx.subscribe` anywhere in the crate.
- **`render` sets the window title**, but only when it changed (`src/main.rs:514`) — render runs constantly, so keep any other side effect out of it.
- **Key contexts partition the keyboard**: `"Editor"` on the canvas pane, `"TextInput"` on the field. `delete`/`backspace` mean "delete node" in one and "delete character" in the other; `escape` exists only in `TextInput` scope and routes focus back via the `FocusCanvas` action.
- `next_id` is serialized *and* part of the snapshot, so a loaded document can't collide with new nodes. It is monotonic only in the absence of undo: an undo rolls `next_id` back with everything else, so the next `add_node` reissues the id the undone node had. Ids are unique within a document at any instant, but not stable across a history step — don't cache one outside the `Doc`.
- The `.dln` file is pretty-printed serde JSON of `Doc`; `undo` and `dirty` are `#[serde(skip)]`, so loading starts with empty history and clean state. `with_default_extension` only appends `.dln` when the path has *no* extension, so "Save As `scene.json`" stays `.json`.
- A fresh `Editor` starts on `Doc::sample()` (a 5-node innkeeper conversation), not an empty document.

## Conventions

- **Adding a command touches four places**: the `actions!` list, the `MENUS` table, an `.on_action(cx.listener(...))` line in `canvas_pane`, and `cx.bind_keys` in `main()`. Menu items are `fn` pointers to the same handlers the keybindings call, so every action is reachable both ways.
- Menu dropdowns are `deferred` so they paint above the canvas, and they `stop_propagation` on mouse-down — otherwise the canvas's own mouse-down closes the menu before the item click lands. Drags register `on_mouse_up_out` alongside `on_mouse_up` so a release outside the pane still clears `self.drag`.
- File dialogs (`cx.prompt_for_paths`, `cx.prompt_for_new_path`) are driven with `cx.spawn(...).detach()`; the real return shape is a triple-nested `Ok(Ok(Some(..)))`. A failed load leaves the current document untouched. All IO errors are stringified (`Result<_, String>`) — there is no `anyhow`.
- **Anything that discards the document must go through `Editor::confirm_discard` first.** It resolves to `true` on "Discard", and all three paths that replace the document — File→New, File→Open, and the window-close hook — go through it. Any fourth one must too. The close path is a two-pass veto: `on_window_should_close` is synchronous but `window.prompt` is not, so the first pass returns `false` and spawns the prompt, "Discard" sets `Editor::closing` and calls `window.remove_window()`, and the second pass returns `true`. `on_window_closed` fires after the fact and cannot veto — it only quits.
- `// ponytail:` comments mark deliberate shortcuts with their ceiling and upgrade path (O(n) node lookup, snapshot undo, sync file IO). Respect the ceiling before optimising; keep the format when adding one.
- GUI code can't be asserted on in `cargo test`. Any new document logic goes in `doc.rs` with a unit test; `main.rs` stays a thin view over it.

## Roadmap — intent, not code

None of this exists yet. Grep before assuming otherwise. Roughly smallest-first; the first three need no schema change at all.

**Autosave and recovery** — same family as the window-close guard: a crash or a kill still loses everything since the last Ctrl+S. A loop of `cx.background_executor().timer(..)` inside a detached `cx.spawn`, waking every minute or so and writing only when `is_dirty()`.

Two traps. First, **do not call `Doc::save`** — it clears the dirty flag, so the title would stop showing `*` and the close guard would stop asking. Write `to_json()` to the file directly and leave `dirty` alone. Second, **do not write over the user's file**: an autosave they never asked for must not clobber the document on disk. Write a sidecar (`<name>.dln.autosave`, and a fixed path for a document with no `path` yet), delete it on a real save and on a confirmed close, and on launch offer to recover any that survived. Skip a tick while `self.drag.is_some()` so a save never lands mid-drag.

**Validation** — a problem list: dangling edges, unreachable nodes, dead ends, empty text, zero-or-many start nodes. Pure `doc.rs` returning a `Vec<Problem>`, which makes it the most testable item here. Surface it in the sidebar; clicking a problem should reuse `Editor::jump_to`, which search already built.

**Play mode** — walk the graph from a start node, showing the node's text and its outgoing edge labels as choices, click to advance. This turns a diagram into something you can actually judge, and it **works today with zero schema change**. The one decision is what "start" means — derive it (the node with no incoming edges) or mark it explicitly; deriving is enough until it isn't. Later this is where variables and conditions pay off, so keep the traversal a plain `doc.rs` function over `&Doc` rather than view state.

**Node kinds** — `kind: NodeKind` on `Node`, with at least *Hub* (a textless branch point) and *Jump* (links to a distant node without dragging an edge across the whole canvas). This is what keeps a large graph legible, far more cheaply than nesting it. The first schema change on this list, so settle it before **entities**: add the field with `#[serde(default)]` so existing `.dln` files keep loading, and decide whether play mode walks *through* a Hub or stops on it.

**Entities** — speakers as first-class records rather than the free-text `Node::speaker` string. An `entities` vec on `Doc` with its own id space; `Node::speaker` becomes an optional id, keeping the string as a fallback for un-migrated files. Watch the name collision with `gpui::Entity<T>` — call the document type `Character` or `Actor`, or `doc.rs` types become unreadable at the `main.rs` call sites.

**Variables** — global flags/counters the dialogue reads and writes. A `variables` vec on `Doc`, typed (bool/int/string), plus per-node effects. Pure model work, fully unit-testable, but inert on its own — it only becomes visible through **conditions** and **play mode**.

**Conditions** on edges — a guard expression gating whether a choice is offered. Extends `Edge` with an optional expression, needs an evaluator over the variable set (`fn eval(&self, vars: &VarState) -> bool`) with tests, and a canvas affordance so a guarded edge reads differently.

**Flow fragments** — nested subgraphs, turning a `Doc` into a tree of graphs rather than one flat node list. Deliberately last: it exists to solve "this graph is too big", and *Jump* nodes plus search solve that far more cheaply. Still the largest change when it comes — it touches ids (unique per fragment or globally?), the `Selection` enum, `View` state (one per fragment), and the JSON format. Design it before starting; snapshot undo over the whole tree stays fine.

### Out of scope

**Templates** — articy:draft's central idea is user-defined property schemas: every entity and node type gets an editable template with its own fields, constraints and read-only properties. Our fixed `Node { speaker, text }` is the opposite of that, and adopting it means a dynamic property bag plus a schema editor — a bigger project than this entire roadmap combined. Named here so nobody half-builds it by accident while adding a field.

Also considered and rejected as out of proportion to a local single-file editor: localisation views and translation, multi-user collaboration and version control, AI and voice synthesis, game-engine import plugins, the vector Location editor, the Word-like Document view, and the plugin/macro SDK.

When a roadmap item ships, move its description into the sections above and delete it from here.
