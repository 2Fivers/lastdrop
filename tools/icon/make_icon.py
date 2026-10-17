"""Draws the Lastdrop app icon: a power symbol whose stem is the Lastdrop drop.

The ring is an open circle (a power button) with round ends; the drop stands in its gap. The
drop is `lastdrop_core::glyph::DROP_PATH`, so the app icon and the tray icon share one shape.
The concept came from an OpenAI Codex image ("power-1"); this script redraws it as vector
geometry so every size is sharp and the small ones are tuned per pixel size.

Writes, into crates/app/icons/:
  lastdrop.ico        16, 20, 24, 32, 40, 48, 64 (32-bit BMP) and 256 (PNG); the exe resource
  lastdrop-<n>.png    every size in the .ico, for previews and the Slint window icon
  lastdrop.svg        the flat vector version (no shading), for documents and the web

Run from the repository root: python tools/icon/make_icon.py
Needs Pillow.
"""

from __future__ import annotations

import io
import math
import struct
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parents[2] / "crates" / "app" / "icons"

# The design frame: the artwork fills 256 x 256 units.
FRAME = 256.0

# DROP_PATH (glyph.rs) on its 16 x 16 grid: tip, the sides' control points, the round bottom.
DROP_TIP = (8.0, 1.2)
DROP_SIDE_CP = 6.9
DROP_CENTER = (8.0, 10.1)
DROP_R = 4.8

RING_TOP = (0x74, 0x79, 0xFF)
RING_BOTTOM = (0x2C, 0x23, 0xEF)
RING_EDGE = (0x1B, 0x12, 0xC9)
DROP_STOPS = [(0.0, (0x13, 0x8B, 0xF4)), (0.55, (0x1C, 0xB2, 0xFA)), (1.0, (0x62, 0xED, 0xF8))]
DROP_EDGE = (0x0A, 0x64, 0xD8)


@dataclass(frozen=True)
class Style:
    """The geometry and effects for one pixel size (design-frame units unless noted)."""

    margin: float  # px of empty border at the target size
    ring_width: float  # stroke width of the ring
    gap: float  # half the gap at the top, degrees from straight up
    drop_r: float  # radius of the drop's round bottom
    tip_y: float  # where the drop's tip is
    shading: bool  # edge darkening, gloss and specular
    shadow: bool  # soft shadow under the whole artwork
    supersample: int


def style_for(size: int) -> Style:
    # The smaller the icon, the wider the gap, the thinner the ring and the smaller and lower
    # the drop, so a pixel of air stays between the drop and the ring's ends; no margin and
    # flat colours below 32 px (gloss turns to mud there).
    if size <= 16:
        return Style(0, 40, 50, 52, 16, False, False, 16)
    if size <= 20:
        return Style(0, 40, 49, 54, 12, False, False, 16)
    if size <= 24:
        return Style(0, 42, 48, 56, 10, False, False, 16)
    if size <= 32:
        return Style(0.5, 44, 48, 56, 8, True, False, 16)
    if size <= 64:
        return Style(size / 32, 46, 44, 60, 4, True, True, 8)
    return Style(size / 32, 48, 42, 62, 2, True, True, 8)


# --- geometry -----------------------------------------------------------------------------


def ring_geometry(st: Style) -> tuple[float, float, float]:
    """Centre x, centre y and mid radius of the ring: its outer edge touches the frame's
    left, right and bottom sides."""
    r_mid = FRAME / 2 - st.ring_width / 2
    return FRAME / 2, FRAME - FRAME / 2, r_mid


def ring_polygon(st: Style, steps: int = 720) -> list[tuple[float, float]]:
    cx, cy, r_mid = ring_geometry(st)
    w2 = st.ring_width / 2
    g0, g1 = math.radians(st.gap), math.radians(360 - st.gap)

    def at(theta: float, r: float) -> tuple[float, float]:
        return cx + r * math.sin(theta), cy - r * math.cos(theta)

    def cap(theta: float, forward: bool) -> list[tuple[float, float]]:
        # A half circle round the end of the stroke, from the outer edge to the inner one.
        c = at(theta, r_mid)
        u = (math.sin(theta), -math.cos(theta))  # outwards
        d = (math.cos(theta), math.sin(theta))  # along increasing theta
        sgn = 1 if forward else -1
        pts = []
        for i in range(1, 64):
            phi = math.pi * i / 64
            pts.append(
                (
                    c[0] + w2 * (sgn * u[0] * math.cos(phi) + sgn * d[0] * math.sin(phi)),
                    c[1] + w2 * (sgn * u[1] * math.cos(phi) + sgn * d[1] * math.sin(phi)),
                )
            )
        return pts

    outer = [at(g0 + (g1 - g0) * i / steps, r_mid + w2) for i in range(steps + 1)]
    inner = [at(g1 - (g1 - g0) * i / steps, r_mid - w2) for i in range(steps + 1)]
    return outer + cap(g1, True) + inner + cap(g0, False)


def drop_transform(st: Style) -> tuple[float, float, float]:
    """Scale and offset that put DROP_PATH centred over the ring: the round bottom sits a
    little above the ring's centre, the tip rises through the gap to the frame's top."""
    s = st.drop_r / DROP_R
    cx, cy, _ = ring_geometry(st)
    oy = st.tip_y - DROP_TIP[1] * s
    ox = cx - DROP_CENTER[0] * s
    return s, ox, oy


def cubic(p0, p1, p2, p3, n):
    out = []
    for i in range(1, n + 1):
        t = i / n
        a, b, c, d = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t * t, t**3
        out.append(
            (
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            )
        )
    return out


def drop_polygon(st: Style, n: int = 256) -> list[tuple[float, float]]:
    s, ox, oy = drop_transform(st)

    def p(x, y):
        return ox + x * s, oy + y * s

    tip = p(*DROP_TIP)
    left = p(DROP_CENTER[0] - DROP_R, DROP_CENTER[1])
    right = p(DROP_CENTER[0] + DROP_R, DROP_CENTER[1])
    pts = [tip]
    pts += cubic(tip, tip, p(DROP_CENTER[0] - DROP_R, DROP_SIDE_CP), left, n)
    c = p(*DROP_CENTER)
    r = DROP_R * s
    # The bottom half circle, left to right through the bottom.
    for i in range(1, n):
        a = math.pi - math.pi * i / n
        pts.append((c[0] + r * math.cos(a), c[1] + r * math.sin(a)))
    pts.append(right)
    pts += cubic(right, p(DROP_CENTER[0] + DROP_R, DROP_SIDE_CP), tip, tip, n)[:-1]
    return pts


def drop_specular(st: Style) -> list[tuple[float, float]]:
    """A line along the drop's upper left side, inset, where the light catches it."""
    s, ox, oy = drop_transform(st)
    c = (ox + DROP_CENTER[0] * s, oy + DROP_CENTER[1] * s)
    pts = drop_polygon(st, 128)
    # The left side from just below the tip down to a little past the widest point.
    left = [q for q in pts if q[0] < c[0]]
    top_y = pts[0][1]
    span = c[1] - top_y
    sel = [q for q in left if top_y + 0.32 * span < q[1] < c[1] + 0.25 * DROP_R * s]
    k, dx = 0.74, -0.07 * DROP_R * s
    return [(c[0] + (q[0] - c[0]) * k + dx, c[1] + (q[1] - c[1]) * k) for q in sel]


# --- drawing ------------------------------------------------------------------------------


def place(pts, size: int, st: Style):
    """Design-frame points to supersampled pixels."""
    ss = st.supersample
    scale = (size - 2 * st.margin) / FRAME * ss
    off = st.margin * ss
    return [(off + x * scale, off + y * scale) for x, y in pts]


def mask(size: int, st: Style, pts) -> Image.Image:
    n = size * st.supersample
    m = Image.new("L", (n, n), 0)
    ImageDraw.Draw(m).polygon(place(pts, size, st), fill=255)
    return m


def vertical_gradient(n: int, y0: float, y1: float, stops) -> Image.Image:
    col = Image.new("RGB", (1, n))
    px = col.load()
    for y in range(n):
        t = min(1.0, max(0.0, (y - y0) / max(1e-6, y1 - y0)))
        for (ta, ca), (tb, cb) in zip(stops, stops[1:]):
            if t <= tb:
                f = (t - ta) / (tb - ta) if tb > ta else 0
                px[0, y] = tuple(round(a + (b - a) * f) for a, b in zip(ca, cb))
                break
    return col.resize((n, n), Image.NEAREST)


def edge_shade(m: Image.Image, radius: float) -> Image.Image:
    """Alpha that is strongest at the shape's edge and fades inwards."""
    inside = m.filter(ImageFilter.GaussianBlur(radius))
    return ImageChops.multiply(m, ImageChops.invert(inside))


def layer(base: Image.Image, colour, alpha: Image.Image, opacity: float) -> Image.Image:
    a = alpha.point(lambda v: round(v * opacity))
    top = Image.new("RGBA", base.size, colour + (0,))
    top.putalpha(a)
    return Image.alpha_composite(base, top)


def render(size: int) -> Image.Image:
    st = style_for(size)
    ss = st.supersample
    n = size * ss
    unit = (size - 2 * st.margin) / FRAME * ss  # supersampled px per design unit

    ring_pts, drop_pts = ring_polygon(st), drop_polygon(st)
    ring_m, drop_m = mask(size, st, ring_pts), mask(size, st, drop_pts)

    ring_ys = [p[1] for p in place(ring_pts, size, st)]
    drop_ys = [p[1] for p in place(drop_pts, size, st)]

    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    if st.shadow:
        both = ImageChops.lighter(ring_m, drop_m)
        sh = both.filter(ImageFilter.GaussianBlur(4 * unit))
        sh = ImageChops.offset(sh, 0, round(3 * unit))
        img = layer(img, (0, 0, 0), sh, 0.22)

    ring = vertical_gradient(n, min(ring_ys), max(ring_ys), [(0, RING_TOP), (1, RING_BOTTOM)])
    ring = ring.convert("RGBA")
    ring.putalpha(ring_m)
    if st.shading:
        ring = layer(ring, RING_EDGE, edge_shade(ring_m, 9 * unit), 0.55)
        # Gloss along the middle of the tube.
        gloss = Image.new("L", (n, n), 0)
        cx, cy, r_mid = ring_geometry(st)
        g = Style(st.margin, st.ring_width * 0.28, st.gap + 4, st.drop_r, st.tip_y, False, False, ss)
        gloss_pts = []
        for x, y in ring_polygon(g):
            # Shift the thin ring's mid radius inwards a little: the light comes from above.
            dx, dy = x - cx, y - cy
            k = (r_mid - st.ring_width * 0.12) / r_mid
            gloss_pts.append((cx + dx * k, cy + dy * k))
        ImageDraw.Draw(gloss).polygon(place(gloss_pts, size, st), fill=255)
        gloss = gloss.filter(ImageFilter.GaussianBlur(5 * unit))
        ring = layer(ring, (255, 255, 255), ImageChops.multiply(gloss, ring_m), 0.30)
        ring.putalpha(ImageChops.multiply(ring.getchannel("A"), ring_m))
    img = Image.alpha_composite(img, ring)

    drop = vertical_gradient(n, min(drop_ys), max(drop_ys), DROP_STOPS).convert("RGBA")
    drop.putalpha(drop_m)
    if st.shading:
        drop = layer(drop, DROP_EDGE, edge_shade(drop_m, 10 * unit), 0.45)
        spec = Image.new("L", (n, n), 0)
        line = place(drop_specular(st), size, st)
        w = max(1, round(12 * unit))
        d = ImageDraw.Draw(spec)
        d.line(line, fill=255, width=w, joint="curve")
        for q in (line[0], line[-1]):
            d.ellipse([q[0] - w / 2, q[1] - w / 2, q[0] + w / 2, q[1] + w / 2], fill=255)
        spec = spec.filter(ImageFilter.GaussianBlur(2 * unit))
        drop = layer(drop, (255, 255, 255), ImageChops.multiply(spec, drop_m), 0.7)
        drop.putalpha(ImageChops.multiply(drop.getchannel("A"), drop_m))
    img = Image.alpha_composite(img, drop)

    # Box-filter the supersampled image down in premultiplied space.
    return img.convert("RGBa").resize((size, size), Image.BOX).convert("RGBA")


# --- files --------------------------------------------------------------------------------


def bmp_entry(im: Image.Image) -> bytes:
    """A 32-bit BGRA DIB as an .ico stores it: bottom-up, height doubled, an AND mask."""
    w, h = im.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    px = im.load()
    rows = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            rows += bytes((b, g, r, a))
    stride = ((w + 31) // 32) * 4
    and_mask = bytearray()
    for y in range(h - 1, -1, -1):
        bits = bytearray(stride)
        for x in range(w):
            if px[x, y][3] == 0:
                bits[x // 8] |= 0x80 >> (x % 8)
        and_mask += bits
    return header + bytes(rows) + bytes(and_mask)


def write_ico(path: Path, images: list[Image.Image]) -> None:
    entries = []
    for im in sorted(images, key=lambda i: i.width):
        if im.width >= 256:
            buf = io.BytesIO()
            im.save(buf, "PNG", optimize=True)
            entries.append((im.width, buf.getvalue()))
        else:
            entries.append((im.width, bmp_entry(im)))
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for w, data in entries:
        b = 0 if w >= 256 else w
        out += struct.pack("<BBBBHHII", b, b, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in entries:
        out += data
    path.write_bytes(bytes(out))


def svg(st: Style) -> str:
    cx, cy, r_mid = ring_geometry(st)
    g0, g1 = math.radians(st.gap), math.radians(360 - st.gap)
    x0, y0 = cx + r_mid * math.sin(g1), cy - r_mid * math.cos(g1)
    x1, y1 = cx + r_mid * math.sin(g0), cy - r_mid * math.cos(g0)
    s, ox, oy = drop_transform(st)

    def hx(c):
        return "#%02X%02X%02X" % c

    stops = "".join(
        f'<stop offset="{t:g}" stop-color="{hx(c)}"/>' for t, c in DROP_STOPS
    )
    f = lambda v: f"{v:.2f}".rstrip("0").rstrip(".")  # noqa: E731
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-8 -8 272 272" width="256" height="256">\n'
        "  <title>Lastdrop</title>\n"
        "  <defs>\n"
        f'    <linearGradient id="ring" x1="0" y1="{f(cy - r_mid - st.ring_width / 2)}" x2="0" y2="{f(FRAME)}" gradientUnits="userSpaceOnUse">'
        f'<stop offset="0" stop-color="{hx(RING_TOP)}"/><stop offset="1" stop-color="{hx(RING_BOTTOM)}"/></linearGradient>\n'
        f'    <linearGradient id="drop" x1="0" y1="0" x2="0" y2="1">{stops}</linearGradient>\n'
        "  </defs>\n"
        f'  <path d="M{f(x0)} {f(y0)}A{f(r_mid)} {f(r_mid)} 0 1 0 {f(x1)} {f(y1)}" fill="none" '
        f'stroke="url(#ring)" stroke-width="{f(st.ring_width)}" stroke-linecap="round"/>\n'
        f'  <path transform="translate({f(ox)} {f(oy)}) scale({f(s)})" fill="url(#drop)" '
        'd="M8 1.2C8 1.2 3.2 6.9 3.2 10.1A4.8 4.8 0 0 0 12.8 10.1C12.8 6.9 8 1.2 8 1.2Z"/>\n'
        "</svg>\n"
    )


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 256]
    images = []
    for s in sizes:
        im = render(s)
        im.save(OUT / f"lastdrop-{s}.png", optimize=True)
        images.append(im)
    write_ico(OUT / "lastdrop.ico", images)
    (OUT / "lastdrop.svg").write_text(svg(style_for(256)), encoding="utf-8", newline="\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
