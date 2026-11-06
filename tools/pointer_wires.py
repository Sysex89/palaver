"""Real-pointer check of elbow-wire hit-testing and drag-to-connect. Uses /dev/uinput, so it acts on whatever has
focus: run it only while the desktop is idle.

Geometry comes from a full-screen capture (the canvas is the one big region of the canvas
colour), pan and zoom from the editor's own `state`, and the document's node positions from
`Doc::sample`'s known layout. Every click is real input; the answer is read back from `state`.
"""
import os, subprocess, sys, tempfile, time, socket, re
from pathlib import Path
from PIL import Image
import evdev
from evdev import UInput, ecodes as e, AbsInfo

ROOT = Path("/home/pavlos/repos/palaver")
BIN = ROOT / "target/debug/palaver"
VARS_W, SIDEBAR_W, MENU_H, STATUS_H = 252, 280, 30, 26  # logical px, as main.rs
WIN = (1200, 800)
NODE_W, NODE_H = 200, 90

tmp = Path(tempfile.mkdtemp(prefix="palaver-wires-"))
sockp = tmp / "palaver-automation.sock"
app = subprocess.Popen([str(BIN)], env=dict(os.environ, TMPDIR=str(tmp)), stderr=subprocess.DEVNULL)

def connect():
    for _ in range(150):
        if sockp.exists(): break
        time.sleep(0.1)
    s = socket.socket(socket.AF_UNIX); s.connect(str(sockp)); return s, s.makefile("r")
sock, rd = connect()
def cmd(c):
    sock.sendall((c+"\n").encode()); r = rd.readline().strip()
    assert r.startswith("ok"), (c, r); return r[3:]

def full_capture():
    out = tmp / "full.png"
    out.unlink(missing_ok=True)
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", str(out)], check=True)
    for _ in range(50):
        if out.exists() and out.stat().st_size > 0: break
        time.sleep(0.1)
    time.sleep(0.3)
    return Image.open(out).convert("RGB")

try:
    cmd(f"resize {WIN[0]} {WIN[1]}"); time.sleep(1.0)
    cmd("theme dark"); cmd("menu View/Actual size"); cmd("activate"); time.sleep(0.8)
    cmd("select node 1"); time.sleep(0.5)   # centres the first node, so its wires are on screen
    cmd("select none"); time.sleep(0.3)
    st = dict(kv.split("=",1) for kv in cmd("state").split())
    zoom = float(st["zoom"]) / 100; pan = tuple(float(v) for v in st["pan"].split(","))

    img = full_capture(); W, H = img.size
    # the canvas colour is #1e1e22; its bounding box is the canvas pane (plus the vars field
    # wells, which are the same colour — so take the biggest run along a row through the middle)
    px = img.load()
    def is_canvas(c): return abs(c[0]-0x1e)<3 and abs(c[1]-0x1e)<3 and abs(c[2]-0x22)<3
    def longest(coords):
        best=(0,-1); start=prev=None
        for c in coords+[None]:
            if start is None:
                if c is not None: start=prev=c
                continue
            if c is not None and c==prev+1:
                prev=c; continue
            if prev-start > best[1]-best[0]: best=(start,prev)
            start=prev=c
        return best
    # widest canvas-coloured run over many rows, tallest over many columns: nodes and wires
    # break any single row, but none of them break all of them.
    left=right=0
    for y in range(0,H,12):
        a,b = longest([x for x in range(W) if is_canvas(px[x,y])])
        if b-a > right-left: left,right=a,b
    top=bottom=0
    for x in range(left,right,12):
        a,b = longest([y for y in range(H) if is_canvas(px[x,y])])
        if b-a > bottom-top: top,bottom=a,b
    scale = (right-left+1) / (WIN[0]-VARS_W-SIDEBAR_W)
    print(f"capture {W}x{H}, canvas px x{left}..{right} y{top}..{bottom}, scale {scale:.3f}, zoom {zoom}, pan {pan}")

    ui = UInput({e.EV_KEY:[e.BTN_LEFT], e.EV_ABS:[(e.ABS_X,AbsInfo(0,0,W-1,0,0,0)),(e.ABS_Y,AbsInfo(0,0,H-1,0,0,0))]}, name="palaver-wires")
    time.sleep(0.6)
    def move(x,y):
        ui.write(e.EV_ABS,e.ABS_X,int(x)); ui.write(e.EV_ABS,e.ABS_Y,int(y)); ui.syn(); time.sleep(0.25)
    def click(x,y):
        move(x,y); ui.write(e.EV_KEY,e.BTN_LEFT,1); ui.syn(); time.sleep(0.12)
        ui.write(e.EV_KEY,e.BTN_LEFT,0); ui.syn(); time.sleep(0.4)
    def doc_to_px(dx,dy):
        sx, sy = dx*zoom+pan[0], dy*zoom+pan[1]     # pane space, logical px
        return left + sx*scale, top + sy*scale
    def selection(): return dict(kv.split("=",1) for kv in cmd("state").split())["selection"]

    # the sample: greet (60,260) -> ask (460,140) "Ask" and -> rude (460,380) "Refuse"
    out_x, out_y = 60+NODE_W, 260+NODE_H/2           # greet's out-port
    ask_in = (460, 140+NODE_H/2); rude_in = (460, 380+NODE_H/2)
    bend = out_x + 20
    checks = [
        ("run into ask (edge 0)",      ((bend+ask_in[0])/2 + 30, ask_in[1]),  "edge:0"),
        ("trunk above the stub",       (bend, (out_y+ask_in[1])/2),           "edge:0"),
        ("run into rude (edge 1)",     ((bend+rude_in[0])/2 + 30, rude_in[1]), "edge:1"),
        ("old diagonal midpoint (empty)", (((out_x+ask_in[0])/2),((out_y+ask_in[1])/2)), "none"),
        ("empty canvas",               (out_x+300, 60),                        "none"),
    ]
    results = []
    for name,(dx,dy),want in checks:
        cmd("select none"); time.sleep(0.2)
        x,y = doc_to_px(dx,dy); click(x,y)
        got = selection()
        ok = got == want
        results.append(ok); print(("PASS " if ok else "FAIL ")+f"{name}: clicked doc ({dx:.0f},{dy:.0f}) -> {got} (want {want})")
    # drag a new wire from greet's out-port onto "Suit yourself." (860,380): a real press, a
    # slow move so the rubber band is drawn, a release over the node.
    cmd("select none"); before = dict(kv.split("=",1) for kv in cmd("state").split())
    x0,y0 = doc_to_px(out_x, out_y); x1,y1 = doc_to_px(860+100, 380+45)
    move(x0,y0); ui.write(e.EV_KEY,e.BTN_LEFT,1); ui.syn(); time.sleep(0.2)
    for k in range(1,11):
        move(x0+(x1-x0)*k/10, y0+(y1-y0)*k/10)
        if k == 6:
            subprocess.run(["spectacle","-b","-n","-f","-o",str(tmp/"drag-preview-full.png")],check=True); time.sleep(0.5)
    ui.write(e.EV_KEY,e.BTN_LEFT,0); ui.syn(); time.sleep(0.5)
    after = dict(kv.split("=",1) for kv in cmd("state").split())
    made = after["selection"].startswith("edge:") and after["dirty"] == "true"
    results.append(made)
    print(("PASS " if made else "FAIL ")+f"drag from the out-port onto a node: selection {before['selection']} -> {after['selection']}, dirty {after['dirty']}")
    print(f"{sum(results)}/{len(results)} passed")
    ui.close()
finally:
    try: cmd("quit discard")
    except Exception: pass
    time.sleep(0.5); app.terminate()
