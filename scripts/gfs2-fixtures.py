#!/usr/bin/env python3
"""CC0-1.0 independent invented GFS2 encoder. Not a native mkfs oracle."""
import hashlib
import json
import pathlib
import struct
import zlib


def build():
    size = 4096
    image = bytearray(4096 * size)
    allocated = {}

    def put(raw, offset, code, *values):
        struct.pack_into(">" + code, raw, offset, *values)

    def meta(raw, kind, version):
        put(raw, 0, "IIQII", 0x01161970, kind, 0, version, 0)

    def block(number, inode=False):
        allocated[number] = 3 if inode else 1
        return memoryview(image)[number * size:(number + 1) * size]

    numbers = {32: 2, 33: 1, 34: 3, 35: 4, 36: 6, 37: 7, 38: 8, 39: 9, 40: 5, 41: 10}

    def inode(number, mode, data=b"", length=None, blocks=1, height=0, entries=0):
        raw = block(number, True)
        meta(raw, 4, 400)
        put(raw, 24, "QQIIIIQQQQQ", numbers[number], number, mode, 1000, 1000, 1,
            len(data) if length is None else length, blocks, (-315619200) & ((1 << 64) - 1), 1700000000, 1700000001)
        put(raw, 138, "H", height)
        put(raw, 148, "I", entries)
        put(raw, 176, "III", 123, 456, 789)
        raw[232:232 + len(data)] = data
        return raw

    def directory(number, parent, entries):
        values = [(".", number, 4), ("..", parent, 4)] + entries
        raw = inode(number, 0x41ed, length=size - 232, entries=len(values))
        at = 232
        for index, (name, target, kind) in enumerate(values):
            name = name.encode()
            length = ((40 + len(name) + 7) // 8) * 8 if index + 1 < len(values) else size - at
            put(raw, at, "QQIHHHH", numbers[target], target, zlib.crc32(name), length, len(name), kind, 0)
            raw[at + 40:at + 40 + len(name)] = name
            at += length

    sb = memoryview(image)[65536:65536 + 272]
    meta(sb, 1, 100)
    put(sb, 24, "IIIIII", 1802, 1900, 0, size, 12, 0)
    put(sb, 48, "QQ", 2, 32)
    put(sb, 80, "QQ", 1, 33)
    sb[96:107] = b"lock_nolock"
    sb[160:179] = b"lovelycarve:fixture"
    sb[256:272] = bytes(range(16))
    directory(32, 32, [("rindex", 34, 8), ("jindex", 35, 4)])
    directory(33, 33, [("hello", 36, 8), ("hardlink", 36, 8), ("sub", 37, 4), ("link", 38, 10), ("fifo", 39, 1), ("sparse", 41, 8)])
    directory(35, 32, [("journal0", 40, 8)])
    directory(37, 33, [("nested", 36, 8)])
    inode(36, 0x81a4, b"hello native-style GFS2\n")
    inode(38, 0xa1ff, b"sub/nested")
    inode(39, 0x11a4)
    raw = inode(41, 0x81a4, length=3 * size - 7, blocks=2, height=1)
    put(raw, 240, "Q", 300)
    block(300)[:] = bytes((index * 17 + 5) & 255 for index in range(size))
    raw = inode(40, 0x8180, length=8 * 1024 * 1024, blocks=2054, height=2)
    for index in range(5):
        put(raw, 232 + index * 8, "Q", 100 + index)
        pointer = block(100 + index)
        meta(pointer, 5, 500)
        for slot in range(min(509, 2048 - index * 509)):
            number = 512 + index * 509 + slot
            put(pointer, 24 + slot * 8, "Q", number)
            block(number)
    # Native-sized journal with one verified latest clean-unmount header.
    raw = block(512)
    meta(raw, 8, 800)
    put(raw, 24, "QIII", 7, 1, 0, 0)
    put(raw, 44, "I", zlib.crc32(raw[:48]))
    index = bytearray(96)
    put(index, 0, "QIIQII", 17, 1, 0, 18, 4078, 1020)
    inode(34, 0x8180, index)
    # Inline raw EA and a separately allocated ED value.
    raw = block(200)
    meta(raw, 10, 1600)
    name, value = b"inline", b"\0\xfffixture"
    put(raw, 24, "II4B", 32, len(value), len(name), 1, 0, 0)
    raw[40:40 + len(name)] = name
    raw[40 + len(name):40 + len(name) + len(value)] = value
    name = b"external"
    put(raw, 56, "II4B", size - 56, 100, len(name), 1, 1, 1)
    raw[72:80] = name
    put(raw, 80, "Q", 201)
    raw = block(201)
    meta(raw, 11, 1700)
    raw[24:124] = bytes(range(100))
    put(memoryview(image)[36 * size:37 * size], 168, "Q", 200)
    rg = memoryview(image)[17 * size:18 * size]
    meta(rg, 2, 200)
    put(rg, 24, "IIIIQQIII", 0, 4078 - len(allocated), len(numbers), 0, 100, 18, 4078, 1020, 0)
    for number, state in allocated.items():
        relative = number - 18
        rg[128 + relative // 4] |= state << ((relative % 4) * 2)
    put(rg, 64, "I", zlib.crc32(rg[:128]))
    return bytes(image)


if __name__ == "__main__":
    directory = pathlib.Path(__file__).resolve().parents[1] / "src-tauri/vendor/gfs2-reader/tests/fixtures"
    directory.mkdir(exist_ok=True, parents=True)
    raw = build()
    packed = zlib.compress(raw, 9)
    (directory / "invented-4096.zlib").write_bytes(packed)
    (directory / "manifest.json").write_text(json.dumps({"license": "CC0-1.0", "generator": "scripts/gfs2-fixtures.py", "decoded_bytes": len(raw), "decoded_sha256": hashlib.sha256(raw).hexdigest(), "compressed_sha256": hashlib.sha256(packed).hexdigest()}, indent=2) + "\n", encoding="utf8")
