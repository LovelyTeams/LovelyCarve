#!/bin/bash
# Build the EROFS fixtures added in lamfold-erofs 0.1.3 (tests/erofs_chunks.rs).
# File contents are the formula regenerate.sh uses: file i of length n is the
# lines "lamfold-erofs fixture <i> line <k>\n" cut to n bytes.
#
#   chunks.erofs        mkfs.erofs --chunksize=8192: chunk-based files with a
#                       4-byte block map. sparse.bin (256 KiB) has data at 0
#                       and 65536 only; mkfs records its empty chunks as holes.
#   extslot_lzma.erofs  an -zlzma image rewritten with one superblock extension
#                       slot (sb_extslots = 1), so its LZMA config sits 16
#                       bytes later; the superblock checksum is recomputed and
#                       fsck.erofs extracts every file intact.
#   hole_full.erofs     a -Elegacy-compress (full index) lz4hc image with
#                       Linux 7.2's hole bit set on f0.txt's first pcluster.
#
# Needs erofs-utils 1.9 or later and python3. Set MKFS, DUMP and FSCK to
# commands that run mkfs.erofs, dump.erofs and fsck.erofs, and REGEN_TMP to a
# directory they can see, for example
#   REGEN_TMP=$HOME/.cache/work MKFS="podman exec f44 mkfs.erofs" \
#     DUMP="podman exec f44 dump.erofs" FSCK="podman exec f44 fsck.erofs" \
#     tests/fixtures/regenerate-chunks.sh
set -euo pipefail
cd "$(dirname "$0")"
FX="$PWD"
MKFS="${MKFS:-mkfs.erofs}"
DUMP="${DUMP:-dump.erofs}"
FSCK="${FSCK:-fsck.erofs}"
WORK=$(mktemp -d "${REGEN_TMP:-$FX}/.regen.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

python3 - "$WORK/src" <<'PY'
import os, sys
d = sys.argv[1]
os.makedirs(d)
for i, n in enumerate([20000, 9000, 100]):
    out, k = [], 0
    while sum(map(len, out)) < n:
        out.append(f"lamfold-erofs fixture {i} line {k}\n".encode()); k += 1
    open(os.path.join(d, f"f{i}.txt"), "wb").write(b"".join(out)[:n])
with open(os.path.join(d, "sparse.bin"), "wb") as f:
    f.write(b"head of a sparse file\n")
    f.seek(65536)
    f.write(b"middle\n")
    f.truncate(262144)
PY

$MKFS -T0 --chunksize=8192 "$WORK/chunks.erofs" "$WORK/src" > /dev/null
$MKFS -T0 -zlzma "$WORK/lzma.erofs" "$WORK/src" > /dev/null
$MKFS -T0 -zlz4hc -Elegacy-compress "$WORK/full.erofs" "$WORK/src" > /dev/null
NID=$($DUMP --path=/f0.txt "$WORK/full.erofs" | awk '/^NID:/ {print $2}')

python3 - "$WORK" "$NID" <<'PY'
import struct, sys
work, nid = sys.argv[1], int(sys.argv[2])
SB = 1024

def crc32c(data, crc=0xffffffff):
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82f63b78 & -(crc & 1))
    return crc

def sb_checksum(img):
    blksz = 1 << img[SB + 12]
    body = bytearray(img[SB:blksz])
    body[4:8] = b"\0\0\0\0"
    return crc32c(body)

# One extension slot: Shift the compression configs 16 bytes later.
img = bytearray(open(f"{work}/lzma.erofs", "rb").read())
assert struct.unpack_from("<I", img, SB + 4)[0] == sb_checksum(img), "checksum model"
assert img[SB + 13] == 0
algs = struct.unpack_from("<H", img, SB + 84)[0]
start = end = SB + 128
for bit in range(16):
    if algs & (1 << bit):
        end += 2 + struct.unpack_from("<H", img, end)[0]
cfg = bytes(img[start:end])
assert not any(img[end:end + 16]), "room after the configs"
img[start:start + 16] = bytes(16)
img[start + 16:end + 16] = cfg
img[SB + 13] = 1
struct.pack_into("<I", img, SB + 4, sb_checksum(img))
open(f"{work}/extslot_lzma.erofs", "wb").write(img)

# The hole bit on f0.txt's first full-index HEAD lcluster.
img = bytearray(open(f"{work}/full.erofs", "rb").read())
meta = struct.unpack_from("<I", img, SB + 40)[0] << img[SB + 12]
off = meta + nid * 32
fmt = struct.unpack_from("<H", img, off)[0]
assert (fmt >> 1) & 7 == 1, "full index"
isize = 64 if fmt & 1 else 32
xic = struct.unpack_from("<H", img, off + 2)[0]
xsize = 0 if xic == 0 else 12 + (xic - 1) * 4
idx0 = ((off + isize + xsize + 7) & ~7) + 16
advise = struct.unpack_from("<H", img, idx0)[0]
assert advise & 3 in (0, 1), "first lcluster is a HEAD"
struct.pack_into("<H", img, idx0, advise | 0x4000)
open(f"{work}/hole_full.erofs", "wb").write(img)
PY

# The rewritten image is valid: fsck.erofs extracts every file intact.
$FSCK --extract="$WORK/out" "$WORK/extslot_lzma.erofs" > /dev/null
diff -r "$WORK/src" "$WORK/out"
for f in chunks extslot_lzma hole_full; do
  cp "$WORK/$f.erofs" "$FX/$f.erofs"
  echo "  $f.erofs $(stat -c %s "$FX/$f.erofs") bytes"
done
