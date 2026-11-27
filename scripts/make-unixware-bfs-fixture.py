"""Add known flat files to the independently formatted empty BFS image."""

import hashlib
from pathlib import Path
import struct
import zlib


FIXTURES = Path(__file__).resolve().parents[1] / "src-tauri/vendor/unixware-bfs-reader/tests/fixtures"


def main():
    image = bytearray(zlib.decompress((FIXTURES / "util-linux-empty.zlib").read_bytes()))
    root_entry = 512
    root_block, root_last = struct.unpack_from("<II", image, root_entry + 4)
    root_start = root_block * 512
    payload = bytes(range(256)) * 5 + b"unixware\n"
    files = [(3, root_last + 1, payload, 2), (4, 0, b"", 1), (5, root_last + 4, b"UnixWare BFS fixture\n", 1)]
    for inode, block, data, links in files:
        last = block + (len(data) + 511) // 512 - 1 if data else 0
        eof = block * 512 + len(data) - 1 if data else 0xFFFFFFFF
        struct.pack_into("<HH11I16x", image, 512 + (inode - 2) * 64, inode, 0, block, last, eof,
                         1, 0o644, 0, 0, links, 1706010000, 1706010001, 1706010002)
        image[block * 512:block * 512 + len(data)] = data
    records = [(3, b"unix"), (0, b"deleted-data"), (3, b"unix.old"), (4, b"empty"), (5, b"1234567890.dat")]
    for index, (inode, name) in enumerate(records, start=2):
        struct.pack_into("<H14s", image, root_start + 16 * index, inode, name)
    struct.pack_into("<I", image, root_entry + 12, root_start + 16 * (len(records) + 2) - 1)
    (FIXTURES / "populated.zlib").write_bytes(zlib.compress(image, 9))
    print(f"{len(image)} bytes {hashlib.sha256(image).hexdigest()}")


if __name__ == "__main__":
    main()
