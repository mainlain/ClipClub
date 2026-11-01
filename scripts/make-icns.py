#!/usr/bin/env python3
"""Wrap native-rendered PNG representations in the macOS ICNS container."""
from pathlib import Path
import struct
import sys

if len(sys.argv) != 3:
    raise SystemExit("Usage: make-icns.py INPUT.iconset OUTPUT.icns")
source = Path(sys.argv[1])
representations = [
    ("icp4", "icon_16x16.png", 16),
    ("ic11", "icon_16x16@2x.png", 32),
    ("icp5", "icon_32x32.png", 32),
    ("ic12", "icon_32x32@2x.png", 64),
    ("ic07", "icon_128x128.png", 128),
    ("ic13", "icon_128x128@2x.png", 256),
    ("ic08", "icon_256x256.png", 256),
    ("ic14", "icon_256x256@2x.png", 512),
    ("ic09", "icon_512x512.png", 512),
    ("ic10", "icon_512x512@2x.png", 1024),
]
chunks = []
for kind, name, size in representations:
    image = (source / name).read_bytes()
    if image[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", image[16:24]) != (size, size):
        raise SystemExit("Invalid icon PNG: " + name)
    chunks.append(kind.encode("ascii") + struct.pack(">I", len(image) + 8) + image)
payload = b"".join(chunks)
Path(sys.argv[2]).write_bytes(b"icns" + struct.pack(">I", len(payload) + 8) + payload)
