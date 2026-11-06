"""Real-input check of the Preferences card: Ctrl+, opens it, a click on a chip changes a setting. Uses /dev/uinput, so it acts on whatever has
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

    ui = UInput({e.EV_KEY:[e.BTN_LEFT], e.EV_ABS:[(e.ABS_X,AbsInfo(0,0,W-1,0,0,0)),(e.ABS_Y,AbsInfo(0,0,H-1,0,0,0))]}, name="palaver-prefs0")
    time.sleep(0.4)
    def move(x,y):
        ui.write(e.EV_ABS,e.ABS_X,int(x)); ui.write(e.EV_ABS,e.ABS_Y,int(y)); ui.syn(); time.sleep(0.25)
    def click(x,y):
        move(x,y); ui.write(e.EV_KEY,e.BTN_LEFT,1); ui.syn(); time.sleep(0.12)
        ui.write(e.EV_KEY,e.BTN_LEFT,0); ui.syn(); time.sleep(0.4)
    ui2 = ui
    # keyboard on the same virtual device: add the keys it may press
    ui.close()
    ui = UInput({e.EV_KEY:[e.BTN_LEFT,e.KEY_LEFTCTRL,e.KEY_COMMA,e.KEY_ESC],
                 e.EV_ABS:[(e.ABS_X,AbsInfo(0,0,W-1,0,0,0)),(e.ABS_Y,AbsInfo(0,0,H-1,0,0,0))]}, name="palaver-prefs")
    time.sleep(0.6)
    def key_chord(*keys):
        for k in keys: ui.write(e.EV_KEY,k,1)
        ui.syn(); time.sleep(0.12)
        for k in reversed(keys): ui.write(e.EV_KEY,k,0)
        ui.syn(); time.sleep(0.5)
    def state(): return dict(kv.split("=",1) for kv in cmd("state").split())
    results=[]
    def check(name, ok, detail=""):
        results.append(ok); print(("PASS " if ok else "FAIL ")+name+(" — "+detail if detail else ""))

    # focus the canvas first so the chord reaches the editor's key context
    move(left+40, top+40); ui.write(e.EV_KEY,e.BTN_LEFT,1); ui.syn(); time.sleep(0.1); ui.write(e.EV_KEY,e.BTN_LEFT,0); ui.syn(); time.sleep(0.4)
    check("starts closed", "prefs" not in state()["overlays"])
    key_chord(e.KEY_LEFTCTRL, e.KEY_COMMA)
    st=state(); check("Ctrl+, opens the card", "prefs" in st["overlays"], st["overlays"])
    key_chord(e.KEY_LEFTCTRL, e.KEY_COMMA)
    check("Ctrl+, again closes it", "prefs" not in state()["overlays"])
    key_chord(e.KEY_LEFTCTRL, e.KEY_COMMA)

    # find the chips. The card is the raised colour; anything inside it that is not is a chip,
    # a line of text or the preview box. Chip rows are the bands of that height (a line of
    # text is half as tall, the preview box twice).
    img = full_capture(); px = img.load()
    def near(c,rgb,t=3): return all(abs(c[i]-rgb[i])<t for i in range(3))
    CARD=(0x2b,0x2b,0x33)
    mx, my = (left+right)//2, (top+bottom)//2
    # grow the card's box outward from the window centre along a row/column that is mostly card
    row_ok=lambda y: sum(near(px[x,y],CARD) for x in range(left,right,4)) > 60
    cl=cr=mx
    while near(px[cl-1,my-60],CARD) or near(px[cl-1,my+60],CARD): cl-=1
    while near(px[cr+1,my-60],CARD) or near(px[cr+1,my+60],CARD): cr+=1
    ct=cb=my
    while near(px[cl+8,ct-1],CARD): ct-=1
    while near(px[cl+8,cb+1],CARD): cb+=1
    print(f"card px x{cl}..{cr} y{ct}..{cb}")
    inner=(cl+30, cr-30)
    busy=[any(not near(px[x,y],CARD,6) for x in range(*inner,2)) for y in range(ct+25,cb-10)]
    bands=[]; start=None
    for k,b in enumerate(busy+[False]):
        if b and start is None: start=k
        if not b and start is not None:
            bands.append((start+ct+25, k+ct+25-1)); start=None
    tall=[(a,b) for a,b in bands if 24*scale/1.15 <= b-a+1 <= 48*scale/1.15 ]
    print("chip bands (y):", tall)
    assert len(tall)>=3, "could not find the three chip rows"
    ya,yb = tall[2]                      # theme, UI font, text font
    ty=(ya+yb)//2
    xs=[any(not near(px[x,ty-(yb-ya)//2+3],CARD,6) or not near(px[x,ty],CARD,6) for _ in [0]) for x in range(cl+30,cr-30)]
    chips=[]; start=None
    for k,b in enumerate(xs+[False]):
        if b and start is None: start=k
        if not b and start is not None:
            if k-start>40: chips.append((start+cl+30,k+cl+30-1))
            start=None
    # text inside a chip leaves gaps; merge runs closer than 12px
    merged=[]
    for a,b in chips:
        if merged and a-merged[-1][1] < 12: merged[-1]=(merged[-1][0],b)
        else: merged.append((a,b))
    print("text-font chips (x):", merged)
    a,b = merged[-1]
    before=state()["settings"]
    click(b-30, ty)   # the right-hand end of the row: the last chip, Courier Prime
    after=state()["settings"]
    check("a click on the last text-font chip changes the text font", after.split("/")[2]=="CourierPrime", f"{before} -> {after}")
    key_chord(e.KEY_ESC)
    check("Esc closes the card", "prefs" not in state()["overlays"])
    print(f"{sum(results)}/{len(results)} passed")
    ui.close()
finally:
    try: cmd("quit discard")
    except Exception: pass
    time.sleep(0.5); app.terminate()
