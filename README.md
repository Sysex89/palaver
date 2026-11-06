# palaver

A node-graph editor for branching game dialogue, a bit like articy:draft. Written in Rust with GPUI.

![A 40-second run: playing a conversation, searching, the flow map, the command palette, then Preferences and the dark theme](docs/palaver.gif)

The same clip as [an mp4](docs/palaver.mp4).

## What it is

You write a conversation as a graph. Speech nodes hold a speaker and a line, and the edges
between them are what the player can choose. The other node kinds do the logic: hubs to branch
from, conditions and instructions to check and set variables, jumps to go somewhere else, and
notes for yourself. Press F5 and it plays the conversation in the editor, the way a game would.

There's also the usual project stuff: variables, a cast of characters, several canvases (flows)
in one file, tags, undo, and a problems list that finds dead ends and broken links. Files are
plain JSON (`.dln`). The lines a player reads can be exported to Fluent (`.ftl`) for translation,
or to a numbered Markdown screenplay.

## Making it readable

The first version worked, but it was hard to look at. Every label was the same size and the
same grey, and every panel sat at the same level. I spent most of the time after that fixing it,
one pass at a time, with a before and after screenshot for each.

**Colour and type.** All colours and text sizes live in one file of named tokens. Surfaces get
lighter the higher they sit, text has three tiers that pass WCAG AA contrast, and each hue
means one thing. Before, amber meant an ending, a condition, a note and a number.

<table><tr>
<td><img src="docs/shots/00-before-default.png" alt="Before the tokens pass"></td>
<td><img src="docs/shots/01-tokens-default.png" alt="After the tokens pass"></td>
</tr><tr><td align="center">before</td><td align="center">after</td></tr></table>

**Layout.** The right sidebar used to hold the editor fields, the search box, the problems list
and the tips, all stacked the same way. Now it only shows the selection, in sections. Anything
about the whole document went to a status bar, and find, problems and the shortcut sheet are
overlays that only exist while you have them open.

![Three states of the editor, before (top row) and after (bottom row) the layout pass](docs/shots/compare-layout.png)

**Fit to view.** A document opens fitted to the window, but never below 50%. The sample used to
open as the whole graph at an unreadable 26%; now you see where the conversation starts. F
fits everything when you want the overview.

<img src="docs/shots/04-fit/00-open.png" alt="The sample document as it opens, at 50%" width="600">

**Node kinds.** Each node has a coloured bar on its left edge for its kind, so a speech node, a
condition and an instruction look different at a glance. A node that nothing links to gets a
small hollow ring, which is where a conversation starts.

<table><tr>
<td><img src="docs/shots/05-before/07-condition.png" alt="Nodes before the kind strip"></td>
<td><img src="docs/shots/05-kinds/07-condition.png" alt="Nodes with the kind strip"></td>
</tr><tr><td align="center">before</td><td align="center">after</td></tr></table>

**Command palette.** Ctrl+K searches every command. It reads the same table as the menus and the
F1 sheet, so a name or shortcut can't differ between them.

<img src="docs/shots/06-palette/22-palette-query.png" alt="The command palette filtered to 'zoom'" width="600">

**Play trail.** While a conversation plays, the canvas shows where you've been. Edges you took are
drawn heavier, nodes you passed are outlined, and once-only choices you've used up are dimmed.
The clip at the top shows it, and so does the screenshot below.

![Mid-play in the light theme: the walked path drawn heavier, the inspector showing the current line](docs/shots/12-wires/L13-play-trail.png)

**Getting around.** Big graphs need it. Search takes filters (`kind:cond`, `speaker:player`,
`tag:act1`, `sets:gold`), and with an empty box it lists your bookmarks, which are just nodes
that have an alias. Every variable, character and flow shows where it's used. Ctrl+M opens the
flow map: one box per canvas, an arrow wherever a jump crosses.

<table><tr>
<td><img src="docs/shots/12-wires/28-search-bookmarks.png" alt="Search with nothing typed, listing bookmarks"></td>
<td><img src="docs/shots/12-wires/26-flow-map.png" alt="The flow map: main jumps to road"></td>
</tr></table>

**Paper, and your own type.** The last pass made the look a setting. There are two themes, Dark
and a light one called Paper, and they follow your desktop unless you say otherwise. Wires are
elbows now: a hub's choices leave on one trunk and each label gets its own run. Ctrl+, opens
Preferences, where the UI font and the text font are separate. The text font is for what a
character says, so you can set dialogue in Newsreader or screenplay Courier without touching
how variable names look. All the fonts are bundled, so it looks the same on any machine.

<table><tr>
<td><img src="docs/shots/13-fonts/L31-prefs.png" alt="The Preferences card in the light theme"></td>
<td><img src="docs/shots/13-fonts/32-text-courier.png" alt="Dark theme with the text font set to Courier Prime"></td>
</tr></table>

## How I checked it

`tools/shots.py` builds the editor, puts it into a fixed list of states over a small automation
socket, and screenshots each one. The states are the same every run, so a run before a change
and a run after make a fair before/after pair. Every image above came from it.

Anything involving the pointer (clicking an elbow wire, dragging a new one out of a port,
Ctrl+, and clicking a font chip) I also tested with real keyboard and mouse events, not only
the socket. That caught more than the unit tests did. Two bugs got past all of them: text typed
into any overlay field was black on near-black, and the palette could highlight two rows at
once, one under the mouse and one from the arrow keys. I only saw both by looking.

## Build and run

```sh
cargo run --release
```

On Linux GPUI renders through Vulkan, so you need the Vulkan and X11 or Wayland development
packages. If the build fails inside a `blade-*` crate or `gpui` itself, it's almost always a
missing system library, not this code.

It opens on a small sample conversation with one of every node kind, a second flow, tags and two
bookmarks, so there's something to click on.

## Not done yet

- No release binary, so for now you have to build it.

[`CLAUDE.md`](CLAUDE.md) covers the architecture and what I decided not to build.
[`docs/next.md`](docs/next.md) is what I'd do next.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The fonts in `assets/fonts` keep their own SIL Open
Font License terms; the licence files are next to them.
