"""Draws the repository's social preview: the app icon, the name and the tagline.

GitHub shows the image (1280 x 640) when the repository is linked in chats and social media,
on light and dark pages alike, so the background is a calm dark indigo that both frame. The
icon is drawn by make_icon.render at full size, so it is as sharp as the app's own.

Writes docs/assets/social-preview.png. GitHub has no API for it: upload it by hand at
Settings ▸ General ▸ Social preview.

Run from the repository root on Windows (it uses Segoe UI): python tools/icon/social_preview.py
Needs Pillow.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parent))
import make_icon  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "docs" / "assets" / "social-preview.png"

W, H = 1280, 640
FONTS = Path(r"C:\Windows\Fonts")
TITLE_FONT = FONTS / "seguisb.ttf"  # Segoe UI Semibold
TAGLINE_FONT = FONTS / "segoeui.ttf"

NAME = "Lastdrop"
TAGLINE = ["How much AI is left in the tank,", "one click away."]
FOOTER = "A Windows 11 tray app  ·  open source"

BG_TOP = (0x14, 0x16, 0x34)
BG_BOTTOM = (0x0B, 0x0D, 0x22)
GLOW = (0x3A, 0x3F, 0xC8)  # between the ring's blues, behind the icon
TITLE = (0xF4, 0xF5, 0xFF)
TAGLINE_COLOUR = (0xB9, 0xBE, 0xEA)
FOOTER_COLOUR = (0x7D, 0x83, 0xC0)

ICON = 300
ICON_X, ICON_Y = 130, (H - ICON) // 2
TEXT_X = ICON_X + ICON + 80


def background() -> Image.Image:
    col = Image.new("RGB", (1, H))
    for y in range(H):
        t = y / (H - 1)
        col.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)))
    img = col.resize((W, H), Image.BILINEAR).convert("RGBA")
    # A soft glow behind the icon.
    glow = Image.new("L", (W, H), 0)
    cx, cy, r = ICON_X + ICON / 2, ICON_Y + ICON / 2, ICON * 0.62
    ImageDraw.Draw(glow).ellipse([cx - r, cy - r, cx + r, cy + r], fill=255)
    glow = glow.filter(ImageFilter.GaussianBlur(110)).point(lambda v: round(v * 0.38))
    top = Image.new("RGBA", (W, H), GLOW + (0,))
    top.putalpha(glow)
    return Image.alpha_composite(img, top)


def main() -> None:
    img = background()
    img.alpha_composite(make_icon.render(ICON), (ICON_X, ICON_Y))

    d = ImageDraw.Draw(img)
    title = ImageFont.truetype(str(TITLE_FONT), 112)
    tagline = ImageFont.truetype(str(TAGLINE_FONT), 44)
    footer = ImageFont.truetype(str(TAGLINE_FONT), 28)

    # The text block is centred on the icon's middle.
    lines = [(NAME, title, TITLE, 18)]
    lines += [(t, tagline, TAGLINE_COLOUR, 8) for t in TAGLINE]
    lines += [(FOOTER, footer, FOOTER_COLOUR, 0)]
    gaps = {len(lines) - 2: 34}  # extra air above the footer
    heights = []
    for i, (text, font, _, after) in enumerate(lines):
        asc, desc = font.getmetrics()
        heights.append(asc + desc + after + gaps.get(i, 0))
    y = ICON_Y + ICON / 2 - sum(heights) / 2
    for (text, font, colour, _), h in zip(lines, heights):
        d.text((TEXT_X, y), text, font=font, fill=colour)
        y += h

    OUT.parent.mkdir(parents=True, exist_ok=True)
    img.convert("RGB").save(OUT, optimize=True)
    print("wrote", OUT, OUT.stat().st_size, "bytes")


if __name__ == "__main__":
    main()
