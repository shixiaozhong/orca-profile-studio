#!/usr/bin/env bash
# Regenerates crates/profile-studio/resources/windows/app.ico from the icon source PNG (macOS:
# sips + python3). Windows icons drop the macOS grid margin, so the tile is cropped tighter.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$root/target/icon-windows"
rm -rf "$work"
mkdir -p "$work"
sips --cropToHeightWidth 864 864 "$root/crates/profile-studio/resources/icon.png" --out "$work/crop.png" >/dev/null
for size in 16 24 32 48 64 256; do
    sips -z "$size" "$size" "$work/crop.png" --out "$work/$size.png" >/dev/null
done
python3 -I - "$work" "$root/crates/profile-studio/resources/windows/app.ico" <<'PY'
import pathlib, struct, sys

work, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
images = [(size, (work / f"{size}.png").read_bytes()) for size in (16, 24, 32, 48, 64, 256)]
offset = 6 + 16 * len(images)
entries = data = b""
for size, png in images:
    dim = 0 if size == 256 else size  # 0 means 256 in an ICONDIRENTRY
    entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset + len(data))
    data += png
out.write_bytes(struct.pack("<HHH", 0, 1, len(images)) + entries + data)
PY
rm -rf "$work"
echo "wrote $root/crates/profile-studio/resources/windows/app.ico"
