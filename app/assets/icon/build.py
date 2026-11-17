#!/usr/bin/env python3
"""Build the Mini Launcher icon set from the master art.

Usage:  python3 app/assets/icon/build.py [--preview]

Input (next to this script):
  mini-launcher.png            1024x1024 master: blue disc, black NFC waves, hole
Outputs (next to this script):
  mini-launcher-small.png      16 px simplified variant (pixel art below)
  mini-launcher-small-20.png   20 px simplified variant
  mini-launcher.ico            16, 20, 24, 32, 40, 48, 64, 256 px; 16/20 use the
                               small variant, the rest are downscaled master
  preview.png                  (--preview) sign-off sheet: 8x nearest-neighbour
                               zoom + 1x, on dark and light backgrounds

At 16/20 px the master's three thin waves smear and black-on-blue loses
contrast, so the small sizes are hand-placed pixel art: two thick waves per
side and a pixel-snapped hole. Only the disc edge is anti-aliased.

Requires Pillow. Re-run after changing the master or the grids below.
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
MASTER = HERE / "mini-launcher.png"
ICO = HERE / "mini-launcher.ico"
PREVIEW = HERE / "preview.png"

ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]

BLUE = (8, 107, 162, 255)  # disc fill, sampled from the master
RIM = (23, 135, 197, 255)  # lighter rim, sampled from the master
WAVE = (0, 0, 0, 255)
CLEAR = (0, 0, 0, 0)

# Left half of each small icon; the right half is the mirror image.
# k = black wave, o = transparent hole, . = disc (or outside the disc).
SMALL_16 = [
    "........",
    "........",
    "........",
    "....k...",
    "...kk...",
    "..kk..k.",
    "..kk.k..",
    "..kk.k.o",
    "..kk.k.o",
    "..kk.k..",
    "..kk..k.",
    "...kk...",
    "....k...",
    "........",
    "........",
    "........",
]
SMALL_20 = [
    "..........",
    "..........",
    "..........",
    "..........",
    "...kk.....",
    "..kk...k..",
    "..kk..kk..",
    "..kk..k...",
    "..kk..k..o",
    "..kk..k.oo",
    "..kk..k.oo",
    "..kk..k..o",
    "..kk..k...",
    "..kk..kk..",
    "..kk...k..",
    "...kk.....",
    "..........",
    "..........",
    "..........",
    "..........",
]
SMALL = {16: SMALL_16, 20: SMALL_20}
SMALL_FILES = {16: HERE / "mini-launcher-small.png", 20: HERE / "mini-launcher-small-20.png"}


def disc(size: int) -> Image.Image:
    """Anti-aliased blue disc with a ~0.6 px lighter rim."""
    ss = 16
    n = size * ss
    im = Image.new("RGBA", (n, n), CLEAR)
    d = ImageDraw.Draw(im)
    d.ellipse([0, 0, n - 1, n - 1], fill=RIM)
    w = ss * 0.6
    d.ellipse([w, w, n - 1 - w, n - 1 - w], fill=BLUE)
    return im.resize((size, size), Image.Resampling.BOX)


def draw_small(size: int) -> Image.Image:
    grid = [row + row[::-1] for row in SMALL[size]]
    assert len(grid) == size and all(len(r) == size for r in grid), size
    im = disc(size)
    px = im.load()
    for y, row in enumerate(grid):
        for x, ch in enumerate(row):
            if ch == "k":
                px[x, y] = WAVE
            elif ch == "o":
                px[x, y] = CLEAR
    return im


def build_frames(master: Image.Image) -> dict[int, Image.Image]:
    return {
        s: draw_small(s) if s in SMALL else master.resize((s, s), Image.Resampling.LANCZOS)
        for s in ICO_SIZES
    }


def write_preview(frames: dict[int, Image.Image], master: Image.Image) -> None:
    zoom, pad = 8, 8
    bgs = [(0x20, 0x20, 0x20, 255), (0xF3, 0xF3, 0xF3, 255)]
    # 16/20 small variant, 24/32 master, then plain-downscaled master at 16
    # for comparison with what the small variant replaces.
    tiles = [frames[16], frames[20], frames[24], frames[32]]
    tiles.append(master.resize((16, 16), Image.Resampling.LANCZOS))
    width = pad + sum(t.width * zoom + pad for t in tiles) + sum(t.width + pad for t in tiles)
    row_h = max(t.width for t in tiles) * zoom + 2 * pad
    sheet = Image.new("RGBA", (width, row_h * len(bgs)))
    for i, bg in enumerate(bgs):
        y0 = i * row_h
        sheet.paste(Image.new("RGBA", (width, row_h), bg), (0, y0))
        x = pad
        for t in tiles:
            big = t.resize((t.width * zoom, t.height * zoom), Image.Resampling.NEAREST)
            sheet.alpha_composite(big, (x, y0 + pad))
            x += big.width + pad
        for t in tiles:
            sheet.alpha_composite(t, (x, y0 + pad))
            x += t.width + pad
    sheet.convert("RGB").save(PREVIEW, optimize=True)


def main() -> None:
    master = Image.open(MASTER).convert("RGBA")
    frames = build_frames(master)
    for s, path in SMALL_FILES.items():
        frames[s].save(path, optimize=True)
    frames[256].save(
        ICO,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=[frames[s] for s in ICO_SIZES if s != 256],
    )
    if "--preview" in sys.argv:
        write_preview(frames, master)


if __name__ == "__main__":
    main()
