"""Independent synthetic SquashFS 3 fixture; Python stdlib only, no firmware data.

On-disk layouts: FMK c72f45d7a062, squashfs-3.0-lzma-damn-small-variant.
Run with Python 3 to regenerate ddwrt-v3.squashfs next to this script.
"""
from pathlib import Path
import lzma
import struct


def compressed(data, lc=3, pb=2):
    filters = [{"id": lzma.FILTER_LZMA1, "dict_size": 1 << 23, "lc": lc, "lp": 0, "pb": pb}]
    return bytes([pb, lc, 0, 0]) + lzma.compress(data, format=lzma.FORMAT_RAW, filters=filters)


def metadata(data, lc, pb):
    encoded = compressed(data, lc, pb)
    return struct.pack("<H", len(encoded)) + encoded


content = b"Independent DD-WRT fixture.\n" * 64
data = compressed(content)
inode_start = 119 + len(data)
root = struct.pack("<HBBII", (0o755 << 4) | 1, 0, 255, 1700000000, 1)
root += struct.pack("<4I", 2, 22, 0, 1)
file = struct.pack("<HBBII", (0o644 << 4) | 2, 0, 255, 1700000000, 2)
file += struct.pack("<Q4I", 119, 0xffffffff, 0, len(content), len(data))
inodes = metadata(root + file, 0, 0)
directory = struct.pack("<BIIHBh5s", 0, 0, 2, 28 | (2 << 13), 4, 0, b"hello")
directories = metadata(directory, 2, 1)
directory_start = inode_start + len(inodes)
fragment_start = directory_start + len(directories)
uid_start = fragment_start
size = uid_start + 4
header = struct.pack(
    "<4s6I4H3BIQ3I7Q", b"hsqt", 2, 0, 0, 0, 0, 0,
    3, 0, 0, 17, 0, 1, 0, 1700000000, 0,
    131072, 0, 0, size, uid_start, size, inode_start, directory_start, fragment_start, 0xffffffffffffffff,
)
assert len(header) == 119
Path(__file__).with_name("ddwrt-v3.squashfs").write_bytes(header + data + inodes + directories + bytes(4))
