#!/usr/bin/env python3
"""Copy the app icon's embedded PNG into site/assets. Does not redraw or resize."""
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
icns = (root / "native/packaging/icon.icns").read_bytes()
if icns[:4] != b"icns":
    sys.exit("native/packaging/icon.icns is not an icns file")
total = int.from_bytes(icns[4:8], "big")
off = 8
png = None
while off + 8 <= len(icns) and off < total:
    typ = icns[off:off + 4]
    ln = int.from_bytes(icns[off + 4:off + 8], "big")
    payload = icns[off + 8:off + ln]
    if typ == b"ic09" and payload.startswith(b"\x89PNG\r\n\x1a\n"):
        png = payload
    off += ln
if png is None:
    sys.exit("icon.icns has no ic09 PNG; refusing to invent a favicon")
out = root / "site/assets/favicon.png"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_bytes(png)
print(f"wrote {out} ({len(png)} bytes, ic09 PNG unchanged)")
