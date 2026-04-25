#!/usr/bin/env python3
"""
DAY-170 tray-icon rasteriser.

Rasterises the Dayseam tray mark (`assets/brand/dayseam-tray.svg`)
down to the 32x32 + 64x64 PNGs the Tauri desktop shell loads at
boot for the menubar tray. Kept as a small self-contained Pillow
script so the repo does not have to depend on `rsvg-convert`,
`magick`, or any other system tool — Pillow ships on every
developer's Python and CI image via a one-line install.

Coordinates and palette are mirrored from the SVG here rather than
parsed out of it, because Pillow has no native SVG parser and the
geometry is genuinely tiny (five lines + one dashed seam). If the
SVG geometry ever changes, update this script in the same commit
— the file-level comment in `dayseam-tray.svg` makes that the
contract.

Run from the repo root:

    python3 scripts/rasterise-tray-icon.py

Outputs:

    apps/desktop/src-tauri/icons/tray-icon.png      (32x32)
    apps/desktop/src-tauri/icons/tray-icon@2x.png   (64x64)

Uses 8x super-sampling + Lanczos downsample so the strand edges
anti-alias cleanly at the target raster size; drawing straight at
32px leaves the thin strokes with staircase artefacts on the
diagonal strands.
"""

from __future__ import annotations

from pathlib import Path
from PIL import Image, ImageDraw

# Canonical SVG canvas size. We use 1024-coordinates internally so
# the numbers here line up 1:1 with `assets/brand/dayseam-tray.svg`
# for readability.
CANVAS = 1024

# Convergence point (the "anchor" the five strands meet at). Lifted
# straight from the SVG so a diff between the two files is trivially
# verifiable.
CONVERGENCE = (680, 512)

# Five strands as (start_point, hex_colour). Angles (–36°, –18°, 0°,
# +18°, +36°) are implied by the start points; we do not recompute
# them here because the SVG is the geometry source of truth.
STRANDS: list[tuple[tuple[int, int], str]] = [
    ((292, 230), "#E89A2C"),
    ((224, 364), "#2B8AA0"),
    ((200, 512), "#D94F6E"),
    ((224, 660), "#5BA567"),
    ((292, 794), "#4D6DD0"),
]

# Seam: horizontal running stitch from the convergence point to the
# right edge. Drawn in a neutral secondary grey so the same PNG
# reads on both the light and dark macOS menubars.
SEAM_END = (980, 512)
SEAM_COLOUR = "#8E8E93"
SEAM_DASH = 44  # on pixels (SVG units)
SEAM_GAP = 24   # off pixels (SVG units)

# Stroke width in SVG units. Matches the tray SVG exactly, for the
# reasons the SVG's file-level comment lays out.
STROKE_WIDTH = 88

# 8x super-sampling before the final Lanczos resize. 8x hits a
# sweet spot: high enough that antialiasing is indistinguishable
# from a vector render, low enough that the intermediate image
# still fits comfortably in memory for a 64x64 target.
SUPERSAMPLE = 8


def _scale(value: int, target_size: int) -> int:
    """Map a 1024-canvas coordinate to the super-sampled target."""
    return int(round(value / CANVAS * target_size * SUPERSAMPLE))


def _scaled_point(point: tuple[int, int], target_size: int) -> tuple[int, int]:
    return (_scale(point[0], target_size), _scale(point[1], target_size))


def _render(target_size: int) -> Image.Image:
    """Render the tray mark at `target_size × target_size`.

    Draws everything at `target_size × SUPERSAMPLE` then downsamples
    with Lanczos. Round line caps are faked by sticking small filled
    circles at each strand endpoint — Pillow's `line()` only offers
    butt/miter caps natively, so on diagonals the un-capped ends
    would otherwise look square-ish after downsampling.
    """
    supersample = target_size * SUPERSAMPLE
    image = Image.new("RGBA", (supersample, supersample), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    stroke_px = _scale(STROKE_WIDTH, target_size)
    # Radius for the round-cap filled circles — half the stroke so
    # the cap matches the line's visual width.
    cap_radius = stroke_px // 2
    convergence_px = _scaled_point(CONVERGENCE, target_size)

    for start, colour in STRANDS:
        start_px = _scaled_point(start, target_size)
        draw.line([start_px, convergence_px], fill=colour, width=stroke_px)
        # Round caps at both ends.
        for cap in (start_px, convergence_px):
            draw.ellipse(
                (
                    cap[0] - cap_radius,
                    cap[1] - cap_radius,
                    cap[0] + cap_radius,
                    cap[1] + cap_radius,
                ),
                fill=colour,
            )

    # Dashed seam. Pillow has no `stroke-dasharray`, so we walk the
    # segment by hand and stamp `SEAM_DASH` on / `SEAM_GAP` off in
    # SVG units until we reach `SEAM_END`.
    seam_x = CONVERGENCE[0]
    while seam_x < SEAM_END[0]:
        dash_end = min(seam_x + SEAM_DASH, SEAM_END[0])
        start_px = _scaled_point((seam_x, CONVERGENCE[1]), target_size)
        end_px = _scaled_point((dash_end, CONVERGENCE[1]), target_size)
        draw.line([start_px, end_px], fill=SEAM_COLOUR, width=stroke_px)
        for cap in (start_px, end_px):
            draw.ellipse(
                (
                    cap[0] - cap_radius,
                    cap[1] - cap_radius,
                    cap[0] + cap_radius,
                    cap[1] + cap_radius,
                ),
                fill=SEAM_COLOUR,
            )
        seam_x = dash_end + SEAM_GAP

    return image.resize((target_size, target_size), Image.LANCZOS)


def main() -> None:
    repo_root = Path(__file__).resolve().parents[1]
    icons_dir = repo_root / "apps" / "desktop" / "src-tauri" / "icons"
    icons_dir.mkdir(parents=True, exist_ok=True)

    targets = [
        (32, icons_dir / "tray-icon.png"),
        (64, icons_dir / "tray-icon@2x.png"),
    ]

    for size, path in targets:
        rendered = _render(size)
        rendered.save(path, format="PNG", optimize=True)
        print(f"wrote {path.relative_to(repo_root)} ({size}x{size})")


if __name__ == "__main__":
    main()
