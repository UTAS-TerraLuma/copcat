"""Generates tests/data/fixture.copc.laz, a small synthetic COPC file.

Usage (needs `pdal` 2.6+ on PATH; Python standard library only):

    python tests/data/generate_fixture.py

The scene is deterministic (its own LCG, no `random`), asymmetric so a
flipped or mirrored render is caught, and has known bounds because the
ground's corners and the tallest tree's tip are placed exactly:

    X  500000.00 .. 500100.00   (100 m, west to east)
    Y 5400000.00 .. 5400060.00  (60 m, south to north)
    Z     100.00 ..    125.00

Classes: 2 ground, 3 low vegetation, 5 high vegetation, 6 building.
"""

import json
import math
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUTPUT = os.path.join(HERE, "fixture.copc.laz")

X0, Y0, Z0 = 500000.0, 5400000.0, 100.0
WIDTH, DEPTH = 100.0, 60.0


class Lcg:
    """Numerical Recipes' 32-bit LCG: the same numbers on every platform."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def unit(self):
        self.state = (1664525 * self.state + 1013904223) & 0xFFFFFFFF
        return self.state / 2**32


def ground_z(x, y):
    """Rises 4 m from west to east, plus 1 m from south to north."""
    return Z0 + 4.0 * x / WIDTH + 1.0 * y / DEPTH


def scene():
    rng = Lcg(20261118)
    points = []  # (x, y, z relative to X0/Y0, absolute z, class, r, g, b)

    def add(x, y, z, cls, rgb):
        points.append((x, y, z, cls, *rgb))

    # Ground: a jittered 1.25 m grid, browns getting lighter to the east,
    # with the four corners exact so the bounds are known.
    step = 1.25
    nx, ny = int(WIDTH / step), int(DEPTH / step)
    for j in range(ny + 1):
        for i in range(nx + 1):
            x, y = i * step, j * step
            if 0 < i < nx and 0 < j < ny:
                x += (rng.unit() - 0.5) * step * 0.6
                y += (rng.unit() - 0.5) * step * 0.6
            shade = int(90 + 80 * x / WIDTH)
            add(x, y, ground_z(x, y), 2, (shade, int(shade * 0.8), int(shade * 0.55)))

    # A building in the north-west: an 18 x 12 m flat roof 9 m up, plus walls.
    bx0, by0, bw, bd, bh = 12.0, 38.0, 18.0, 12.0, 9.0
    base = ground_z(bx0, by0)
    for j in range(int(bd / 0.75) + 1):
        for i in range(int(bw / 0.75) + 1):
            add(bx0 + i * 0.75, by0 + j * 0.75, base + bh, 6, (200, 60, 50))
    for k in range(1, int(bh / 1.0)):
        z = base + k
        for i in range(int(bw / 1.0) + 1):
            add(bx0 + i, by0, z, 6, (170, 160, 150))
            add(bx0 + i, by0 + bd, z, 6, (170, 160, 150))
        for j in range(1, int(bd / 1.0)):
            add(bx0, by0 + j, z, 6, (170, 160, 150))
            add(bx0 + bw, by0 + j, z, 6, (170, 160, 150))

    # Trees in the south-east: cones of points. The first is the tallest and
    # its tip sets the cloud's top at exactly Z0 + 25 m.
    trees = [(80.0, 15.0, 0.0), (65.0, 22.0, 15.0), (88.0, 40.0, 13.0), (55.0, 10.0, 12.0)]
    tip_x, tip_y = trees[0][0], trees[0][1]
    trees[0] = (tip_x, tip_y, Z0 + 25.0 - ground_z(tip_x, tip_y))
    for tx, ty, height in trees:
        ground = ground_z(tx, ty)
        add(tx, ty, ground + height, 5, (30, 120, 40))
        for _ in range(int(height * 40)):
            h = 2.0 + rng.unit() * (height - 2.0)
            radius = (height - h) * 0.3 * rng.unit()
            angle = rng.unit() * 6.283185307179586
            x = tx + radius * math.cos(angle)
            y = ty + radius * math.sin(angle)
            add(x, y, ground + h, 5, (30, int(100 + 80 * h / height), 40))

    # Low vegetation: a hedge along the south edge of the building.
    for _ in range(400):
        x = bx0 + rng.unit() * bw
        y = by0 - 2.0 - rng.unit() * 1.0
        add(x, y, ground_z(x, y) + 0.3 + rng.unit() * 0.9, 3, (90, 170, 60))

    return points


def main():
    points = scene()
    with tempfile.TemporaryDirectory() as tmp:
        csv = os.path.join(tmp, "points.csv")
        with open(csv, "w", newline="\n") as f:
            f.write("X,Y,Z,Classification,Red,Green,Blue\n")
            for x, y, z, cls, r, g, b in points:
                # 8-bit colours scaled to LAS's 16 bits.
                f.write(
                    f"{X0 + x:.2f},{Y0 + y:.2f},{z:.2f},{cls},{r * 257},{g * 257},{b * 257}\n"
                )
        pipeline = [
            {"type": "readers.text", "filename": csv},
            {
                "type": "writers.copc",
                "filename": OUTPUT,
                "scale_x": 0.01,
                "scale_y": 0.01,
                "scale_z": 0.01,
                "offset_x": X0,
                "offset_y": Y0,
                "offset_z": Z0,
                # Fixed header fields. The octree layout still varies run to
                # run, so regenerating changes the bytes (see README.md).
                "software_id": "copcat generate_fixture.py",
                "creation_doy": 1,
                "creation_year": 2026,
            },
        ]
        subprocess.run(
            ["pdal", "pipeline", "--stdin"],
            input=json.dumps(pipeline),
            text=True,
            check=True,
        )
    classes = {}
    for p in points:
        classes[p[3]] = classes.get(p[3], 0) + 1
    print(f"Wrote {OUTPUT}: {len(points)} points, classes {dict(sorted(classes.items()))}")
    print(f"{os.path.getsize(OUTPUT)} bytes")


if __name__ == "__main__":
    sys.exit(main())
