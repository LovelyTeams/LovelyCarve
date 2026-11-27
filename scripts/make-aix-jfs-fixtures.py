"""Generate deterministic AIX JFS1 images from the documented disk layout."""

import hashlib
from pathlib import Path
import struct
import zlib


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "src-tauri/vendor/aix-jfs-reader/tests/fixtures"
PAGE = 4096
AG_BYTES = 512 * 1024
INODES_PER_AG = 64


def mkvolume(fragment_size, extended):
    data = bytearray(2 * AG_BYTES)

    def word(offset, *values):
        struct.pack_into(">" + "I" * len(values), data, offset, *values)

    def address(offset, needed=PAGE):
        assert offset % fragment_size == 0
        units = (needed + fragment_size - 1) // fragment_size
        unused = PAGE // fragment_size - units
        return (unused << 28) | (offset // fragment_size)

    def inode(number, mode, size, blocks=None, indirect=0, inline=None, links=1):
        group, slot = divmod(number, INODES_PER_AG if extended else AG_BYTES // PAGE)
        offset = (group * AG_BYTES if group else 32 * PAGE) + slot * 128
        word(offset + 4, mode)
        struct.pack_into(">H", data, offset + 8, links)
        word(offset + 20, size, 0, 0xFFFFFFFE, 85000, 1706000001, 12000, 1706000003, 89000)
        if inline is not None:
            data[offset + 80:offset + 80 + len(inline)] = inline
        if blocks:
            word(offset + 80, *blocks)
        if indirect:
            word(offset + 116, address(indirect))

    def directory(offset, records):
        cursor = 0
        for index, (number, name) in enumerate(records):
            length = (8 + len(name) + 1 + 3) & ~3
            if index + 1 == len(records):
                length = 512 - cursor
            struct.pack_into(">IHH", data, offset + cursor, number, length, len(name))
            data[offset + cursor + 8:offset + cursor + 8 + len(name)] = name
            cursor += length

    for offset in (PAGE, PAGE * 31):
        word(offset, 0x65872143 if extended else 0x43218765, 0, AG_BYTES // fragment_size, 1, len(data) // 512)
        struct.pack_into(">H", data, offset + 20, PAGE)
        data[offset + 24:offset + 30] = b"ORACLE"
        if extended:
            word(offset + 48, 1, fragment_size, INODES_PER_AG, 0)

    subfile = (INODES_PER_AG if extended else AG_BYTES // PAGE) + 7
    directory(0x28000, [(2, b"."), (2, b".."), (5, b"tiny"), (6, b"dir"), (7, b"single"),
                         (8, b"double"), (9, b"link"), (0, b"unlinked"), (10, b"longlink")])
    directory(0x29000, [(6, b"."), (2, b".."), (5, b"alias"), (subfile, b"follow")])
    inode(2, 0o40755, 512, blocks=[address(0x28000, 512)], links=3)
    inode(5, 0o100644, 7, blocks=[address(0x2A000, 7)], links=2)
    data[0x2A000:0x2A007] = b"aixjfs\n"
    inode(6, 0o40755, 512, blocks=[address(0x29000, 512)], links=2)
    inode(7, 0o100644, 9 * PAGE, blocks=[address(0x2A000)], indirect=0x30000)
    word(0x30000, address(0x32000))
    word(0x30000 + 8 * 4, address(0x33000))
    data[0x32000:0x33000] = b"A" * PAGE
    data[0x33000:0x34000] = b"Z" * PAGE
    inode(8, 0o100644, 8 * 1024 * 1024, indirect=0x36000)
    word(0x36000 + 8 + 4, address(0x38000))
    word(0x38000 + 4, address(0x39000))
    data[0x39000:0x3A000] = bytes(range(256)) * 16
    inode(9, 0o120777, 4, inline=b"tiny")
    target = b"./" * 28 + b"tiny"
    inode(10, 0o120777, len(target), blocks=[address(0x3A000, len(target))])
    data[0x3A000:0x3A000 + len(target)] = target
    inode(subfile, 0o100644, 16, blocks=[address(0x89000, 16)])
    data[0x89000:0x89010] = b"allocation group"
    return bytes(data)


def main():
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, fragment, extended in [("fsv3", PAGE, False)] + [
        (f"fsv3p-{size}", size, True) for size in (512, 1024, 2048, 4096)
    ]:
        data = mkvolume(fragment, extended)
        output = FIXTURES / f"{name}.zlib"
        output.write_bytes(zlib.compress(data, level=9))
        print(f"{output.name} {len(data)} bytes {hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
