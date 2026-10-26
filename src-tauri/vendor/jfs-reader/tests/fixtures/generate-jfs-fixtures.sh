#!/usr/bin/env bash
# Generate the JFS2 fixture for docs/need_fixtures.md.
#
# Runs inside WSL Ubuntu (or any Linux box). Setup the once-only host bits
# before the first run:
#   sudo apt install -y jfsutils libguestfs-tools \
#                       linux-image-virtual \
#                       linux-modules-extra-$(uname -r)
#   sudo dpkg-statoverride --update --add root root 0644 /boot/vmlinuz-*
#
# The WSL kernel may not have the jfs module loaded; we mount via
# libguestfs's QEMU appliance (which uses the host's /boot/vmlinuz +
# /lib/modules/*).
#
# Why JFS2 only:
#   AIX JFS1 (the older sibling) uses a different on-disk format-id and is
#   not relevant on Linux. Linux JFS2 accepts superblock versions 1 and 2;
#   mkfs.jfs commonly writes version 1 (this is not the AIX JFS1 format).
#
# Output:
#   tests/fixtures/test_jfs.img.zst
#
# Layout per docs/need_fixtures.md (mirrors the ext / reiserfs / ufs
# fixtures):
#   /hello.txt          -> "Hello, JFS!"
#   /subdir/nested.txt  -> "nested file"
#   /link.txt           -> symlink to hello.txt
#   /large.bin          -> 24 KiB deterministic data (exercises xtree leaf)
#   /tiny.txt           -> 10 bytes
#   /bigdir/file_<NNN>  -> 200 numbered tiny files (exercises external
#                          dtree: blows past the inline-dtroot's 8-slot
#                          cap so JFS allocates an off-disk dtpage). The
#                          read-side multi-page dtree walker (parked in
#                          OPEN-WORK §8) verifies against this directory
#                          once it lands.

set -euo pipefail

# LovelyCarve adaptation: resolve the portable output before entering WORK.
FIXTURES_DIR=${JFS_FIXTURES_DIR:-"$(cd -- "$(dirname -- "$0")" && pwd)"}
FIXTURES_DIR=$(cd -- "$FIXTURES_DIR" && pwd)

WORK=$(mktemp -d /tmp/jfs_fixtures.XXXXXX)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

# Allow guestfish to use the kernel without permission complaints.
export LIBGUESTFS_BACKEND=direct

build_one() {
    local raw=$1

    echo "=== building jfs2 ==="

    # 16 MiB is the JFS minimum (any smaller and mkfs.jfs refuses).
    dd if=/dev/zero of="$raw" bs=1M count=16 status=none
    # `-q` quiet, `-f` force (overwrite existing FS), `-L` label.
    mkfs.jfs -q -L jfs_test "$raw"

    # Populate via guestfish (its appliance kernel has jfs).
    guestfish --rw -a "$raw" <<'EOF'
run
mount /dev/sda /
write /hello.txt "Hello, JFS!"
mkdir /subdir
write /subdir/nested.txt "nested file"
ln-s hello.txt /link.txt
write /tiny.txt "tiny bytes"
mkdir /bigdir
EOF

    # Populate /bigdir with 200 tiny files. This blows past the inline
    # dtroot's 8-slot cap (an inline dtree maxes out around ~8 short-name
    # entries), forcing JFS to convert /bigdir to an external dtree with
    # off-disk dtpages. The read-side multi-page dtree walker (parked in
    # OPEN-WORK §8) regression-tests against this directory once it
    # lands; today's parse_inline_dtree refuses with a clear error.
    {
        echo "run"
        echo "mount /dev/sda /"
        for i in $(seq -w 1 200); do
            echo "write /bigdir/file_${i} \"line ${i}\""
        done
    } | guestfish --rw -a "$raw"

    # Write a 24 KiB "large" file separately (binary content -> heredoc
    # would mangle bytes). guestfish 'upload' takes a host path.
    python3 -c "
import os
data = bytearray()
for i in range(24 * 1024):
    data.append((i * 37 + 11) & 0xFF)
open('$WORK/large.bin', 'wb').write(data)
"
    guestfish --rw -a "$raw" <<EOF
run
mount /dev/sda /
upload $WORK/large.bin /large.bin
EOF

    # Sanity check: list and stat the entries we just wrote.
    guestfish --ro -a "$raw" <<'EOF'
run
mount /dev/sda /
ll /
ls /subdir
EOF

    # Quick byte-level probe so the fixture-driven tests can pin
    # absolute offsets without guessing.
    echo "--- aggregate superblock @ byte 32768 ---"
    xxd -s 32768 -l 64 "$raw" | head -4

    # Compress and copy to the project fixtures directory.
    zstd -q -f -19 "$raw" -o "${raw}.zst"
    cp "${raw}.zst" "$FIXTURES_DIR/"
    echo "wrote $FIXTURES_DIR/${raw}.zst ($(stat -c %s ${raw}.zst) bytes)"
}

build_one test_jfs.img

ls -la "$FIXTURES_DIR"/test_jfs.img.zst
