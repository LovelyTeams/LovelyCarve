#!/usr/bin/env bash
# Regenerate fixtures/udf201.img for tests/udf201.rs.
#
# mkudffs makes a UDF 2.01 volume (its default revision) and the kernel's UDF
# driver writes the files through a loop mount, so the File Entries are the
# kernel's own: Extended File Entries (tag 266), which UDF 2.00 and later use.
#
#   /readme.txt     "udf 2.01 root file\n"
#   /sub/inner.txt  "udf 2.01 nested file\n"
#   /big.bin        70000 bytes, byte i = (i * 11 + i / 257) & 0xff
#
# Requires: mkudffs (udftools), the udf kernel module, sudo, python3.
# Usage: ./regenerate-udf201.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d /var/tmp/lamfold-udf.XXXXXX)"
MNT="$WORK/mnt"
mkdir "$MNT"
cleanup() {
  if findmnt --mountpoint "$MNT" >/dev/null; then sudo umount "$MNT"; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

truncate -s 4M "$WORK/udf201.img"
/usr/sbin/mkudffs --media-type=hd --udfrev=0x0201 --blocksize=2048 \
  --label=lamfold201 --uuid=0123456789abcdef "$WORK/udf201.img" >/dev/null
sudo mount -o loop,rw -t udf "$WORK/udf201.img" "$MNT"
findmnt --mountpoint "$MNT" -o FSTYPE -n | grep -qx udf
printf 'udf 2.01 root file\n' | sudo tee "$MNT/readme.txt" >/dev/null
sudo mkdir "$MNT/sub"
printf 'udf 2.01 nested file\n' | sudo tee "$MNT/sub/inner.txt" >/dev/null
python3 -c "import sys; sys.stdout.buffer.write(bytes((i * 11 + i // 257) & 0xff for i in range(70000)))" \
  | sudo tee "$MNT/big.bin" >/dev/null
sync
sudo umount "$MNT"
cp "$WORK/udf201.img" "$HERE/udf201.img"
ls -l "$HERE/udf201.img"
