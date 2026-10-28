#!/usr/bin/env python3
"""mac-icon.py OUT.png -- orrery's icon, 1024 pixels square, RGBA.

An orrery seen from a little above: a brass sun, three tilted orbits and a
body on each, on the window's paper (draw.rs's light theme: base, line,
accent, ink).  Drawn by distance, so every edge is anti-aliased without
supersampling, and written as a PNG by hand -- zlib is the only library.
tools/mac-app.sh turns it into the .icns.
"""
import math
import struct
import sys
import zlib

N = 1024
BASE = (0xFC, 0xE2, 0xAB)
TILE = (0xFF, 0xF2, 0xD4)
LINE = (0xC4, 0xA8, 0x70)
ACCENT = (0x87, 0x70, 0x4F)
INK = (0x12, 0x12, 0x12)
BRASS = (0xB0, 0x6A, 0x24)
SUN = (0xF0, 0xA0, 0x3A)


def cover(d):
    """Coverage of a pixel whose centre is d pixels outside an edge."""
    return min(1.0, max(0.0, 0.5 - d))


def rounded_square(x, y, inset, r):
    # Distance to a square inset from the canvas edge, corners of radius r.
    c = N / 2
    h = c - inset - r
    qx, qy = abs(x - c) - h, abs(y - c) - h
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def ellipse_ring(x, y, cx, cy, a, b, tilt, width):
    # Distance to a tilted ellipse's outline, by the first-order estimate
    # |f| / |grad f| -- good to a pixel at these sizes.
    ct, st = math.cos(tilt), math.sin(tilt)
    dx, dy = x - cx, y - cy
    u, v = dx * ct + dy * st, -dx * st + dy * ct
    f = (u / a) ** 2 + (v / b) ** 2 - 1
    gu, gv = 2 * u / a / a, 2 * v / b / b
    g = math.hypot(gu, gv) or 1e-9
    return abs(f) / g - width / 2


def on_ellipse(cx, cy, a, b, tilt, t):
    ct, st = math.cos(tilt), math.sin(tilt)
    u, v = a * math.cos(t), b * math.sin(t)
    return cx + u * ct - v * st, cy + u * st + v * ct


def render():
    c = N / 2
    orbits = [  # (a, b, tilt, width, body angle, body radius, colour)
        (180, 70, -0.35, 7, 0.6, 26, ACCENT),
        (300, 120, -0.35, 7, 3.7, 34, INK),
        (400, 165, -0.35, 6, 5.4, 22, BRASS),
    ]
    bodies = [on_ellipse(c, c, a, b, tl, t) + (r, col) for a, b, tl, _, t, r, col in orbits]
    rows = []
    for j in range(N):
        y = j + 0.5
        row = bytearray()
        for i in range(N):
            x = i + 0.5
            # The tile, with a line of LINE at its edge; transparent outside.
            d = rounded_square(x, y, 100, 185)
            alpha = cover(d)
            if alpha == 0:
                row += b"\0\0\0\0"
                continue
            col = list(BASE)
            # A paler centre, like light on paper.
            t = min(1.0, math.hypot(x - c, y - c * 0.85) / 520)
            col = [TILE[k] + (BASE[k] - TILE[k]) * t for k in range(3)]

            def paint(rgb, a):
                for k in range(3):
                    col[k] = col[k] + (rgb[k] - col[k]) * a

            paint(LINE, cover(abs(d + 6) - 6))
            for a, b, tl, w, *_ in orbits:
                paint(LINE if w < 7 else ACCENT, 0.85 * cover(ellipse_ring(x, y, c, c, a, b, tl, w)))
            # The sun, with a soft corona.
            ds = math.hypot(x - c, y - c)
            paint(SUN, 0.25 * max(0.0, 1 - (ds - 70) / 60) if ds > 70 else 0)
            paint(SUN, cover(ds - 72))
            paint(BRASS, cover(abs(ds - 72) - 4))
            for bx, by, r, bc in bodies:
                db = math.hypot(x - bx, y - by)
                paint(bc, cover(db - r))
            row += bytes(int(round(v)) for v in col) + bytes([int(round(alpha * 255))])
        rows.append(bytes(row))
    return rows


def png(rows, path):
    raw = b"".join(b"\0" + r for r in rows)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: mac-icon.py OUT.png")
    png(render(), sys.argv[1])
