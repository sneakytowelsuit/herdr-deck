#!/usr/bin/env python3
"""Compose `herdr-deckd --dry-run` output into a single Stream Deck + preview image.

Used by the docs workflow so the picture in the documentation is regenerated from the real
renderer on every publish, rather than being a committed screenshot that quietly goes stale.

Pure standard library — no Pillow — so it runs anywhere without a dependency step.

    compose-preview.py <tiles-dir> <output.png>
"""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path

BACKGROUND = b"\x20\x22\x28\xff"
GAP = 8


def read_png(path: Path) -> tuple[int, int, list[bytes]]:
    """Decode an RGBA PNG into rows of bytes, undoing the per-line filters."""
    data = path.read_bytes()
    pos, width, height, idat = 8, 0, 0, b""
    while pos < len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        kind = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            width, height = struct.unpack(">II", chunk[:8])
        elif kind == b"IDAT":
            idat += chunk
        pos += 12 + length

    raw = zlib.decompress(idat)
    stride = width * 4
    rows: list[bytes] = []
    previous = bytearray(stride)
    offset = 0
    for _ in range(height):
        filter_type = raw[offset]
        offset += 1
        line = bytearray(raw[offset : offset + stride])
        offset += stride
        if filter_type == 1:  # Sub
            for x in range(4, stride):
                line[x] = (line[x] + line[x - 4]) & 255
        elif filter_type == 2:  # Up
            for x in range(stride):
                line[x] = (line[x] + previous[x]) & 255
        elif filter_type == 3:  # Average
            for x in range(stride):
                left = line[x - 4] if x >= 4 else 0
                line[x] = (line[x] + ((left + previous[x]) >> 1)) & 255
        elif filter_type == 4:  # Paeth
            for x in range(stride):
                a = line[x - 4] if x >= 4 else 0
                b = previous[x]
                c = previous[x - 4] if x >= 4 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                predictor = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + predictor) & 255
        rows.append(bytes(line))
        previous = line
    return width, height, rows


def chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    )


def write_png(path: Path, width: int, height: int, canvas: list[bytearray]) -> None:
    raw = b"".join(b"\x00" + bytes(row) for row in canvas)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def blit(canvas: list[bytearray], rows: list[bytes], width: int, x: int, y: int) -> None:
    for row_index, row in enumerate(rows):
        canvas[y + row_index][x * 4 : (x + width) * 4] = row


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    tiles, output = Path(sys.argv[1]), Path(sys.argv[2])

    keys = [read_png(tiles / f"key-{i:02d}.png") for i in range(8)]
    dials = [read_png(tiles / f"dial-{i}.png") for i in range(4)]

    key_w, key_h = keys[0][0], keys[0][1]
    dial_w, dial_h = dials[0][0], dials[0][1]

    strip_w = dial_w * len(dials)
    width = max(4 * key_w + 5 * GAP, strip_w + 2 * GAP)
    height = GAP + dial_h + GAP + 2 * key_h + 3 * GAP
    canvas = [bytearray(BACKGROUND * width) for _ in range(height)]

    # Touchstrip across the top, mirroring the physical layout.
    strip_x = (width - strip_w) // 2
    for index, (w, _, rows) in enumerate(dials):
        blit(canvas, rows, w, strip_x + index * dial_w, GAP)

    # Keys below, 4 across.
    keys_x = (width - (4 * key_w + 3 * GAP)) // 2
    keys_y = GAP + dial_h + GAP
    for index, (w, h, rows) in enumerate(keys):
        blit(
            canvas,
            rows,
            w,
            keys_x + (index % 4) * (key_w + GAP),
            keys_y + (index // 4) * (key_h + GAP),
        )

    write_png(output, width, height, canvas)
    print(f"wrote {output} ({width}x{height})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
