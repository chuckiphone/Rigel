#!/usr/bin/env python3
"""Generate the Tauri icon set from the single-star mark.

The mark matches public/rigel.svg: an amber four-point star with a faint glow on a
near-black rounded tile. It is drawn at 1024 px and downsampled, so every size in the
.ico and .icns is antialiased. The PNGs are what Linux bundles consume; the .icns is
the macOS bundle icon and is written as a standard PNG-chunk ICNS container.

Requires Pillow (pip install pillow).
"""

from __future__ import annotations

import struct
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageDraw

BIG = 1024
BG = (0x0B, 0x0C, 0x0E, 255)
AMBER = (0xE9, 0xA2, 0x3B, 255)
GLOW = (0xE9, 0xA2, 0x3B, 36)  # 0.14 alpha, as in the SVG
SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
PNG_ICONS = [
    (32, "32x32.png"),
    (128, "128x128.png"),
    (256, "128x128@2x.png"),
    (1024, "icon.png"),
]
# ICNS element types and pixel sizes. PNG payloads are valid for every modern type;
# keeping the full ladder means Finder, Dock, Gatekeeper and Spotlight all get a native size.
ICNS_SIZES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
]

# Coordinates are in the 24-unit SVG space, from public/rigel.svg.
STAR_CURVES = [
    ((12, 4), (12.9, 11.1), (20, 12)),
    ((20, 12), (12.9, 12.9), (12, 20)),
    ((12, 20), (11.1, 12.9), (4, 12)),
    ((4, 12), (11.1, 11.1), (12, 4)),
]


def to_px(x: float, y: float, scale: float) -> tuple[float, float]:
    return x * scale, y * scale


def quad_points(p0, c, p1, scale: float, steps: int = 40):
    out = []
    for i in range(steps):
        t = i / steps
        x = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * c[0] + t**2 * p1[0]
        y = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * c[1] + t**2 * p1[1]
        out.append(to_px(x, y, scale))
    return out


def render(size: int = BIG) -> Image.Image:
    scale = size / 24.0
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    draw.rounded_rectangle((0, 0, size - 1, size - 1), radius=5 * scale, fill=BG)

    r = 5.5 * scale
    cx = cy = 12 * scale
    draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=GLOW)

    points: list[tuple[float, float]] = []
    for p0, c, p1 in STAR_CURVES:
        points += quad_points(p0, c, p1, scale)
    draw.polygon(points, fill=AMBER)
    return img


def write_icns(img: Image.Image, out: Path) -> None:
    """Write a PNG-chunk ICNS file without relying on macOS-only tooling."""
    chunks: list[bytes] = []
    for kind, size in ICNS_SIZES:
        payload = BytesIO()
        img.resize((size, size), Image.Resampling.LANCZOS).save(payload, format="PNG")
        data = payload.getvalue()
        chunks.append(kind + struct.pack(">I", len(data) + 8) + data)
    body = b"".join(chunks)
    out.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def main() -> None:
    icons = Path(__file__).resolve().parent.parent / "crates/rigel-app/icons"
    icons.mkdir(parents=True, exist_ok=True)
    mark = render()

    out = icons / "icon.ico"
    mark.save(out, format="ICO", sizes=SIZES)
    print(f"wrote {out} ({out.stat().st_size} bytes)")

    out = icons / "icon.icns"
    write_icns(mark, out)
    print(f"wrote {out} ({out.stat().st_size} bytes)")

    for size, name in PNG_ICONS:
        out = icons / name
        mark.resize((size, size), Image.Resampling.LANCZOS).save(out, format="PNG")
        print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
