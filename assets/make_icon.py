#!/usr/bin/env python3
"""Draws the VeloEdit icon (a "V" and a text caret on a dark rounded square)
with plain Python - no imaging library needed - and writes PNGs next to it.

    python3 assets/make_icon.py        # -> assets/icon-256.png, assets/icon-128.png
"""
import math, os, struct, zlib

SS = 4  # supersampling per axis


def rounded_box(px, py, x0, y0, x1, y1, r):
    """Signed distance to a rounded rectangle (negative inside)."""
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    hx, hy = (x1 - x0) / 2 - r, (y1 - y0) / 2 - r
    dx, dy = abs(px - cx) - hx, abs(py - cy) - hy
    return math.hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - r


def segment(px, py, ax, ay, bx, by):
    """Distance to the segment a-b."""
    vx, vy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)))
    return math.hypot(px - (ax + t * vx), py - (ay + t * vy))


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def sample(x, y):
    """RGBA (0..1, not premultiplied) of the icon at a point of the 256x256 canvas."""
    out = (0.0, 0.0, 0.0, 0.0)

    def over(rgb, a):
        nonlocal out
        if a <= 0:
            return
        oa = a + out[3] * (1 - a)
        out = tuple((rgb[i] * a + out[i] * out[3] * (1 - a)) / oa for i in range(3)) + (oa,)

    # Background plate with a faint rim.
    d = rounded_box(x, y, 10, 10, 246, 246, 56)
    if d <= 0:
        over(mix((0.17, 0.20, 0.27), (0.08, 0.09, 0.12), y / 256), 1.0)
        if d > -3.5:
            over((0.42, 0.50, 0.66), 0.35)
    # The V: two thick strokes with round ends.
    dv = min(segment(x, y, 64, 76, 128, 184, ), segment(x, y, 192, 76, 128, 184))
    if dv <= 21:
        over(mix((0.55, 0.80, 1.0), (0.24, 0.55, 0.88), (y - 55) / 150), 1.0)
    # A text caret, so it reads as an editor.
    dc = rounded_box(x, y, 204, 134, 217, 200, 6)
    if dc <= 0:
        over((0.90, 0.75, 0.48), 1.0)
    return out


def render(size):
    n = 256
    raw = bytearray()
    for py in range(size):
        raw.append(0)  # PNG filter: none
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]  # premultiplied sum
            for sy in range(SS):
                for sx in range(SS):
                    x = (px + (sx + 0.5) / SS) * n / size
                    y = (py + (sy + 0.5) / SS) * n / size
                    r, g, b, a = sample(x, y)
                    acc[0] += r * a; acc[1] += g * a; acc[2] += b * a; acc[3] += a
            a = acc[3] / (SS * SS)
            if a > 0:
                rgb = [acc[i] / acc[3] for i in range(3)]
            else:
                rgb = [0, 0, 0]
            raw += bytes([round(c * 255) for c in rgb] + [round(a * 255)])
    return bytes(raw)


def png(size, raw):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    for size in (256, 128):
        path = os.path.join(here, f"icon-{size}.png")
        with open(path, "wb") as f:
            f.write(png(size, render(size)))
        print("wrote", path)
