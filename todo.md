# Keyboard-only use

The story: *as a writer, I can draft, revise and test a branching conversation without
taking my hands off the keyboard.*

What reaches the keyboard today: every `MENUS` command (through the Ctrl+K palette and its
own shortcut), the arrow keys walking the graph (the Go menu: nodes along edges, wires with
Alt, Home for the start), Enter into the selection's field and Tab between the inspector's
fields, Up / Down / Enter through the find results, Ctrl+L to link the selection to any node
it can reach, Tab / Ctrl+1–6 to branch or place a node, the clipboard keys, undo, zoom, Fit,
the overlays, the file commands, and F5, 1–9 and Escape to start, play and stop a
play-through. The piece below closes the last gap in the story.

## 1. Kind, Ending and Once-only as commands

The kind picker and the Ending and once-only toggles are chips with no command behind
them, so the palette cannot reach them. Each becomes a command by the four-place convention
(`actions!`, `MENUS`, `.on_action`, `bind_keys`), and so appears in the menus, the F1 sheet
and Ctrl+K at once.

The F1 sheet has no room left for those rows: at 860px its card is already at the
`relative(0.92)` cap, and one more row puts the icon credit below the scroll edge. This
piece has to make the room as well, for instance by grouping the kind commands under one
row the way the Node menu's six kinds could be, or by giving the sheet a fourth column on
a window wide enough for one.

## Not part of the story

Layout and the left pane stay mouse-first without breaking it: moving nodes is cosmetic,
Fit and centring on a find hit stand in for panning, and the Variables, Cast, Flows and Tags
tabs are setup rather than writing.
