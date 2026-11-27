# Keyboard-only use

The story: *as a writer, I can draft, revise and test a branching conversation without
taking my hands off the keyboard.*

What reaches the keyboard today: every `MENUS` command (through the Ctrl+K palette and its
own shortcut), Tab / Ctrl+1–6 to branch or place a node, the clipboard keys, undo, zoom, Fit,
the overlays, the file commands, and F5 / Escape to start and stop a play-through. Each
piece below closes one gap in the story, in the order they matter.

## 1. Graph navigation on the canvas

There is no way to select an existing node from the keyboard: once a node is left, it cannot
be returned to. The arrow keys walk the graph's edges, not the screen:

- **Right**: the first choice out of the selected node.
- **Left**: back along the first-drawn edge in, the rule `Doc::context` already follows.
- **Up / Down**: the previous or next sibling among the parent's choices.
- **Alt+Right**: select the edge itself rather than its target, so a choice label can be
  reached.

## 2. Enter to edit, Tab between fields

The Text field is reachable only by double-click, and the inspector's fields have no focus
order: Tab in a field does nothing.

- **Enter** on a selected node puts the cursor at the end of its Text field (or Speaker
  where the kind has no text); on a selected edge, in its Choice field.
- **Tab / Shift+Tab** in an inspector field move to the next and previous field. Tab means
  Branch only in `"Canvas"`, so a `"TextInput"` binding does not collide with it.

## 3. Keys in the find overlay

Find filters, but a hit can only be clicked. **Up / Down** move a highlighted hit and
**Enter** jumps to it, on the command palette's pattern (a key context on the card, an
ancestor of its field).

## 4. Link to an existing node

The only way to connect to an existing node is to drag from the out-port, so merges, loops
and a jump's target cannot be made from the keyboard. **Link to…** is a command that reuses
the find list as a picker and calls `Doc::link`, guarded by `Doc::can_link` before its
checkpoint.

## 5. Choosing in play

The play panel's choices are click-only, so F5 starts a run that cannot be advanced.
**1–9** pick from `Doc::offered`, and the panel numbers its rows so the keys are visible.

## 6. Kind, Ending and Once-only as commands

The kind picker and the Ending and once-only toggles are chips with no command behind
them, so the palette cannot reach them. Each becomes a command by the four-place convention
(`actions!`, `MENUS`, `.on_action`, `bind_keys`), and so appears in the menus, the F1 sheet
and Ctrl+K at once.

## Not part of the story

Layout and the left pane stay mouse-first without breaking it: moving nodes is cosmetic,
Fit and centring on a find hit stand in for panning, and the Variables, Cast, Flows and Tags
tabs are setup rather than writing.
