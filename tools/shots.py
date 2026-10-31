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
    ("17-play-over-problems", ["select node 1", "menu Play/Play conversation", "menu View/Problems"]),
    # From here on the shots edit the document, which `quit discard` throws away at the end.
    ("18-note", ["select none", "menu Node/Note", "field text Rewrite this branch once the pass is open."]),
    # Two nodes dropped with nothing selected, where the view's centre already holds a node:
    # they must stack below it rather than land on top.
    ("19-stacked", ["select none", "menu Node/Note", "select none", "menu Node/Hub"]),
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


def capture(editor, out):
    out.unlink(missing_ok=True)
    editor.send("activate")
    time.sleep(SETTLE)
    subprocess.run(["spectacle", "-b", "-n", "-a", "-o", str(out)], check=True)
    for _ in range(50):
        if out.exists() and out.stat().st_size > 0:
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("spectacle wrote nothing")
    time.sleep(0.2)  # let the write finish
    w, h = png_size(out)
    # Spectacle includes the frame and its shadow, so the capture is a little larger.
    if w < WINDOW[0] or h < WINDOW[1] or w > WINDOW[0] + 400:
        out.rename(out.with_suffix(".wrong-window.png"))
        raise RuntimeError(f"captured {w}x{h} — another window was active")


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

        for name, commands in SHOTS:
            if only and not any(name.endswith(o) or name == o for o in only):
                continue
            try:
                for command in RESET + commands:
                    editor.send(command)
                time.sleep(SETTLE)
                capture(editor, out_dir / f"{name}.png")
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
