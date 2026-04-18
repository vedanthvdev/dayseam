#!/usr/bin/env python3
"""Generate placeholder PNG icons for the Dayseam desktop app.

These are intentionally minimal — a solid brand-tinted square. Before any
release the icons should be regenerated from a proper artwork source using
`pnpm tauri icon path/to/real-icon.png`, which produces ICNS/ICO/PNG sets
in the native sizes Tauri's bundler expects.
"""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

ICON_DIR = Path(__file__).resolve().parent.parent / "apps" / "desktop" / "src-tauri" / "icons"

# Dayseam brand tint: warm daybreak amber on near-black.
BG = (18, 18, 24, 255)
FG = (232, 183, 96, 255)

SIZES = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 512,
}


def _png_chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def _make_png(size: int) -> bytes:
    # Draw a solid background with a centred filled square as a simple mark.
    pixels = bytearray()
    mark_lo = size // 4
    mark_hi = size - mark_lo
    for y in range(size):
        pixels.append(0)  # filter type: None
        for x in range(size):
            in_mark = mark_lo <= x < mark_hi and mark_lo <= y < mark_hi
            r, g, b, a = FG if in_mark else BG
            pixels.extend((r, g, b, a))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # RGBA, 8bpc
    idat = zlib.compress(bytes(pixels), level=9)

    signature = b"\x89PNG\r\n\x1a\n"
    return (
        signature
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", idat)
        + _png_chunk(b"IEND", b"")
    )


def main() -> None:
    ICON_DIR.mkdir(parents=True, exist_ok=True)
    for filename, size in SIZES.items():
        out = ICON_DIR / filename
        out.write_bytes(_make_png(size))
        print(f"wrote {out.relative_to(ICON_DIR.parent.parent.parent.parent)} ({size}x{size})")


if __name__ == "__main__":
    main()
