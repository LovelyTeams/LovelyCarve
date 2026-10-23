#!/bin/bash
# Build the EROFS fixtures added in lamfold-erofs 0.1.2 (the older comp_* and
# erofs.img fixtures came from mkfs.erofs 1.8.6 by hand; see each test's
# header). Every file's contents are a formula the tests recompute, so no
# payload is committed alongside:
#   file i, length n: the lines "lamfold-erofs fixture <i> line <k>\n" for
#   k = 0, 1, 2, ..., cut to n bytes.
# lz4_zeropad.erofs is the exception: its one file is seeded pseudo-random
# executable-like data, checked by SHA-256.
# Needs erofs-utils 1.9 or later (custom LZMA props, 48-bit, metabox, --zD)
# and python3. Set MKFS to a command that runs one, and REGEN_TMP to a
# directory it can see, for example
#   REGEN_TMP=/srv/shared MKFS="podman exec f44 mkfs.erofs" tests/fixtures/regenerate.sh
# with /srv/shared bound into the container at the same path.
set -euo pipefail
cd "$(dirname "$0")"
FX="$PWD"
MKFS="${MKFS:-mkfs.erofs}"
WORK=$(mktemp -d "${REGEN_TMP:-$FX}/.regen.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# tree DIR COUNT BASE [XATTR]: COUNT files f<i>.txt of BASE + 1237*i bytes,
# each with a 39-byte user.k xattr when XATTR is set. The one-letter name and
# 39-byte value make a 56-byte inline xattr area, which puts a compressed
# file's index base on a 32-byte boundary.
tree() {
  python3 - "$@" <<'PY'
import os, sys
d, count, base = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
xattr = len(sys.argv) > 4
os.makedirs(d, exist_ok=True)
for i in range(count):
    n = base + 1237 * i
    out, k = [], 0
    while sum(map(len, out)) < n:
        out.append(f"lamfold-erofs fixture {i} line {k}\n".encode()); k += 1
    p = os.path.join(d, f"f{i}.txt")
    open(p, "wb").write(b"".join(out)[:n])
    if xattr:
        os.setxattr(p, "user.k", b"%02d" % i + b"x" * 37)
PY
}

build() {  # build NAME TREE OPTS...
  local name=$1 src=$2; shift 2
  $MKFS "$@" "$WORK/$name.erofs" "$src" > /dev/null
  cp "$WORK/$name.erofs" "$FX/$name.erofs"
  echo "  $name.erofs $(stat -c %s "$FX/$name.erofs") bytes"
}

# One 153059-byte file of executable-like data whose lz4hc pclusters include
# one with a single byte of zero padding before its stream: lamfold-erofs
# 0.1.1 decoded that pcluster from offset 0, which also "succeeds", and
# returned wrong bytes with no error.
lz4pad() {
  mkdir -p "$1"
  python3 - "$1/lz4pad.bin" <<'PY'
import random, sys
r = random.Random(1 * 1000 + 77)
b = bytearray()
n = r.randint(600_000, 1_400_000)
while len(b) < n:
    k = r.random()
    if k < 0.25:
        b += r.randbytes(r.randint(1, 40))
    elif k < 0.50:
        op = r.choice([b'\x48\x89', b'\x48\x8b', b'\xe8', b'\x0f\x1f\x44\x00\x00', b'\x41\x57', b'\xc3'])
        b += op + r.randint(0, 1 << 32).to_bytes(4, 'little')[: r.randint(0, 4)]
    elif k < 0.62:
        b += bytes(r.randint(1, 16))
    elif k < 0.80 and len(b) > 256:
        s = r.randint(max(0, len(b) - 60000), len(b) - 16)
        b += b[s:s + r.randint(4, 64)]
    else:
        b += bytes(r.choice(b'abcdefghijklmnopqrstuvwxyz_.0123456789') for _ in range(r.randint(3, 24))) + b'\0'
open(sys.argv[1], 'wb').write(bytes(b[:153059]))
PY
}

tree "$WORK/xattr" 4 40000 xattr
tree "$WORK/plain" 3 30000
lz4pad "$WORK/lz4pad"
echo "regenerating with $($MKFS --version 2>&1 | head -n 1):"
build lz4_zeropad "$WORK/lz4pad" -zlz4hc,9
sha256sum "$WORK/lz4pad/lz4pad.bin" | cut -d' ' -f1 > "$FX/lz4_zeropad.sha256"
build xattr_zstd "$WORK/xattr" -zzstd -C65536
build xattr_lzma "$WORK/xattr" -zlzma -C65536
build lzma_props "$WORK/plain" -zlzma,lc=4,lp=0,pb=0
build blk1024_zstd "$WORK/plain" -zzstd -b1024
build blk2048_lz4hc "$WORK/plain" -zlz4hc -b2048
build refuse_fragments "$WORK/plain" -zlz4hc -Eall-fragments
build refuse_dircompress "$WORK/plain" -zlz4hc --zD
build refuse_48bit "$WORK/plain" -E48bit
build refuse_metabox "$WORK/plain" -zzstd -m4096
