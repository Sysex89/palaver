# The `.dln` format

A `.dln` file is one palaver document: every conversation in a project, the variables they
read and write, and the cast. This page describes the file and, more importantly, how a game
should play what is in it, so that a conversation behaves in the game the way it behaves when
you press F5 in the editor.

The file is UTF-8 JSON, pretty-printed with a trailing newline so that it diffs and merges
cleanly in git.

## An example

A toll at an inn's door. The player can pay, which sets a flag, or walk on, which they can do
only once. A check on the flag decides which of two closing lines they hear.

```json
{
  "nodes": [
    {
      "id": 18315609803685780223,
      "x": 0.0,
      "y": 0.0,
      "speaker": "Innkeeper",
      "text": "That'll be two coppers.",
      "alias": "toll",
      "flow": "",
      "tags": [],
      "end": false,
      "kind": "Speech",
      "effects": [],
      "conditions": []
    },
    {
      "id": 9902035496539809498,
      "x": 400.0,
      "y": 0.0,
      "speaker": "",
      "text": "",
      "alias": "",
      "flow": "",
      "tags": [],
      "end": false,
      "kind": "Instruction",
      "effects": [
        {
          "var": "paid",
          "value": {
            "Bool": true
          },
          "op": "Set"
        }
      ],
      "conditions": []
    },
    {
      "id": 17715439360488774658,
      "x": 800.0,
      "y": 0.0,
      "speaker": "",
      "text": "",
      "alias": "",
      "flow": "",
      "tags": [],
      "end": false,
      "kind": "Condition",
      "effects": [],
      "conditions": [
        {
          "var": "paid",
          "want": {
            "Bool": true
          },
          "op": "Eq"
        }
      ]
    },
    {
      "id": 2196245561037600040,
      "x": 1200.0,
      "y": 0.0,
      "speaker": "Innkeeper",
      "text": "Much obliged.",
      "alias": "",
      "flow": "",
      "tags": [
        "vo.recorded"
      ],
      "end": true,
      "kind": "Speech",
      "effects": [],
      "conditions": []
    },
    {
      "id": 15383755906875001587,
      "x": 1200.0,
      "y": 120.0,
      "speaker": "Innkeeper",
      "text": "Then mind the cold.",
      "alias": "",
      "flow": "",
      "tags": [],
      "end": true,
      "kind": "Speech",
      "effects": [],
      "conditions": []
    }
  ],
  "edges": [
    {
      "from": 18315609803685780223,
      "to": 9902035496539809498,
      "label": "Pay",
      "branch": false,
      "once": false
    },
    {
      "from": 18315609803685780223,
      "to": 17715439360488774658,
      "label": "Walk on",
      "branch": false,
      "once": true
    },
    {
      "from": 9902035496539809498,
      "to": 17715439360488774658,
      "label": "",
      "branch": false,
      "once": false
    },
    {
      "from": 17715439360488774658,
      "to": 2196245561037600040,
      "label": "",
      "branch": true,
      "once": false
    },
    {
      "from": 17715439360488774658,
      "to": 15383755906875001587,
      "label": "",
      "branch": false,
      "once": false
    }
  ],
  "variables": [
    {
      "name": "paid",
      "default": {
        "Bool": false
      }
    }
  ],
  "characters": [
    {
      "name": "Innkeeper",
      "color": 16238954
    }
  ],
  "flows": [],
  "premise": ""
}
```

Played from `toll`, the walk through it is either

- *That'll be two coppers.* → **Pay** → (sets `paid`) → (checks `paid == true`, which holds) → *Much obliged.*, or
- *That'll be two coppers.* → **Walk on** → (checks `paid == true`, which fails) → *Then mind the cold.*

## Reading it safely

**Node ids are unsigned 64-bit integers, written as bare JSON numbers**, and most of them are
larger than 2⁵³. Anything that parses JSON numbers into doubles will silently change them, and
then no edge points at the right node. JavaScript's `JSON.parse` does this, and so do the
JSON readers of some game engines, so check yours. Read ids as 64-bit integers (`u64` in Rust,
`ulong` in C#, plain `int` in Python), or use a parser that keeps big numbers as strings. Ids are random rather than counted, so that
two people adding nodes on separate branches never mint the same one.

**Unknown fields are ignored, and a field with a default may be missing**, and that is the
whole versioning story: there is no version number. Every field added since the format began
has a default, so an old file loads in a new editor and looks the same. The tables below give
each default; a field with none, such as a node's `id` or an edge's `label`, is always written.
A game reader should do the same: ignore what it does not know, and default what is missing.

## The document

| Field | Type | |
| --- | --- | --- |
| `nodes` | array of [nodes](#nodes) | In drawing order: a later node is drawn over an earlier one. |
| `edges` | array of [edges](#edges) | **In choice order.** See [Playing a conversation](#playing-a-conversation). |
| `variables` | array of [variables](#variables-and-values) | Default `[]`. |
| `characters` | array of [characters](#characters) | Default `[]`. |
| `flows` | array of strings | The named canvases. Default `[]`. |
| `premise` | string | A sentence on the setting, for the author. Default `""`. |
| `runs` | array of [saved runs](#saved-runs) | Editor test data. Absent when there are none. |

Older files may carry a `next_id` field. It is ignored.

## Nodes

| Field | Type | |
| --- | --- | --- |
| `id` | u64 | Stable and unique. Never reused, even after an undo. |
| `x`, `y` | number | Where the node sits on its canvas. Layout only. |
| `speaker` | string | Who says the line. Meaningful on `Speech` nodes only. |
| `text` | string | The line, a hub's label, or a note. See the table below. |
| `alias` | string | The author's name for the node, for a game to find it by. Default `""`. |
| `flow` | string | Which canvas the node is drawn on. `""` is the main one. Default `""`. |
| `tags` | array of strings | Free labels such as `todo` or `vo.recorded`, without the `#`. Default `[]`. |
| `end` | bool | The author marked this as a deliberate ending. Default `false`. |
| `kind` | string | One of the kinds below. Default `"Speech"`. |
| `effects` | array of [effects](#effects-and-conditions) | What an `Instruction` sets. Default `[]`. |
| `conditions` | array of [conditions](#effects-and-conditions) | What a `Condition` checks. Default `[]`. |
| `placeholder` | `{"seed": u64}` | Present while `text` is a generated draft line nobody has accepted. |

What each kind is, and which of its fields mean anything:

| `kind` | The player sees it | Uses | Its edges are |
| --- | --- | --- | --- |
| `Speech` | yes | `speaker`, `text` | the player's choices |
| `Hub` | yes | `text`, an optional label shown above the choices | the player's choices |
| `Instruction` | no | `effects` | the way on; the first one is taken |
| `Condition` | no | `conditions` | two pins, told apart by `branch` |
| `Jump` | no | | at most one: its target, often on another flow |
| `Note` | never | `text`, for the author | none; nothing links to or from a note |

A field a kind does not use may still hold a value, left over from when the node was another
kind (a hub that used to be a line keeps its old `speaker`). The editor keeps those values so
that changing a node's kind back loses nothing. **A reader must ignore them**: never show a
hub's `speaker`, and never run an `effects` list that is not on an `Instruction`.

**`alias` is the handle a game should use.** The author sets it and the editor never changes
it. An `id` is stable too, but nobody can type or read one. Aliases are unique when the file
is healthy; two nodes with the same alias are an error the editor reports.

## Edges

| Field | Type | |
| --- | --- | --- |
| `from`, `to` | u64 | Node ids. |
| `label` | string | The choice text the player reads. `""` is an unlabelled "continue". |
| `branch` | bool | Out of a `Condition`: `true` is the pin taken when the checks hold. Meaningless elsewhere. Default `false`. |
| `once` | bool | A choice offered only until the player takes it. Meaningful out of `Speech` and `Hub` only. Default `false`. |
| `placeholder` | `{"seed": u64}` | Present while `label` is a generated draft. |

An edge whose `from` or `to` names no node is broken. A reader should skip it; the editor
reports it as an error.

## Variables and values

| Field | Type | |
| --- | --- | --- |
| `name` | string | Unique. A dot groups names into sets in the editor (`quest.paid`); to a reader it is just part of the name. |
| `default` | value | What the variable holds when a play-through starts. |

A value is one of `{"Bool": true}` or `{"Int": 42}`, where `Int` is a signed 64-bit integer.
A variable's type is whichever of the two its `default` is; there is no separate type field.

## Effects and conditions

An effect is `{"var": name, "value": value, "op": op}`, where `op` is:

| `op` | Means | Default |
| --- | --- | --- |
| `"Set"` | the variable becomes `value` | yes |
| `"Add"` | an `Int` grows by `value`, saturating at the 64-bit limits; on a `Bool`, nothing happens | |

A condition is `{"var": name, "want": value, "op": op}`, and holds when the variable compares
to `want` as `op` says:

| `op` | `"Eq"` | `"Ne"` | `"Lt"` | `"Le"` | `"Gt"` | `"Ge"` |
| --- | --- | --- | --- | --- | --- | --- |
| reads | `==` | `!=` | `<` | `<=` | `>` | `>=` |

`"Eq"` is the default. A `Bool` orders `false` before `true`, though the editor only writes
`Eq` and `Ne` for one. **A condition on a variable nothing declares never holds, and neither
does one comparing a `Bool` with an `Int`.** Both are errors the editor reports, and taking
the false pin is the answer that decides nothing. An effect naming an undeclared variable does
nothing.

## Playing a conversation

These are the rules the editor's player follows. A game that follows them plays the same
conversation.

**State.** A play-through is the list of nodes it has visited, in order, including the ones it
walked through without showing. Variables start at their `default` values. They change as
instructions are passed, and nowhere else.

**Starting.** The game names the node to start at, by alias (or id). From there it *walks on*.

**Walking on** from the current node:

1. A `Speech` or `Hub` node: stop, and show it.
2. A `Condition`: it holds when **every** condition in its list holds, and a condition node
   with an empty list holds. Follow the first edge out of it whose `branch` equals that
   result and whose target exists. If there is no such edge, stop here: the graph ran out.
3. An `Instruction`: apply its effects, in order, then follow the first edge out of it whose
   target exists. With none, stop here.
4. A `Jump`: follow its one edge. With no target, stop here.
5. If following an edge would pass a node already passed since the last stop, stop instead.
   This guards against a loop made only of instructions, conditions and jumps.

**Showing a node.** A `Speech` node shows its `speaker` and its `text`. A `Hub` shows its
`text` as a heading above the choices, and may have none.

**Choices.** The choices are the edges out of the shown node, **in the order they appear in
`edges`**, skipping any whose target is missing. The author sets that order in the editor, so
keep it. A choice's text is its `label`; an empty label is a plain "continue". A `once` choice
that this play-through has already taken (the pair `from` → `to` appears consecutively in the
visited list) is spent: the editor shows it greyed out, and a game should not offer it. When a
choice is taken, add its target to the visited list and walk on from there.

**Ending.** The conversation is over when the shown node offers nothing: it has no choices, or
every one is spent. The `end` flag does not end anything by itself. It is the author saying an
ending here is deliberate, which stops the editor warning about a dead end.

**Running out of graph.** If walking stops on a node the player does not see, because a
condition's pin is unwired, a jump has no target, or a loop was cut, the file is incomplete.
The editor says "The flow stops here" and reports it in the problems list. A game should treat
it as the end of the conversation.

**What persists.** Spent choices and variables belong to one play-through in the editor. If a
game wants them to last between conversations, it keeps its own variable storage and loads the
`default` values only once.

## Characters

| Field | Type | |
| --- | --- | --- |
| `name` | string | Matches `speaker` on the nodes this character speaks. |
| `color` | u32 | `0xRRGGBB`, written as a decimal number. The tint in the editor. |

A `speaker` with no matching character is fine; the editor warns about it. The name *is* the
link, so renaming a character in the editor rewrites every node's `speaker` with it.

## Flows

A flow is a named canvas: a way of laying out a big document, nothing more. A node's `flow`
says which canvas draws it, `""` being the main one. The `flows` array lists the declared ones,
so that an empty canvas survives a save. Playing ignores flows entirely: a walk crosses from
one canvas to another through a `Jump` like any other edge.

## Saved runs

`runs` holds play-throughs an author saved in the editor to replay after an edit:
`{"name": "Run 1", "run": {"path": [ids], "sets": [...], "forced": [...]}}`. They are test data
for the editor. A game should ignore them.

## Translation and voice

Every string a player reads has a key:

| String | Key |
| --- | --- |
| a `Speech` node's `text`, or a `Hub`'s label | `node-<id>` |
| an edge's `label`, out of a `Speech` or `Hub` | `edge-<from>-<to>` |

The key of a choice is its two ends, not its position, so it survives any edit that leaves the
choice in place. Blank strings, notes, and text left on kinds that do not show it have no key.

**File → Export Fluent** writes those strings as a [Fluent](https://projectfluent.org) `.ftl`
file, with a comment over each entry saying who speaks it and what it answers. A line that can
be reached more than one way names the first way in, in the order the edges were drawn:

```
# Innkeeper, after the choice "Walk on"
node-15383755906875001587 = Then mind the cold.
```

**File → Export Recording Script** writes the same strings as a CSV for a voice session,
keyed the same way, so a take named after its key matches its line, its translations and the
game's lookup. Its columns are `key`, `character`, `kind` (`line`, `heading` or `choice`),
`line`, `after` (what it answers), `section` (where it is in the screenplay export, `§4`),
`tags` and `placeholder`. It starts with a UTF-8 byte-order mark so that Excel reads it
correctly; code reading it should open it as `utf-8-sig`.

**File → Export Screenplay** writes a numbered Markdown script for reading. It carries no keys.

All three can be written without opening a window, which is how a build keeps them current:

```sh
palaver talk.dln --export-ftl strings.ftl --export-recording vo.csv --export-screenplay talk.md
palaver talk.dln --export-ftl -        # to standard output
palaver talk.dln --after-save 'palaver "$PALAVER_FILE" --export-ftl strings.ftl'
```

The last line opens the editor with a command to run after every save, in the file's folder,
so each save regenerates `strings.ftl` beside it. A failed export exits with status 1, and so
does asking for two exports that name the same file, before either is written.

## What the editor checks

Its problems list is the quickest way to know a file is safe to ship. An **error** is a file a
game cannot play correctly:

- an edge whose end names no node
- nodes, but none that nothing links to, so no conversation can start
- an effect or condition naming an undeclared variable
- an effect or condition with a value, or an operator, of the wrong type for its variable
- two nodes with the same alias
- an ordinary edge between two canvases, which only a `Jump` may cross

A **warning** is something an author may want to look at, in a file that still plays: an
unreachable node, a dead end not marked `end`, an empty node, a condition pin with nothing
wired to it, an undeclared speaker, the same choice twice, and lines still marked as
placeholders.
