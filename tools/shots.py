#!/usr/bin/env python3
"""Screenshots of the editor in a fixed list of states, for before/after comparisons.

    python3 -I tools/shots.py docs/shots/02-something [--only hub,edge]

Builds with `--features automation`, launches the editor against a private TMPDIR (see
src/automation.rs for why that matters), drives it over the automation socket, and
captures each state with Spectacle. Every run produces the same states from the same
sample document, so two runs on either side of a change make a real before/after pair.

Spectacle captures the *active* window, so leave the desktop alone while it runs (about
half a minute). A capture that comes out the wrong size is reported rather than kept.
"""

import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "debug" / "palaver"
WINDOW = (1200, 800)
SETTLE = 0.4  # seconds for a frame to land after a command

# Each shot starts from RESET, so the order only matters for the last one, which edits.
# Escape closes an overlay *instead of* stopping play when one is open, so it is sent twice:
# the first closes whatever is up, the second stops a play-through the overlay was hiding.
RESET = [
    "menu Play/Stop",
    "menu Play/Stop",
    "select none",
    "pane variables",
    "flow main",
    "field search",
    "focus canvas",
    # Opening fits the view, so without this every detail shot would be at whatever zoom the
    # document opened at. 100% keeps them comparable with the passes before fit existed.
    "menu View/Actual size",
]

SHOTS = [
    ("01-default", []),
    ("02-speech", ["select node 1"]),
    ("03-speech-typing", ["select node 1", "focus text"]),
    ("04-hub", ["select kind hub"]),
    ("05-jump", ["select kind jump"]),
    ("06-instruction", ["select kind instruction"]),
    ("07-condition", ["select kind condition"]),
    ("08-edge", ["select edge 0"]),
    ("09-multi", ["select node 2", "select nodes 1 2 3"]),
    ("10-search", ["menu Edit/Find", "field search inn"]),
    ("11-cast", ["pane cast"]),
    ("12-flows", ["pane flows"]),
    ("13-play", ["select node 1", "menu Play/Play conversation"]),
    ("14-play-hub", ["select node 1", "menu Play/Play conversation", "choose 0", "choose 0", "choose 0"]),
    ("15-problems", ["menu View/Problems"]),
    ("16-help", ["menu View/Keyboard shortcuts"]),
    ("24-play-trail", ["select node 1", "menu Play/Play conversation", "choose 0", "choose 0", "choose 0", "choose 1"]),
    ("17-play-over-problems", ["select node 1", "menu Play/Play conversation", "menu View/Problems"]),
    ("25-tags", ["pane tags"]),
    ("27-search-filter", ["menu Edit/Find", "field search kind:cond"]),
    ("21-palette", ["menu View/Command palette"]),
    # `zoom` matches two commands, so `palette next` leaves the second row highlighted.
    ("22-palette-query", ["menu View/Command palette", "field palette zoom", "palette next"]),
    ("23-palette-empty", ["menu View/Command palette", "field palette zzz"]),
    # From here on the shots edit the document, which `quit discard` throws away at the end.
    ("18-note", ["select none", "menu Node/Note", "field text Rewrite this branch once the pass is open."]),
    # Two nodes dropped with nothing selected, where the view's centre already holds a node:
    # they must stack below it rather than land on top.
    ("19-stacked", ["select none", "menu Node/Note", "select none", "menu Node/Hub"]),
    # The view the editor opens on is captured separately, before any reset (see main()).
    # The Fit command is the overview: everything, however small.
    ("20-fit-all", ["menu View/Fit to view"]),
    # Last: the map is a mode, not an overlay, so it stays up for every shot after it.
    ("26-flow-map", ["menu View/Flow map"]),
]


class Editor:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(path))
        self.reader = self.sock.makefile("r")

    def send(self, command):
        self.sock.sendall((command + "\n").encode())
        reply = self.reader.readline().strip()
        if not reply.startswith("ok"):
            raise RuntimeError(f"{command!r}: {reply}")
        return reply[3:]


def png_size(path):
    with open(path, "rb") as f:
        head = f.read(24)
    return struct.unpack(">II", head[16:24])


# The size of the first capture that passed, which every later one has to match: the editor is
# resized to a fixed logical size, so its capture is the same size every time.
_expected = None


def capture(editor, out, scratch):
    """Captures the editor into `scratch` and moves it to `out` only once it is known to be
    the editor. Spectacle shoots whatever window is *active*, and the person running this
    may switch to another one mid-run — so a capture is never written into the repository
    until it has passed, or a rejected one could be committed with `git add docs`."""
    global _expected
    shot = scratch / "capture.png"
    shot.unlink(missing_ok=True)
    editor.send("activate")
    time.sleep(SETTLE)
    subprocess.run(["spectacle", "-b", "-n", "-a", "-o", str(shot)], check=True)
    for _ in range(50):
        if shot.exists() and shot.stat().st_size > 0:
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("spectacle wrote nothing")
    time.sleep(0.2)  # let the write finish
    w, h = png_size(shot)
    # Spectacle includes the frame and its shadow, so the capture is a little larger, and the
    # desktop's scale factor multiplies all of it — a plain size check broke the day the scale
    # changed. The *shape* survives that: 1200x800 plus a title bar and shadow is about
    # 1.39:1 at every scale seen, and a browser window, the thing that was once captured by
    # mistake, is 1.53. Then it has to match the first capture exactly.
    if not 1.36 <= w / h <= 1.42 or (_expected and (w, h) != _expected):
        shot.unlink(missing_ok=True)
        raise RuntimeError(f"captured {w}x{h} — another window was active, not kept")
    _expected = _expected or (w, h)
    shutil.move(str(shot), str(out))


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    out_dir = Path(args[0])
    only = None
    if "--only" in args:
        only = set(args[args.index("--only") + 1].split(","))
    out_dir.mkdir(parents=True, exist_ok=True)

    subprocess.run(["cargo", "build", "--features", "automation"], cwd=ROOT, check=True)

    tmp = Path(tempfile.mkdtemp(prefix="palaver-shots-"))
    sock = tmp / "palaver-automation.sock"
    env = dict(os.environ, TMPDIR=str(tmp))
    app = subprocess.Popen([str(BINARY)], env=env, stderr=subprocess.DEVNULL)
    failed = []
    taken = 0
    try:
        for _ in range(150):
            if sock.exists():
                break
            time.sleep(0.1)
        else:
            sys.exit("the editor never opened its automation socket")
        editor = Editor(sock)
        editor.send(f"resize {WINDOW[0]} {WINDOW[1]}")
        time.sleep(1.0)  # first frame, fonts

        # The view the document opens on, before RESET touches the zoom. Skipped by --only
        # unless asked for by name, like any other shot.
        if not only or "00-open" in only:
            try:
                capture(editor, out_dir / "00-open.png", tmp)
                print(f"  {'00-open':22} {editor.send('state')}")
                taken += 1
            except RuntimeError as error:
                failed.append("00-open")
                print(f"  {'00-open':22} FAILED: {error}")

        for name, commands in SHOTS:
            if only and not any(name.endswith(o) or name == o for o in only):
                continue
            try:
                for command in RESET + commands:
                    editor.send(command)
                time.sleep(SETTLE)
                capture(editor, out_dir / f"{name}.png", tmp)
                print(f"  {name:22} {editor.send('state')}")
                taken += 1
            except RuntimeError as error:
                failed.append(name)
                print(f"  {name:22} FAILED: {error}")

        editor.send("quit discard")
        app.wait(timeout=5)
    finally:
        if app.poll() is None:
            app.kill()
        shutil.rmtree(tmp, ignore_errors=True)

    print(f"{taken} shots in {out_dir}" + (f", failed: {failed}" if failed else ""))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
