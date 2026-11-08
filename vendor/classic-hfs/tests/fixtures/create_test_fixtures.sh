#!/bin/bash
set -e

FIXTURES="/home/dani/repos/rusty-backup/tests/fixtures"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo -n "nested file" > "$TMPDIR/nested.txt"

# ========== FAT12 (1MB) ==========
echo "=== Creating FAT12 ==="
dd if=/dev/zero of="$TMPDIR/fat12.img" bs=1M count=1 2>/dev/null
mkfs.fat -F 12 -n "TEST_FAT12" "$TMPDIR/fat12.img" >/dev/null
echo -n "Hello, fat12!" > "$TMPDIR/hello.txt"
mcopy -i "$TMPDIR/fat12.img" "$TMPDIR/hello.txt" "::hello.txt"
mmd -i "$TMPDIR/fat12.img" "::subdir"
mcopy -i "$TMPDIR/fat12.img" "$TMPDIR/nested.txt" "::subdir/nested.txt"
zstd -19 -f "$TMPDIR/fat12.img" -o "$FIXTURES/test_fat12.img.zst"
echo "FAT12 done: $(stat -c%s "$FIXTURES/test_fat12.img.zst") bytes"

# ========== FAT16 (16MB) ==========
echo "=== Creating FAT16 ==="
dd if=/dev/zero of="$TMPDIR/fat16.img" bs=1M count=16 2>/dev/null
mkfs.fat -F 16 -n "TEST_FAT16" "$TMPDIR/fat16.img" >/dev/null
echo -n "Hello, fat16!" > "$TMPDIR/hello.txt"
mcopy -i "$TMPDIR/fat16.img" "$TMPDIR/hello.txt" "::hello.txt"
mmd -i "$TMPDIR/fat16.img" "::subdir"
mcopy -i "$TMPDIR/fat16.img" "$TMPDIR/nested.txt" "::subdir/nested.txt"
zstd -19 -f "$TMPDIR/fat16.img" -o "$FIXTURES/test_fat16.img.zst"
echo "FAT16 done: $(stat -c%s "$FIXTURES/test_fat16.img.zst") bytes"

# ========== FAT32 (16MB) ==========
echo "=== Creating FAT32 ==="
dd if=/dev/zero of="$TMPDIR/fat32.img" bs=1M count=16 2>/dev/null
mkfs.fat -F 32 -n "TEST_FAT32" "$TMPDIR/fat32.img" >/dev/null
echo -n "Hello, fat32!" > "$TMPDIR/hello.txt"
mcopy -i "$TMPDIR/fat32.img" "$TMPDIR/hello.txt" "::hello.txt"
mmd -i "$TMPDIR/fat32.img" "::subdir"
mcopy -i "$TMPDIR/fat32.img" "$TMPDIR/nested.txt" "::subdir/nested.txt"
zstd -19 -f "$TMPDIR/fat32.img" -o "$FIXTURES/test_fat32.img.zst"
echo "FAT32 done: $(stat -c%s "$FIXTURES/test_fat32.img.zst") bytes"

# ========== NTFS (2MB) ==========
echo "=== Creating NTFS ==="
dd if=/dev/zero of="$TMPDIR/ntfs.img" bs=1M count=2 2>/dev/null
mkfs.ntfs --force -f -L "test_ntfs" -s 512 "$TMPDIR/ntfs.img" >/dev/null 2>&1
mkdir -p "$TMPDIR/mnt_ntfs"
ntfs-3g "$TMPDIR/ntfs.img" "$TMPDIR/mnt_ntfs" -o rw
echo -n "Hello, ntfs!" > "$TMPDIR/mnt_ntfs/hello.txt"
mkdir -p "$TMPDIR/mnt_ntfs/subdir"
echo -n "nested file" > "$TMPDIR/mnt_ntfs/subdir/nested.txt"
fusermount -u "$TMPDIR/mnt_ntfs" || umount "$TMPDIR/mnt_ntfs"
zstd -19 -f "$TMPDIR/ntfs.img" -o "$FIXTURES/test_ntfs.img.zst"
echo "NTFS done: $(stat -c%s "$FIXTURES/test_ntfs.img.zst") bytes"

# ========== exFAT (1MB) ==========
echo "=== Creating exFAT ==="
dd if=/dev/zero of="$TMPDIR/exfat.img" bs=1M count=1 2>/dev/null
mkfs.exfat -L "test_exfat" "$TMPDIR/exfat.img" >/dev/null 2>&1
mkdir -p "$TMPDIR/mnt_exfat"
# Try kernel mount first, fall back to FUSE
if mount -o loop "$TMPDIR/exfat.img" "$TMPDIR/mnt_exfat" 2>/dev/null; then
    echo "  (using kernel exfat)"
elif mount.exfat-fuse "$TMPDIR/exfat.img" "$TMPDIR/mnt_exfat" 2>/dev/null; then
    echo "  (using fuse exfat)"
elif mount.exfat "$TMPDIR/exfat.img" "$TMPDIR/mnt_exfat" 2>/dev/null; then
    echo "  (using mount.exfat)"
else
    echo "ERROR: Cannot mount exFAT. Install exfat-fuse: apt install exfat-fuse"
    exit 1
fi
echo -n "Hello, exfat!" > "$TMPDIR/mnt_exfat/hello.txt"
mkdir -p "$TMPDIR/mnt_exfat/subdir"
echo -n "nested file" > "$TMPDIR/mnt_exfat/subdir/nested.txt"
sync
fusermount -u "$TMPDIR/mnt_exfat" 2>/dev/null || umount "$TMPDIR/mnt_exfat"
zstd -19 -f "$TMPDIR/exfat.img" -o "$FIXTURES/test_exfat.img.zst"
echo "exFAT done: $(stat -c%s "$FIXTURES/test_exfat.img.zst") bytes"

# ========== HFS (classic, 800KB) ==========
echo "=== Creating HFS ==="
dd if=/dev/zero of="$TMPDIR/hfs.img" bs=1k count=800 2>/dev/null
hformat -l "test_hfs" "$TMPDIR/hfs.img" >/dev/null 2>&1
hmount "$TMPDIR/hfs.img" >/dev/null 2>&1
echo -n "Hello, hfs!" > "$TMPDIR/hello_hfs.txt"
hcopy -r "$TMPDIR/hello_hfs.txt" ":hello.txt"
hmkdir ":subdir"
hcd ":subdir"
hcopy -r "$TMPDIR/nested.txt" ":nested.txt"
humount
zstd -19 -f "$TMPDIR/hfs.img" -o "$FIXTURES/test_hfs.img.zst"
echo "HFS done: $(stat -c%s "$FIXTURES/test_hfs.img.zst") bytes"

# ========== HFS+ (4MB) ==========
echo "=== Creating HFS+ ==="
dd if=/dev/zero of="$TMPDIR/hfsplus.img" bs=1M count=4 2>/dev/null
mkfs.hfsplus -v "test_hfsplus" "$TMPDIR/hfsplus.img" >/dev/null 2>&1
# hpmount/hpcopy from hfsplus package (no kernel mount needed)
hpmount "$TMPDIR/hfsplus.img" >/dev/null 2>&1
echo -n "Hello, hfsplus!" > "$TMPDIR/hello_hfsplus.txt"
hpcopy "$TMPDIR/hello_hfsplus.txt" ":hello.txt"
hpmkdir ":subdir"
hpcd ":subdir"
hpcopy "$TMPDIR/nested.txt" ":nested.txt"
hpumount
zstd -19 -f "$TMPDIR/hfsplus.img" -o "$FIXTURES/test_hfsplus.img.zst"
echo "HFS+ done: $(stat -c%s "$FIXTURES/test_hfsplus.img.zst") bytes"

echo ""
echo "=== All fixtures created ==="
ls -la "$FIXTURES/"

# Fix ownership so non-root user can read
chmod 644 "$FIXTURES"/*.img.zst
