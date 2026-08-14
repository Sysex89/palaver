# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`papugai` — a node-graph dialog editor in the spirit of articy:draft: authoring branching dialogue/flow graphs, with entities, conditions and variables attached to nodes.

Status: skeleton only. `src/main.rs` is a hello-world, `Cargo.toml` has no dependencies yet. Everything below the Commands section is intent, not existing code — verify against the tree before assuming it exists.

## Stack

- Rust, edition 2024 (needs a recent toolchain; verified on rustc 1.97).
- GUI: **GPUI**, the framework Zed is built on (`gpui` on crates.io, currently 0.2.x). Its API still moves between releases — pin an exact version, and when an API doesn't match the docs, read the source in `~/.cargo/registry/src/*/gpui-*/` rather than guessing.
- On Linux GPUI renders through Vulkan/Blade and needs X11/Wayland + Vulkan dev packages present; a build failure in `blade-*` or `gpui` almost always means a missing system lib, not a code bug.

## Commands

```sh
cargo run                 # run the app
cargo build --release
cargo test                # all tests
cargo test <name>         # single test by (substring of) its path, e.g. cargo test graph::edge_cycle
cargo test -- --nocapture # see println!/dbg! output
cargo clippy --all-targets -- -D warnings
cargo fmt
```

GUI code can't be asserted on in `cargo test` — keep graph/document/serialization logic in plain modules with unit tests, and let the GPUI layer stay a thin view over them.

## Architecture notes

GPUI shapes the design more than most GUI crates, so follow its grain:

- State lives in `Entity<T>` handles owned by the app; you mutate through `cx.update`/`Context<T>` and call `cx.notify()` to trigger a re-render. Views are types implementing `Render`, returning an element tree built with the Tailwind-like styling builders (`div().flex().gap_2()`).
- Rendering is immediate-mode over retained state: `render` runs often and must be cheap and side-effect-free. Anything expensive (layout of a large graph, search indexes) is cached in the entity, not recomputed in `render`.
- Cross-view communication is by events (`cx.emit` + `cx.subscribe`) rather than shared mutable references.

Consequences for this project:

- The document model (nodes, edges, flow fragments, variables, entity definitions) belongs in its own module with no `gpui` import, so it stays testable and serializable on its own.
- The canvas is a custom element doing its own hit-testing and pan/zoom transform; don't try to express a graph canvas with nested `div`s.
- Undo/redo is a property of the document model (command/edit log), not of the views.
