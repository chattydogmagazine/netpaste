#!/usr/bin/env python3
"""Pack PNGs into a Windows .ico, with no image library needed.

An .ico is just a small directory header followed by the files themselves;
Windows Vista and later read PNG entries directly, so the PNGs go in as-is.

    make-ico.py out.ico 16.png 32.png 48.png ...
"""
import struct
import sys


def png_size(data: bytes) -> tuple[int, int]:
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("not a PNG")
    return struct.unpack(">II", data[16:24])


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2

    out, sources = sys.argv[1], sys.argv[2:]
    images = []
    for path in sources:
        with open(path, "rb") as f:
            data = f.read()
        w, h = png_size(data)
        if not (1 <= w <= 256 and 1 <= h <= 256):
            raise SystemExit(f"{path}: {w}x{h} is outside the .ico range of 1-256")
        images.append((w, h, data))

    images.sort(key=lambda i: i[0])
    header = struct.pack("<HHH", 0, 1, len(images))  # reserved, type 1 = icon, count
    offset = len(header) + 16 * len(images)

    entries, blobs = b"", b""
    for w, h, data in images:
        entries += struct.pack(
            "<BBBBHHII",
            w % 256,  # 256 is stored as 0
            h % 256,
            0,  # palette size: 0 for truecolour
            0,  # reserved
            1,  # colour planes
            32,  # bits per pixel
            len(data),
            offset,
        )
        blobs += data
        offset += len(data)

    with open(out, "wb") as f:
        f.write(header + entries + blobs)

    sizes = ", ".join(f"{w}x{h}" for w, h, _ in images)
    print(f"{out}: {len(images)} images ({sizes})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
