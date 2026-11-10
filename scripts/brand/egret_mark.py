#!/usr/bin/env python3
"""Generate every egret logo asset from one geometric construction.

The mark is built only from circles and straight lines, combined shape-builder style:

  neck  = two half-rings (circle minus a concentric circle). Their centres A and B sit on one
          vertical axis and each half is cut on that axis, so the two bands meet flush.
  head  = one circle;  beak = triangle whose sides are tangent to a small root circle
  body  = BACK ∩ BELLY (intersection of two circles)
  wing  = crescent (circle minus circle) cut out of the body
  legs  = two rectangles

Every outline in the emitted SVGs is an arc (A) or a line (L). Change the construction values
below and re-run to regenerate all assets.

Usage (from the repo root):
  python3 scripts/brand/egret_mark.py              # SVGs + PNG/ICO
  python3 scripts/brand/egret_mark.py --svg-only   # SVGs only
  python3 scripts/brand/egret_mark.py --docs ../docs   # also refresh the docs site copies

PNG/ICO need headless Chrome (set CHROME=/path/to/chrome if it is not found) and either
macOS `sips` or Pillow for downscaling.
"""
import argparse
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PI = math.pi

# ---------------------------------------------------------------- construction (512 grid)
def circle3(a, b, c):
    """circle through three points → (cx, cy, r)"""
    (ax, ay), (bx, by), (cx, cy) = a, b, c
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    ux = ((ax*ax + ay*ay) * (by - cy) + (bx*bx + by*by) * (cy - ay) + (cx*cx + cy*cy) * (ay - by)) / d
    uy = ((ax*ax + ay*ay) * (cx - bx) + (bx*bx + by*by) * (ax - cx) + (cx*cx + cy*cy) * (bx - ax)) / d
    return (ux, uy, math.hypot(ax - ux, ay - uy))


def tangent_points(cx, cy, r, p):
    """the two points where lines from p touch circle (cx, cy, r)"""
    dx, dy = p[0] - cx, p[1] - cy
    a, b = math.acos(r / math.hypot(dx, dy)), math.atan2(dy, dx)
    return [(cx + r * math.cos(b + s * a), cy + r * math.sin(b + s * a)) for s in (1, -1)]


X = 300                                  # neck axis
W = 24                                   # neck weight
A = (X, 252); RA_IN, RA_OUT = 40, 40 + W         # lower bowl, bulges forward
B = (X, A[1] - RA_OUT - 20); RB_IN, RB_OUT = 20, 20 + W   # upper bowl, bulges back
# flush joint: top band of A == bottom band of B
assert (A[1] - RA_OUT, A[1] - RA_IN) == (B[1] + RB_IN, B[1] + RB_OUT)
HEAD = (X + 12, B[1] - (RB_IN + RB_OUT) / 2 - 1, 16.5)
TIP = (452, HEAD[1] + 22)
ROOT = (HEAD[0] + 9, HEAD[1] + 3, 8.5)           # beak root circle
BEAK = [TIP] + tangent_points(*ROOT, TIP)
TAIL = (58, 384)
FEET_Y = 470
BACK = circle3(TAIL, (180, 314), (X + 2, A[1] + RA_IN - 2))
BELLY = circle3(TAIL, (220, 402), (A[0] + RA_OUT - 22, A[1] + 50))
WING_O = circle3((126, 372), (220, 346), (300, 336))
WING_I = circle3((126, 372), (220, 351), (300, 336))
LEGS = [[(282, 360), (290, 360), (286, FEET_Y), (278, FEET_Y)],
        [(236, 384), (244, 386), (190, FEET_Y), (182, 468)]]

# centre the bird in the tile
TOP = min(B[1] - RB_OUT, HEAD[1] - HEAD[2])
DX, DY = 256 - (TAIL[0] + TIP[0]) / 2, 256 - (TOP + FEET_Y) / 2

DARK, ORANGE, ORANGE_ON_LIGHT = "#0d1117", "#f5a524", "#e8891a"

# ---------------------------------------------------------------- path emission (M / L / A / Z)
def num(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


def pt(p):
    return f"{num(p[0] + DX)} {num(p[1] + DY)}"


def on(c, r, a):
    return (c[0] + r * math.cos(a), c[1] + r * math.sin(a))


def arc_to(c, r, a0, a1, ccw):
    """arc on circle (c, r) from angle a0 to a1; ccw=True is counter-clockwise on screen"""
    span = (a0 - a1) % (2 * PI) if ccw else (a1 - a0) % (2 * PI)
    return f"A{num(r)} {num(r)} 0 {int(span > PI)} {0 if ccw else 1} {pt(on(c, r, a1))}"


def inside(p, c):
    return math.dist(p, c[:2]) < c[2] - 1e-6


def intersections(c1, c2):
    (x1, y1, r1), (x2, y2, r2) = c1, c2
    d = math.dist((x1, y1), (x2, y2))
    a = (r1*r1 - r2*r2 + d*d) / (2 * d); h = math.sqrt(r1*r1 - a*a)
    mx, my = x1 + a * (x2 - x1) / d, y1 + a * (y2 - y1) / d
    return [(mx + h * (y2 - y1) / d, my - h * (x2 - x1) / d), (mx - h * (y2 - y1) / d, my + h * (x2 - x1) / d)]


def angle(c, p):
    return math.atan2(p[1] - c[1], p[0] - c[0])


def arc_between(c, p, q, other, want_inside):
    """arc of c from p to q, taking the side whose midpoint is (not) inside `other`"""
    a0, a1 = angle(c, p), angle(c, q)
    for ccw in (False, True):
        span = -((a0 - a1) % (2 * PI)) if ccw else (a1 - a0) % (2 * PI)
        if inside(on(c, c[2], a0 + span / 2), other) == want_inside:
            return arc_to(c, c[2], a0, a1, ccw)
    raise ValueError("circles do not form the expected region")


def lens(c1, c2):
    p, q = intersections(c1, c2)
    return f"M{pt(p)} {arc_between(c1, p, q, c2, True)} {arc_between(c2, q, p, c1, True)}Z"


def crescent(outer, inner):
    p, q = intersections(outer, inner)
    return f"M{pt(p)} {arc_between(outer, p, q, inner, False)} {arc_between(inner, q, p, outer, True)}Z"


def polygon(points):
    return "M" + " L".join(pt(p) for p in points) + "Z"


NECK = (f"M{pt((X, A[1] + RA_OUT))} "
        f"{arc_to(A, RA_OUT, PI/2, -PI/2, True)} "               # front of the lower bowl
        f"{arc_to(B, RB_IN, PI/2, 3*PI/2, False)} "              # inner edge of the upper bowl
        f"L{pt((HEAD[0], B[1] - RB_IN))} L{pt((HEAD[0], B[1] - RB_OUT))} L{pt((X, B[1] - RB_OUT))} "
        f"{arc_to(B, RB_OUT, 3*PI/2, PI/2, True)} "              # back of the upper bowl
        f"{arc_to(A, RA_IN, -PI/2, PI/2, False)}Z")              # inner edge of the lower bowl
HEAD_PATH = (f"M{pt((HEAD[0] - HEAD[2], HEAD[1]))} "
             f"{arc_to(HEAD, HEAD[2], PI, 0, False)} {arc_to(HEAD, HEAD[2], 0, PI, False)}Z")
BODY, WING = lens(BACK, BELLY), crescent(WING_O, WING_I)
for p in intersections(WING_O, WING_I):
    assert inside(p, BACK) and inside(p, BELLY), "the wing cut must sit inside the body"


def svg(fg="#ffffff", accent=ORANGE, bg=DARK, tile=True, legs=True, wing=True, label=True, rx=112):
    """the mark as an SVG document; legs/wing are dropped for the 16 px favicon"""
    out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"'
           + (' role="img" aria-label="Egret"' if label else "") + ">"]
    if tile:
        out.append(f'<rect width="512" height="512" rx="{rx}" fill="{bg}"/>')
    out.append(f'<path fill="{fg}" fill-rule="evenodd" d="{BODY}' + (f" {WING}" if wing else "") + '"/>')
    out.append(f'<path fill="{fg}" d="{NECK}"/><path fill="{fg}" d="{HEAD_PATH}"/>')
    if legs:
        out.append(f'<path fill="{fg}" d="' + " ".join(polygon(l) for l in LEGS) + '"/>')
    out.append(f'<path fill="{accent}" d="{polygon(BEAK)}"/>')
    out.append("</svg>")
    return "".join(out)


def construction_svg():
    """the mark, faded, with every construction circle and the neck axis on top"""
    def circ(c, **kw):
        attrs = "".join(f' {k.replace("_", "-")}="{v}"' for k, v in kw.items())
        return f'<circle cx="{num(c[0] + DX)}" cy="{num(c[1] + DY)}" r="{num(c[2])}"{attrs}/>'
    line = dict(fill="none", stroke="#e5484d", stroke_width="1.2")
    circles = [(*A, RA_IN), (*A, RA_OUT), (*B, RB_IN), (*B, RB_OUT), HEAD, ROOT, BACK, BELLY, WING_O, WING_I]
    parts = [f'<g opacity=".2">{inner(svg(fg=DARK, accent=DARK, tile=False, label=False))}</g>']
    parts += [circ(c, **line) for c in circles]
    parts += [circ((c[0], c[1], 2.4), fill="#e5484d") for c in (A, B, HEAD, BACK, BELLY)]
    parts.append(f'<path d="{polygon(BEAK)} ' + " ".join(polygon(l) for l in LEGS) + '" fill="none" stroke="#e5484d" stroke-width="1.2"/>')
    parts.append(f'<line x1="{num(X + DX)}" y1="40" x2="{num(X + DX)}" y2="480" stroke="#3b82f6" stroke-width="1" stroke-dasharray="4 4"/>')
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">'
            '<rect width="512" height="512" rx="112" fill="#f4f1ea"/>' + "".join(parts) + "</svg>\n")


def inner(doc):
    return re.sub(r"^<svg[^>]*>|</svg>$", "", doc)


# ---------------------------------------------------------------- writers
def replace_group(text, opener, new):
    """swap the <g …> element that starts with `opener` (balanced on <g>/</g>)"""
    i, depth = text.index(opener), 0
    for m in re.finditer(r"<g\b|</g>", text[i:]):
        depth += 1 if m.group() == "<g" else -1
        if depth == 0:
            return text[:i] + new + text[i + m.end():]
    raise ValueError(f"unbalanced group: {opener}")


def write_svgs():
    pub, gh = REPO / "frontend/public", REPO / ".github/assets"
    (pub / "favicon.svg").write_text(svg() + "\n")
    (gh / "egret-mark-construction.svg").write_text(construction_svg())
    # wordmarks: the bird fills the 100-unit box left of the letters
    x0, x1 = TAIL[0] + DX, TIP[0] + DX
    y0, y1 = TOP + DY, FEET_Y + DY
    sc = 96 / (x1 - x0); tx, ty = 4 - x0 * sc, 50 - (y0 + y1) / 2 * sc
    for theme, fg, accent in (("light", DARK, ORANGE_ON_LIGHT), ("dark", "#ffffff", ORANGE)):
        bird = (f'<g transform="translate({tx:.2f} {ty:.2f}) scale({sc:.4f})">'
                f'{inner(svg(fg=fg, accent=accent, tile=False, label=False))}</g>')
        for d in (pub, gh):
            p = d / f"egret-wordmark-{theme}.svg"; s = p.read_text()
            letters = s[s.index('<g transform="translate(112'):]
            p.write_text(s[:s.index(">") + 1] + bird + letters)
    # architecture diagrams: 30 px tile next to the "Egret" title
    for theme in ("light", "dark"):
        p = gh / f"architecture-{theme}.svg"; s = p.read_text()
        opener = '<g transform="translate(308 66) scale('
        new = f'<g transform="translate(308 66) scale({30/512:.6f})">{inner(svg(label=False))}</g>'
        p.write_text(replace_group(s, opener, new))


def find_chrome():
    for c in (os.environ.get("CHROME"), "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
              shutil.which("google-chrome"), shutil.which("chromium"), shutil.which("chromium-browser")):
        if c and Path(c).exists():
            return c
    sys.exit("headless Chrome not found; set CHROME=/path/to/chrome or pass --svg-only")


def render(chrome, doc, out_png, size=1024):
    with tempfile.TemporaryDirectory() as t:
        (Path(t) / "m.svg").write_text(doc)
        (Path(t) / "r.html").write_text(f"<html><body style='margin:0;background:transparent'>"
                                        f"<img src='m.svg' width={size} height={size}></body></html>")
        subprocess.run([chrome, "--headless=new", "--disable-gpu", "--allow-file-access-from-files",
                        f"--window-size={size},{size}", "--default-background-color=00000000",
                        "--hide-scrollbars", f"--screenshot={out_png}", f"file://{t}/r.html"],
                       check=True, capture_output=True)


def downscale(src, dst, px):
    if shutil.which("sips"):
        subprocess.run(["sips", "-z", str(px), str(px), str(src), "--out", str(dst)], check=True, capture_output=True)
        return
    try:
        from PIL import Image
    except ImportError:
        sys.exit("need macOS `sips` or Pillow to downscale PNGs")
    Image.open(src).resize((px, px), Image.LANCZOS).save(dst)


def write_ico(dst, pngs):
    data = [Path(p).read_bytes() for p, _ in pngs]
    head = struct.pack("<HHH", 0, 1, len(data)); offset = 6 + 16 * len(data); entries = b""
    for d, (_, px) in zip(data, pngs):
        entries += struct.pack("<BBBBHHII", px % 256, px % 256, 0, 0, 1, 32, len(d), offset); offset += len(d)
    Path(dst).write_bytes(head + entries + b"".join(data))


def write_rasters():
    chrome, pub = find_chrome(), REPO / "frontend/public"
    with tempfile.TemporaryDirectory() as t:
        full, small, apple = (Path(t) / n for n in ("full.png", "small.png", "apple.png"))
        render(chrome, svg(), full)
        render(chrome, svg(legs=False, wing=False), small)
        render(chrome, svg(rx=0), apple)                  # iOS rounds the corners itself
        downscale(small, pub / "favicon-16.png", 16)
        downscale(full, pub / "favicon-32.png", 32)
        downscale(full, Path(t) / "ico-48.png", 48)
        downscale(apple, pub / "apple-touch-icon.png", 180)
        downscale(full, pub / "icon-192.png", 192)
        downscale(full, pub / "icon-512.png", 512)
        write_ico(pub / "favicon.ico", [(pub / "favicon-16.png", 16), (pub / "favicon-32.png", 32),
                                        (Path(t) / "ico-48.png", 48)])


# the docs site (egret-data/docs) ships byte-identical copies of these assets
DOCS_COPIES = {
    "frontend/public/favicon.svg": ["static/img/favicon.svg", "static/img/logo.svg"],
    "frontend/public/favicon.ico": ["static/favicon.ico"],
    "frontend/public/favicon-32.png": ["static/img/favicon-32.png"],
    "frontend/public/apple-touch-icon.png": ["static/img/apple-touch-icon.png"],
    "frontend/public/egret-wordmark-light.svg": ["static/img/wordmark-light.svg"],
    "frontend/public/egret-wordmark-dark.svg": ["static/img/wordmark-dark.svg"],
    ".github/assets/architecture-light.svg": ["static/img/architecture-light.svg"],
    ".github/assets/architecture-dark.svg": ["static/img/architecture-dark.svg"],
}


def copy_to_docs(docs):
    docs = Path(docs)
    if not (docs / "docusaurus.config.ts").exists():
        sys.exit(f"{docs} is not an egret-data/docs checkout")
    for src, dsts in DOCS_COPIES.items():
        for dst in dsts:
            shutil.copyfile(REPO / src, docs / dst)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--svg-only", action="store_true", help="skip PNG/ICO rendering")
    ap.add_argument("--docs", metavar="PATH", help="also copy the assets into an egret-data/docs checkout")
    args = ap.parse_args()
    write_svgs()
    if not args.svg_only:
        write_rasters()
    if args.docs:
        copy_to_docs(args.docs)
    print("egret mark assets written")
