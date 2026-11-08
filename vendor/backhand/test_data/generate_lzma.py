"""CC0-1.0 independent SquashFS v3 fixtures, Python stdlib only.

Raw LZMA, LZMA-alone, five-byte properties and zlib; all carry the same
multi-block file, with different block parameters to exercise cache fallback.
Layouts follow SquashFS 3.0; no third-party filesystem data is included.
"""
from pathlib import Path
import lzma
import struct
import zlib

ROOT = Path(__file__).resolve().parent
CONTENT = b"Independent Rust LZMA fixture.\n" * 350
BLOCK = 4096


def compress(data, mode, lc=3, pb=2):
    if mode == "gzip":
        return zlib.compress(data)
    filters = [dict(id=lzma.FILTER_LZMA1, dict_size=1 << 23, lc=lc, lp=0, pb=pb)]
    if mode == "alone":
        return lzma.compress(data, format=lzma.FORMAT_ALONE, filters=filters)
    raw = lzma.compress(data, format=lzma.FORMAT_RAW, filters=filters)
    if mode == "props":
        return bytes([(pb * 5) * 9 + lc]) + struct.pack("<I", 1 << 23) + raw
    return raw


def image(mode, big=False):
    endian = ">" if big else "<"
    def pack(fmt, *args):
        return struct.pack(endian + fmt, *args)
    def inode(kind, permissions, number):
        bits = (kind << 12) | permissions if big else kind | (permissions << 4)
        return pack("HBBII", bits, 0, 255, 1700000000, number)
    def metadata(data, lc, pb):
        encoded = compress(data, mode, lc, pb)
        return pack("H", len(encoded)) + encoded

    blocks = [compress(CONTENT[start:start+BLOCK], mode, lc, pb)
              for start, (lc, pb) in zip(range(0, len(CONTENT), BLOCK), [(3, 2), (0, 0), (2, 1)])]
    assert len(blocks) == 3
    data = b"".join(blocks)
    # Directory size is its encoded byte count plus SquashFS's three-byte bias.
    dir_bits = (28 << 3) | 2 if big else 28 | (2 << 13)
    directory = pack("BIIHBh5s", 0, 0, 2, dir_bits, 4, 0, b"hello")
    size_bits = (len(directory) + 3) << 13 if big else len(directory) + 3
    root = inode(1, 0o755, 1) + pack("4I", 2, size_bits, 0, 1)
    file = inode(2, 0o644, 2) + pack("Q3I", 119, 0xffffffff, 0, len(CONTENT))
    file += pack("3I", *(len(block) for block in blocks))
    inodes = metadata(root + file, 0, 0)
    directories = metadata(directory, 2, 1)
    inode_start = 119 + len(data)
    directory_start = inode_start + len(inodes)
    uid_start = directory_start + len(directories)
    size = uid_start + 4
    header = pack(
        "4s6I4H3BIQ3I7Q", b"sqsh" if big else b"hsqs", 2, 0, 0, 0, 0, 0,
        3, 0, 0, 12, 0, 1, 0, 1700000000, 0,
        BLOCK, 0, 0, size, uid_start, size, inode_start, directory_start,
        uid_start, 0xffffffffffffffff,
    )
    assert len(header) == 119
    return header + data + inodes + directories + bytes(4)


if __name__ == "__main__":
    for mode in ["raw", "alone", "props", "gzip"]:
        if mode != "gzip":
            (ROOT / f"lzma-{mode}.block").write_bytes(compress(CONTENT[:BLOCK], mode))
        for big in [False, True]:
            (ROOT / f"lzma-v3-{mode}-{'be' if big else 'le'}.squashfs").write_bytes(image(mode, big))
