# What's next

Ideas for taking palaver from "finished editor" to "portfolio piece", in the order I would do
them. **This is a wish list, not scope.** `CLAUDE.md` is the record of what exists and of what
was deliberately never built; nothing here is promised, and an item earns a section there only
once it ships.

The goal behind all of it: the editor inherits the flat, everything-at-one-level look of agentic
coding tools, and the work so far has been making it legible. Judge each item by how it reads
on screen, not by whether the tests pass.

## Already done

| Pass | What it did | Shots |
| --- | --- | --- |
| Tokens | One named colour and type scale in `src/theme.rs`; hue means one thing | `docs/shots/01-tokens-default.png`, `02-tokens/` |
| Layout | Status bar, sectioned inspector, overlays instead of stacked panels | `03-layout/` |
| Fit to view | Opens fitted (50% floor), `F` fits everything, flow tabs refit | `04-fit/` |
| Kind strip | Node kind as a coloured edge bar; hollow ring on a conversation's start | `05-before/` → `05-kinds/` |
| Command palette | Ctrl+K over `MENUS`; modal; one highlight | `06-palette/` |
| Play trail | Walked edges heavier, passed nodes outlined, spent choices dimmed | `07-trail/` |
| Merge | The flow map, tags, filters, references and screenplay export from `origin/main`, in the tokenised layout; a sample that shows them | `08-merge/`, `09-sample/` |
| Paper | Dark and light themes (live tokens, contrast-tested), Paper's shape language, elbow wires on one route function, two font settings with every face bundled, a Preferences card | `10-light/`, `11-paper/`, `12-wires/`, `13-fonts/` |

## Before any new feature

These matter more than a sixth feature, because they decide whether a stranger sees any of it.

1. **A README / case study.** The material exists: a diagnosis, then a before/after pair per
   pass. Write it as a story (the problem, what I changed, how I checked it), not a feature list.
   The screenshots script is itself part of the story: same states every run, so two runs
   either side of a change are an honest pair.
2. ~~**Bundle Inter.**~~ Done with the Paper pass: every face the Preferences card offers is embedded, and
   `Noto Serif` for notes is now bundled Newsreader italic.
3. **A release binary.** Nobody builds a GPUI app from a link. The screen recording is done
   (`tools/record.py`, `docs/palaver.gif`); a binary to run without building is not.
4. **Two known rough edges.**
   - A *selected* speech node's outline (`ACCENT`) and its strip (`KIND_SPEECH`) are nearly the
     same blue. The clean fix is a different `KIND_SPEECH`, but it is also `SPEAKER_DEFAULT`, so
     that retints every undeclared speaker's name; decide that on purpose.
   - An edge label that misses fitting by a pixel or two can wrap and leave an orphaned `?`.
     It is a gpui line-wrapper quirk; the width estimate in `edge_labels` is the lever.

## Features, in order

### 1. A trail while playing
Play only highlights the node you are standing on. Draw the walked path on the canvas (the
edges taken, the nodes passed through) and dim spent once-only choices.

- **Why first:** the best single moment in a recording, and nearly free. The play path is
  already the one copy of the play state (`Editor::play`), and `Doc::spent` already says which
  choices are used up, so this is drawing, not new state.
- **Watch for:** `edge_lines` is the one place edges are painted and already picks a colour per
  edge; the trail is another input to that choice, not a second pass. Selection must still
  outrank it, as it outranks a condition's branch colour today.
- **Done when:** a screenshot mid-play shows where you have been without reading the inspector.

### 2. The palette reaching nodes, flows and variables
Ctrl+K becomes the one way to go anywhere: type a speaker or a line to jump to a node, a flow
name to switch canvas, a variable to select it.

- **Why next:** it finishes something that already shipped, and it is cheap. Find
  (`Doc::find`, `jump_to`), flows (`show_flow`) and variables all exist.
- **Watch for:** `palette_entries` is a pure function over `MENUS` with unit tests, and its
  value is that the palette, the menus and the F1 sheet are one table. Keep commands and
  destinations as two sources feeding one list rather than putting destinations in `MENUS`, or
  the shortcut sheet fills with node names. Rows need a kind-colour glyph (`kind_style`) so a
  node result reads like a find result.
- **Done when:** one field replaces both Ctrl+F and the menu for keyboard users.

### 3. Tidy
One command lays a selected conversation out in clean columns.

- **Why:** a before/after pair on its own, and it fixes "I dragged things everywhere".
- **Watch for:** keep it a pure function in `doc.rs` with tests (layers by distance from the
  root, `GAP_X` between columns, `free_spot` to avoid collisions) and one `checkpoint()` so a
  single undo restores the whole layout. It must not move nodes on other flows, and it must
  leave a note where it is — a note is not part of the graph.
- **Done when:** a messy conversation becomes readable in one keystroke, and Ctrl+Z puts it
  back.

### 4. A minimap
A small overview in a corner of the canvas, drawn in the kind-strip colours, with the viewport
outlined and click-to-pan.

- **Why:** large graphs are where the flat-hierarchy problem is worst, and the strip colours
  are exactly what makes a thumbnail legible.
- **Watch for:** this is the costly one, because it needs its own hit test and a new rung in the
  mouse map (`on_mouse_down` is an ordered chain and the order is the specification). Anything
  that sits over the canvas has to be accounted for in `Editor::window_pos` or every hit test is
  wrong by its size; and it needs the overlay pattern (`.occlude()` *and* a mouse-down
  `stop_propagation`) so a click on it does not also start a marquee. Verify with the real
  pointer, as the overlays were.
- **Done when:** you can find your place in a 100-node graph without zooming out.

### 5. Edit in place
Double-click a node to type its line on the canvas instead of in the inspector.

- **Why:** the biggest interaction gain.
- **Watch for:** the riskiest item. `TextInput` is vendored third-party code and assumes a
  field in the flow of the layout, not one positioned in document space and scaled by `View`.
  Today double-click on empty canvas creates a node, so this needs a rung of its own in the
  mouse map, and `sync_fields` / `load_fields` must still see exactly one editor of a node's
  text. Prototype it for one kind (speech) before committing to all six.
- **Done when:** authoring a branch never leaves the canvas.

### Stretch: a light theme
Would prove the token layer is real, since every colour is already named for its meaning.
But the tokens are `const`, so it is a refactor to a runtime theme (a struct read through the
window), not a new file. Do it last, and only if the README wants a "the system holds" shot.

## Rules for working through this

- **Verify GUI changes by looking**, with `python3 -I tools/shots.py docs/shots/<pass>` before
  and after. Passing tests say nothing about whether it is usable. Both bugs found while
  finishing the palette (typed text black on near-black in every overlay field, and two rows
  highlighted at once) passed every test and were found only by looking.
- **Run the shots script, or anything that injects keys or clicks, only while the desktop is
  idle.** It acts on whatever window has focus.
- Keep `cargo fmt --check` and clippy at `-D warnings` clean, with and without
  `--features automation`.
