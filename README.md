# palaver

A node-graph editor for branching game dialogue, in the spirit of articy:draft, written in Rust
with GPUI, the UI framework Zed is built on.

![palaver mid-play in the light theme: elbow wires, the walked path drawn heavier, the inspector showing the current line](docs/shots/12-wires/L13-play-trail.png)

## What it is

You write a conversation as a graph. Speech nodes hold a speaker and a line, the edges between
them are the player's choices, and a few other node kinds carry the logic: hubs to branch
from, conditions and instructions to read and set variables, jumps to go elsewhere, and
notes for the author. Press F5 and the editor plays the conversation right there, walking the
graph the way a game would.

Around that sit the things a real project needs: document-wide variables, a cast of characters,
several canvases (flows) in one file, tags, full undo, and a problems list for dead ends and
broken links. Documents are plain JSON (`.dln`), and the lines a player reads export to Fluent
(`.ftl`) for translation, or to a numbered Markdown screenplay.

## Making it legible

The first working version looked like a lot of tools written with an AI coding agent: every
label the same size and grey, every panel at the same weight, nothing telling you where to
look. It all worked, and it was hard to read. Most of the later work was fixing that, one pass
at a time, each checked with a before and after screenshot.

**Colour and type.** Every colour and text size now lives in one file of named tokens. Surfaces
step up in brightness by elevation, text comes in three tiers checked against WCAG AA contrast,
and each hue means exactly one thing. Before this, amber meant an ending, a condition, a note
and a number.

<table><tr>
<td><img src="docs/shots/00-before-default.png" alt="Before the tokens pass"></td>
<td><img src="docs/shots/01-tokens-default.png" alt="After the tokens pass"></td>
</tr><tr><td align="center">before</td><td align="center">after</td></tr></table>

**Layout.** The right sidebar used to hold the editor fields, the search box, the problems list
and the tips, all stacked at the same weight. Now it is only an inspector for the selection,
split into sections. Facts about the whole document moved to a status bar, and things you use
in bursts (find, problems, the shortcut sheet) became overlays that are only there while open.

![Three states of the editor, before (top row) and after (bottom row) the layout pass](docs/shots/compare-layout.png)

**Fit to view.** A document now opens fitted to the window. It never opens below 50%, so the
sample shows where its conversation starts instead of the whole graph at an unreadable 26%.
Pressing F gives you the full overview when you ask for it.

<img src="docs/shots/04-fit/00-open.png" alt="The sample document as it opens, at 50%" width="600">

**Node kinds.** Each node got a coloured bar down its left edge for its kind, so a speech node, a
condition and an instruction are different at a glance. A node that nothing links to gets a
small hollow ring, which marks where a conversation begins.

<table><tr>
<td><img src="docs/shots/05-before/07-condition.png" alt="Nodes before the kind strip"></td>
<td><img src="docs/shots/05-kinds/07-condition.png" alt="Nodes with the kind strip"></td>
</tr><tr><td align="center">before</td><td align="center">after</td></tr></table>

**Command palette.** Ctrl+K opens a search over every command. It reads from the same table as
the menus and the F1 sheet, so the three can never disagree about a name or a shortcut.

<img src="docs/shots/06-palette/22-palette-query.png" alt="The command palette filtered to 'zoom'" width="600">

**Play trail.** While a conversation plays, the canvas shows where you have been: the edges you
took are drawn heavier, the nodes you passed are outlined, and once-only choices you have used
up are dimmed. The screenshot at the top is one of these.

**Finding your way.** A graph gets big, so there are a few ways to get around it. Search takes
filters (`kind:cond`, `speaker:player`, `tag:act1`, `sets:gold`), and when the box is empty it
lists your bookmarks, which are just the nodes you gave an alias. Every variable, character and
flow shows where it is used. The flow map, on Ctrl+M, draws one box per canvas with an arrow
for each pair a jump connects.

<table><tr>
<td><img src="docs/shots/12-wires/28-search-bookmarks.png" alt="Search with nothing typed, listing bookmarks"></td>
<td><img src="docs/shots/12-wires/26-flow-map.png" alt="The flow map: main jumps to road"></td>
</tr></table>

**Paper, and your own type.** The last pass turned the look into a choice. There are two themes,
Dark and a light one called Paper, and by default they follow the desktop. Wires are elbows
now, so a hub's choices leave on one trunk and each label sits on a run of its own. Ctrl+, opens
Preferences, where the UI font and the text font are separate settings: the text font is what a
character says, so it can be Newsreader or the Courier of a screenplay without touching the
names of your variables. Every font is bundled, so it looks the same on any machine.

<table><tr>
<td><img src="docs/shots/13-fonts/L31-prefs.png" alt="The Preferences card in the light theme"></td>
<td><img src="docs/shots/13-fonts/32-text-courier.png" alt="Dark theme with the text font set to Courier Prime"></td>
</tr></table>

## How I checked it

`tools/shots.py` builds the editor, drives it into a fixed list of states over a small automation
socket, and screenshots each one. The states are the same every run, so two runs on either side
of a change make an honest before and after pair, and that is where every image above comes from.

The pointer-heavy parts (clicking an elbow wire, dragging a new one out of a port, Ctrl+, and a
click on a font chip) were checked with real keyboard and mouse events, not just the scripted
socket. This mattered more than the tests did. Two real bugs passed every test: text typed into any
overlay field was black on near-black, and the palette could highlight two rows at once (one
under the mouse, one from the arrow keys). Both were found only by looking at the screenshots.

## Build and run

```sh
cargo run --release
```

On Linux GPUI renders through Vulkan, so you need the Vulkan and X11 or Wayland development
packages installed. If the build fails inside a `blade-*` crate or `gpui` itself, it is almost
always a missing system library rather than a problem in this code.

The editor opens on a small sample conversation with one of every node kind, a second flow, tags and two bookmarks, so there is something to click on.

## Not built yet

- There is no release binary and no screen recording yet, so for now you have to build it.

[`CLAUDE.md`](CLAUDE.md) explains the architecture and lists what was deliberately left out of
scope. [`docs/next.md`](docs/next.md) is what I would do next.
