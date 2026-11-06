#!/usr/bin/env python3
"""A screen recording of the editor, as an mp4 and a GIF.

    python3 -I tools/record.py docs/palaver

Spectacle's window recording needs someone to pick the window, so this does not use it. It
starts a headless `sway` in a private XDG_RUNTIME_DIR, runs the editor (the `automation`
build, software Vulkan) inside it, drives a fixed scene over the automation socket and grabs
frames with `grim`, then encodes them with `ffmpeg`. The real desktop is never captured and
never touched, so unlike tools/shots.py this is safe to run while you work.

Needs: sway, grim, ffmpeg, and Mesa's lavapipe (`lvp_icd`). Edit SCENE to change the story.
"""

import glob
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "debug" / "palaver"
SIZE = (1280, 860)
LVP = next(iter(glob.glob("/usr/share/vulkan/icd.d/lvp_icd*x86_64.json") + glob.glob("/usr/share/vulkan/icd.d/lvp_icd*.json")), None)

# The story. `hold(s)` waits, `send(cmd)` is an automation command (see src/automation.rs),
# `type_into(field, text)` types a character at a time.
SCENE = """
send("resize 1280 860"); hold(1.0)
for c in ["theme light", "font ui inter", "font text same", "menu View/Actual size"]: send(c)
hold(1.8)

def play_through(gap=1.35, stop=True):
    send("select node 1"); hold(1.2)
    send("menu Play/Play conversation"); hold(gap)
    for choice in [0, 0, 0, 1]:
        send(f"choose {choice}"); hold(gap)
    hold(1.0 if stop else 3.0)
    if stop:
        send("menu Play/Stop"); send("menu Play/Stop"); hold(0.8)

play_through()
send("select none"); hold(0.4)
send("menu Edit/Find"); hold(0.8)
type_into("search", "kind:cond"); hold(1.8)
send("menu Play/Stop"); hold(0.5)
send("menu View/Flow map"); hold(2.2)
send("menu View/Flow map"); hold(0.8)
send("menu View/Command palette"); hold(0.6)
type_into("palette", "zoom"); hold(1.4)
send("menu Play/Stop"); hold(0.4)
send("prefs"); hold(1.6)
send("font text courier"); hold(1.2)
send("theme dark"); hold(1.6)
send("menu Play/Stop"); hold(0.6)
play_through(stop=False)
"""


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    for tool in ("sway", "grim", "ffmpeg"):
        if not shutil.which(tool):
            sys.exit(f"{tool} is not installed")
    if not LVP:
        sys.exit("no lavapipe Vulkan driver found (vulkan-lvp / mesa-vulkan-drivers)")
    out = Path(sys.argv[1])
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cargo", "build", "--features", "automation"], cwd=ROOT, check=True)

    work = Path(tempfile.mkdtemp(prefix="palaver-rec-"))
    runtime = work / "runtime"  # private, so the compositor's sockets cannot meet the desktop's
    runtime.mkdir(mode=0o700)
    frames = work / "frames"
    frames.mkdir()
    (work / "sway.conf").write_text(
        f"output HEADLESS-1 resolution {SIZE[0]}x{SIZE[1]} position 0 0\n"
        "default_border none\nfocus_follows_mouse no\n"
        "for_window [app_id=\".*\"] fullscreen enable\n"
    )
    base = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "WAYLAND_DISPLAY")}
    base["XDG_RUNTIME_DIR"] = str(runtime)
    sway = subprocess.Popen(
        ["sway", "-c", str(work / "sway.conf")],
        env=dict(base, WLR_BACKENDS="headless", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman"),
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    app = None
    try:
        for _ in range(100):
            if (runtime / "wayland-1").exists():
                break
            time.sleep(0.1)
        else:
            sys.exit("the headless compositor never came up")
        env = dict(base, WAYLAND_DISPLAY="wayland-1", TMPDIR=str(work),
                   XDG_CONFIG_HOME=str(work / "config"), VK_ICD_FILENAMES=LVP)
        app = subprocess.Popen([str(BINARY)], env=env, stderr=subprocess.DEVNULL)
        sock = work / "palaver-automation.sock"
        for _ in range(150):
            if sock.exists():
                break
            time.sleep(0.1)
        else:
            sys.exit("the editor never opened its automation socket")
        conn = socket.socket(socket.AF_UNIX)
        conn.connect(str(sock))
        reader = conn.makefile("r")

        def send(command):
            conn.sendall((command + "\n").encode())
            reply = reader.readline().strip()
            if not reply.startswith("ok"):
                raise RuntimeError(f"{command!r}: {reply}")
            return reply[3:]

        def type_into(field, text, gap=0.11):
            for i in range(1, len(text) + 1):
                send(f"field {field} {text[:i]}")
                time.sleep(gap)

        stop, stamps = threading.Event(), []

        def grab():
            n = 0
            while not stop.is_set():
                stamps.append(time.time())
                subprocess.run(["grim", "-l", "1", str(frames / f"{n:05d}.png")], env=env, check=False)
                n += 1

        grabber = threading.Thread(target=grab, daemon=True)
        grabber.start()
        try:
            exec(SCENE, {"send": send, "hold": time.sleep, "type_into": type_into})
        finally:
            stop.set()
            grabber.join(timeout=5)
        try:
            send("quit discard")
            app.wait(timeout=5)
        except Exception:
            pass
    finally:
        if app and app.poll() is None:
            app.kill()
        sway.terminate()

    shots = sorted(frames.glob("*.png"))
    if len(shots) < 30:
        sys.exit(f"only {len(shots)} frames were captured")
    shots[-1].unlink()  # may be half-written when the grabber stopped
    n = len(shots) - 1
    fps = n / (stamps[n - 1] - stamps[0])  # grim's pace varies, so use what it actually did
    src = ["-framerate", f"{fps:.2f}", "-i", str(frames / "%05d.png")]
    quiet = ["ffmpeg", "-v", "error", "-y"]
    subprocess.run(quiet + src + ["-vf", "scale=1280:-2,format=yuv420p", "-r", "30",
                                   "-c:v", "libx264", "-crf", "20", "-preset", "slow",
                                   "-movflags", "+faststart", str(out) + ".mp4"], check=True)
    subprocess.run(quiet + src + ["-vf", "fps=12,scale=800:-1:flags=lanczos,split[a][b];"
                                   "[a]palettegen=max_colors=128:stats_mode=diff[p];"
                                   "[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle",
                                   str(out) + ".gif"], check=True)
    shutil.rmtree(work, ignore_errors=True)
    print(f"{n} frames at {fps:.1f} fps -> {out}.mp4, {out}.gif")


if __name__ == "__main__":
    main()
