# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`papugai` — a node-graph dialog editor in the spirit of articy:draft: branching dialogue graphs of speaker/text nodes connected by labelled choice edges.

Working today: pan, node drag, edge drag from a port, select/delete node or edge, sidebar text editing, snapshot undo/redo, new/open/save/save-as of `.dln` JSON files.

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

- `src/doc.rs` — the document model. `Doc { nodes, edges, next_id }` plus geometry constants (`NODE_W/H`, `PORT_R`), hit-testing, undo and JSON IO. **Imports no `gpui`** and must stay that way; it is the only testable layer (17 unit tests at the bottom of the file).
- `src/main.rs` — `Editor`, the single `Render` view: menu bar, canvas pane, sidebar, all mouse/key handling.
- `src/text_input.rs` — a single-line `TextInput`, **vendored from gpui 0.2.2's `examples/input.rs`**; the header comment lists the local deltas. Treat it as third-party: on a gpui upgrade, re-vendor and re-apply the deltas rather than hand-patching. It holds the crate's only hand-written `impl Element` (`TextElement`, `text_input.rs:544`). `text_input::bind_keys(cx)` must be called at startup or the fields ignore every key.

## How the canvas is actually drawn

Hybrid, deliberately:

- Node boxes, port dots and edge labels are **absolutely-positioned `div`s** built in `Editor::canvas_pane` (`src/main.rs:577`). Follow this when adding chrome — do not port them to a custom element.
- Only the edge lines and arrowheads go through gpui's `canvas()` helper + `PathBuilder` (`src/main.rs:605`), because there is no div-based way to draw a diagonal line.
- Hit-testing is **not** GPUI's; it is arithmetic in `doc.rs` (`hit_test`, `port_hit_test`, `edge_at`) run from `Editor::on_mouse_down`. Ports overlap node boxes, so `port_hit_test` must be checked first (`doc.rs:100`). `hit_test` iterates in reverse so the topmost (last-painted) node wins.

### Coordinate spaces (the most common source of bugs)

Three of them, converted only through the helpers at `src/main.rs:330-342`:

1. **Window** — what mouse events carry.
2. **Canvas pane** — window minus `MENU_H`, since the pane sits under the menu bar.
3. **Document** — canvas minus `pan`. Everything in `doc.rs` is in this space.

`doc_pos` goes window → document, `screen_pos` goes document → canvas. Separately, inside the `canvas()` paint closure coordinates are window-absolute, so `bounds.origin` is added back (`src/main.rs:610`). Change `MENU_H` and hit-testing silently shifts. There is no zoom factor yet, so screen↔doc is pure translation.

## Invariants

- **Checkpoint before you mutate.** `Doc` never checkpoints itself — the view calls `doc.checkpoint()` (`doc.rs:154`) *before* each mutation. It pushes a full snapshot (50 deep) *and* sets the dirty flag; it is the single choke point for both, and `dirty` is only cleared in `save()`. A mutation that skips it is both un-undoable and silently unsaved. `undo()` and `redo()` also set dirty — stepping through history counts as an edit.
- **`checkpoint()` clears the redo stack**, since a fresh edit branches history. `undo`/`redo` are symmetric: pop one stack, push the current snapshot onto the other. Only `undo` is capped at `UNDO_DEPTH`; the redo stack needs no cap of its own because it can only ever hold snapshots `undo` popped.
- **Checkpoints are placed to avoid empty undo steps.** Typing coalesces: `Editor::checkpoint_once` + the `text_dirty` flag give one checkpoint per editing session, and `text_dirty` resets in `load_fields` when the selection changes. Node drag checkpoints on the first `MouseMove`, not on mouse-down, so a plain click leaves no history entry.
- **`Selection::Edge(usize)` is a bare index into `doc.edges`**, invalidated by any edge removal. The mitigation is convention: every structural change calls `select(None, cx)`, and `on_undo` re-validates a node selection and drops anything else.
- **`sync_fields` runs on every keystroke** (via `cx.observe` on the three `Entity<TextInput>` handles, wired in `Editor::new`), so it must early-return when the content is unchanged — otherwise `set_content` → notify → sync loops and spams checkpoints. It is paired with `load_fields`, which pushes doc content *into* the fields; `self.edited` tracks which selection the fields currently hold. There is no `cx.emit`/`cx.subscribe` anywhere in the crate.
- **`render` sets the window title**, but only when it changed (`src/main.rs:514`) — render runs constantly, so keep any other side effect out of it.
- **Key contexts partition the keyboard**: `"Editor"` on the canvas pane, `"TextInput"` on the field. `delete`/`backspace` mean "delete node" in one and "delete character" in the other; `escape` exists only in `TextInput` scope and routes focus back via the `FocusCanvas` action.
- `next_id` is serialized *and* part of the snapshot, so a loaded document can't collide with new nodes. It is monotonic only in the absence of undo: an undo rolls `next_id` back with everything else, so the next `add_node` reissues the id the undone node had. Ids are unique within a document at any instant, but not stable across a history step — don't cache one outside the `Doc`.
- The `.dln` file is pretty-printed serde JSON of `Doc`; `undo` and `dirty` are `#[serde(skip)]`, so loading starts with empty history and clean state. `with_default_extension` only appends `.dln` when the path has *no* extension, so "Save As `scene.json`" stays `.json`.
- A fresh `Editor` starts on `Doc::sample()` (a 5-node innkeeper conversation), not an empty document.

## Conventions

- **Adding a command touches four places**: `actions!` (`src/main.rs:16`), the `MENUS` table (`src/main.rs:28`), an `.on_action(cx.listener(...))` line in `canvas_pane`, and `cx.bind_keys` in `main()`. Menu items are `fn` pointers to the same handlers the keybindings call, so every action is reachable both ways.
- Menu dropdowns are `deferred` so they paint above the canvas, and they `stop_propagation` on mouse-down — otherwise the canvas's own mouse-down closes the menu before the item click lands. Drags register `on_mouse_up_out` alongside `on_mouse_up` so a release outside the pane still clears `self.drag`.
- File dialogs (`cx.prompt_for_paths`, `cx.prompt_for_new_path`) are driven with `cx.spawn(...).detach()`; the real return shape is a triple-nested `Ok(Ok(Some(..)))`. A failed load leaves the current document untouched. All IO errors are stringified (`Result<_, String>`) — there is no `anyhow`.
- `// ponytail:` comments mark deliberate shortcuts with their ceiling and upgrade path (O(n) node lookup, snapshot undo, sync file IO). Respect the ceiling before optimising; keep the format when adding one.
- GUI code can't be asserted on in `cargo test`. Any new document logic goes in `doc.rs` with a unit test; `main.rs` stays a thin view over it.

## Roadmap — intent, not code

None of this exists yet. Grep before assuming otherwise. Where each piece should land when built:

**Entities** — speakers as first-class records rather than the free-text `Node::speaker` string. An `entities` vec on `Doc` with its own id space; `Node::speaker` becomes an optional id, keeping the string as a fallback for un-migrated files. Watch the name collision with `gpui::Entity<T>` — call the document type `Character` or `Actor`, or `doc.rs` types become unreadable at the `main.rs` call sites.

**Variables** — global flags/counters the dialogue reads and writes. A `variables` vec on `Doc`, typed (bool/int/string), plus per-node effects. Pure model work, fully unit-testable.

**Conditions** on edges — a guard expression gating whether a choice is offered. Extends `Edge` with an optional expression, needs an evaluator over the variable set (`fn eval(&self, vars: &VarState) -> bool`) with tests, and a canvas affordance so a guarded edge reads differently.

**Flow fragments** — nested subgraphs, turning a `Doc` into a tree of graphs rather than one flat node list. The largest change: it touches ids (unique per fragment or globally?), the `Selection` enum, pan state (per fragment), and the JSON format. Design it before starting; snapshot undo over the whole tree stays fine.

**Zoom** — add a scale factor next to `pan` and thread it through the three coordinate helpers (`src/main.rs:330-342`) and nothing else. Node sizes come from `doc.rs` constants, so they scale for free.

When a roadmap item ships, move its description into the sections above and delete it from here.
