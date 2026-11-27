"""Build deterministic VxFS metadata and byte-range fixtures from published layouts."""

import hashlib
from pathlib import Path
import struct
import zlib


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "src-tauri/vendor/vxfs-reader/tests/fixtures"
SPARSE_OFFSET = 5 << 30


def volume(layout, order, block_size):
    blocks = 96
    image = bytearray(block_size * blocks)
    superblock = 8192 if order == ">" else 1024

    def field(data, offset, kind, *values):
        struct.pack_into(order + kind, data, offset, *values)

    def inode(offset, mode, size, org, contents=b"", links=1):
        field(image, offset, "II", mode, links)
        field(image, offset + 16, "Q", size)
        field(image, offset + 24, "IIIIII", 1712300001, 12, 1712300002, 321, 1712300003, 9876)
        image[offset + 49] = org
        image[offset + 80:offset + 80 + len(contents)] = contents

    def direct(*runs, indirect=0, unit=0):
        data = bytearray(96)
        field(data, 4, "II", unit, indirect)
        for index, run in enumerate(runs):
            field(data, 16 + index * 8, "II", *run)
        return data

    def typed(*runs):
        data = bytearray(len(runs) * 16)
        for index, (kind, logical, physical, length) in enumerate(runs):
            field(data, index * 16, "QII", (kind << 56) | logical, physical, length)
        return data

    def directory(*records, hashes=0):
        data = bytearray(4 + hashes * 2)
        field(data, 2, "H", hashes)
        for number, name in records:
            length = (10 + len(name) + 3) & ~3
            record = bytearray(length)
            field(record, 0, "IHHH", number, length, len(name), 0)
            record[10:10 + len(name)] = name
            data.extend(record)
        return data

    def data(block, contents):
        start = block * block_size
        image[start:start + len(contents)] = contents

    field(image, superblock, "II", 0xA501FCF5, layout)
    field(image, superblock + 32, "II", block_size, blocks)
    field(image, superblock + 192, "I", 25)
    image[superblock + 333] = 0x5A
    image[superblock + 348:superblock + 354] = b"ORACLE"
    field(image, superblock + 368, "III", 20, 0, 1)
    field(image, superblock + 388, "I", 256)

    # Place the two bootstrap objects in separate OLT extents.
    for block, kind, value, next_block in [(20, 2, 3, 21), (21, 4, 22, 0)]:
        pos = block * block_size
        field(image, pos, "II", 0xA504FCF5, 56)
        field(image, pos + 28, "III", block, 0, 1)
        field(image, pos + 40, "III", next_block, 0, int(next_block != 0))
        field(image, pos + 56, "IIII", kind, 16, value, 0)
        field(image, pos + 72, "II", 1, block_size - 72)

    init = 22 * block_size
    inode(init + 3 * 256, 0x10000000, block_size * 2, 1, direct((25, 2)))
    inode(init + 5 * 256, 0x20000000, block_size * 2, 3, typed((2, 0, 27, 2)))
    field(image, 25 * block_size + 48, "I", 5)
    field(image, 26 * block_size + 48, "I", 6)
    inode(27 * block_size + 6 * 256, 0x20000000, block_size * 3, 1, direct((29, 3)))

    ilist = 29 * block_size
    root = directory((3, b"tiny"), (4, b"dir"), (5, b"link"), (6, b"linear"), (7, b"empty"))
    inode(ilist + 2 * 256, 0o40755, len(root), 2, root)
    inode(ilist + 3 * 256, 0o100644, 5, 2, b"tiny\n", links=2)
    inode(ilist + 4 * 256, 0o40755, block_size * 2, 1, direct((33, 2)))
    inode(ilist + 5 * 256, 0o120777, 4, 2, b"tiny")
    inode(ilist + 6 * 256, 0o100644, block_size * 3 - 19, 1,
          direct((36, 1), (0, 1), indirect=35, unit=1))
    inode(ilist + 7 * 256, 0o100644, 0, 0)
    inode(ilist + 8 * 256, 0o100600, SPARSE_OFFSET + block_size * 2, 3,
          typed((2, 0, 40, 1), (1, SPARSE_OFFSET // block_size, 38, 1)))
    data(33, directory((0, b"discarded"), (3, b"alias"), hashes=4))
    data(34, directory((8, b"large"), hashes=2))
    field(image, 35 * block_size, "I", 37)
    data(36, b"D" * block_size)
    data(37, b"I" * block_size)
    data(38, typed((1, 1, 39, 1)))
    data(39, typed((2, 0, 41, 1)))
    data(40, b"F" * block_size)
    data(41, bytes(range(256)) * (block_size // 256))
    return bytes(image)


def main():
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, layout, order, size in [("le2", 2, "<", 1024), ("le3", 3, "<", 2048), ("be4", 4, ">", 8192)]:
        image = volume(layout, order, size)
        output = FIXTURES / f"{name}.zlib"
        output.write_bytes(zlib.compress(image, level=9))
        print(f"{output.name} {len(image)} bytes {hashlib.sha256(image).hexdigest()}")


if __name__ == "__main__":
    main()
