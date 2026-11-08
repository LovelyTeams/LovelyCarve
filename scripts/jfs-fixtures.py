#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Encode minimal independent JFS read fixtures (not a formatter/fsck oracle).

Two IAGs, literal Unix names, indexed/legacy directory heads, sparse/unwritten
content, hardlinks, 200-byte inline links and external links at every block size.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib

OUTPUT = Path(__file__).resolve().parent.parent / 'vendor/jfs-reader/tests/fixtures'
STAMP = 0x12345678
LONG_NAME = 'long_中文_' + 'x' * 180 + '\\tail'


def u16(data, offset, value):
    struct.pack_into('<H', data, offset, value)


def u32(data, offset, value):
    struct.pack_into('<I', data, offset, value)


def u64(data, offset, value):
    struct.pack_into('<Q', data, offset, value)


def pxd(start, count):
    return struct.pack('<II', count | ((start >> 32) << 24), start & 0xffffffff)


def xmap(runs, maximum=10):
    data = bytearray(maximum * 16)
    data[16] = 0x83
    u16(data, 18, 2 + len(runs))
    u16(data, 20, maximum)
    for index, (logical, physical, count, flags) in enumerate(runs):
        offset = 32 + index * 16
        data[offset] = flags
        data[offset + 3] = logical >> 32
        u32(data, offset + 4, logical & 0xffffffff)
        data[offset + 8:offset + 16] = pxd(physical, count)
    return data


def directory(entries, block_size, indexed, physical=None, parent=2):
    external = physical is not None
    data = bytearray(4096 if external else 288)
    maximum = 128 if external else 9
    table_slot = 124 if external else 0
    table = table_slot * 32 if external else 24
    data[16] = 0x82 if external else 0x83
    data[17] = len(entries)
    if external:
        data[20:22] = bytes([128, table_slot])
        data[24:32] = pxd(physical, 4096 // block_size)
    else:
        u32(data, 20, parent)
    available = list(range(1, table_slot if external else maximum))
    locations = []
    for order, (name, ino) in enumerate(entries):
        raw = name.encode('utf-16le')
        units = len(raw) // 2
        head_units = 11 if indexed else 13
        needed = 1 + max(0, units - head_units + 14) // 15
        selected, available = available[:needed], available[needed:]
        data[table + order] = selected[0]
        head = selected[0] * 32
        u32(data, head, ino)
        data[head + 4] = selected[1] if len(selected) > 1 else 255
        data[head + 5] = units
        take = min(units, head_units)
        data[head + 6:head + 6 + take * 2] = raw[:take * 2]
        if indexed:
            u32(data, head + 28, order + 2)
        cursor = take * 2
        for index, slot in enumerate(selected[1:]):
            offset = slot * 32
            data[offset] = selected[index + 2] if index + 2 < len(selected) else 255
            data[offset + 1] = 1  # slot count, never a character count
            take = min(30, len(raw) - cursor)
            data[offset + 2:offset + 2 + take] = raw[cursor:cursor + take]
            cursor += take
        locations.append(bytes([0, 1, order, 0]) + struct.pack('<I', physical or 0))
    data[18] = len(available)
    data[19] = available[0] if available else 255
    for index, slot in enumerate(available):
        data[slot * 32] = available[index + 1] if index + 1 < len(available) else 255
        data[slot * 32 + 1] = 1
    return data, b''.join(locations)


def make(block_size, indexed):
    image = bytearray(1024 * 1024)
    total = len(image) // block_size
    used = set()

    def allocate(offset, length):
        assert offset % block_size == 0 and length % block_size == 0
        used.update(range(offset // block_size, (offset + length) // block_size))

    def write(offset, content, allocated=None):
        image[offset:offset + len(content)] = content
        if allocated:
            allocate(offset, allocated)

    allocate(0, 65536)
    header = memoryview(image)[32768:33280]
    header[:4] = b'JFS1'
    u32(header, 4, 1)
    u64(header, 8, len(image) // 512)
    u32(header, 16, block_size)
    u16(header, 20, block_size.bit_length() - 1)
    u16(header, 22, (block_size // 512).bit_length() - 1)
    u32(header, 24, 512)
    u16(header, 28, 9)
    u32(header, 32, 8192)
    u32(header, 36, 0x10000101 | (0x00200000 if indexed else 0))
    header[152:159] = b'fixture'
    image[61440:61952] = header

    def inode(number, mode, size, blocks, root=None, fileset=16, parent_extent=131072, nlink=1):
        raw = bytearray(512)
        struct.pack_into('<IIII', raw, 0, STAMP, fileset, number, 17 + number)
        raw[16:24] = pxd(parent_extent // block_size, 16384 // block_size)
        u64(raw, 24, size)
        u64(raw, 32, blocks)
        u32(raw, 40, nlink)
        u32(raw, 52, mode)
        for offset, seconds, nanos in [(56, 0, 0), (64, 4_000_000_001, 7), (72, 4_000_000_000, 123456789), (80, 1, 0)]:
            u32(raw, offset, seconds)
            u32(raw, offset + 4, nanos)
        if root:
            raw[224:224 + len(root)] = root
        return raw

    def user(number, raw):
        extent = 131072 if number < 4096 else 163840
        write(extent + (number % 32) * 512, raw)
        allocate(extent, 16384)

    for number, start, length in [(2, 65536, 6 * 4096), (16, 98304, 3 * 4096)]:
        raw = inode(number, 0x18000, length, length // block_size,
                    xmap([(0, start // block_size, length // block_size, 0)], 18), 1, 45056)
        write(45056 + number * 512, raw)
        allocate(start, length)
    control = memoryview(image)[98304:102400]
    u32(control, 4, 2)
    u32(control, 8, 64)
    u32(control, 12, 55)
    u32(control, 16, 16384 // block_size)
    u32(control, 20, (16384 // block_size).bit_length() - 1)
    for group, allocated, extent in [(0, [2, 4, 5, 6, 7, 8, 9], 131072), (1, [2, 3], 163840)]:
        page = memoryview(image)[98304 + (group + 1) * 4096:98304 + (group + 2) * 4096]
        u32(page, 8, group)
        word = sum(0x80000000 >> number for number in allocated)
        u32(page, 2048, word)
        u32(page, 2560, word)
        page[3072:3080] = pxd(extent // block_size, 16384 // block_size)
        for offset in [32, 36, 40, 44]:
            u32(page, offset, 0x7fffffff if offset == 32 else 0xffffffff)
        u32(page, 48, 0x80000000)
        u32(page, 64, 32 - len(allocated))
        u32(page, 68, 127)

    entries = [('alias.bin', 4), ('big.bin', 4), ('empty', 9), ('link-external', 7),
               ('link-long', 6), ('link-short', 5), (LONG_NAME, 8), ('sub', 4098)]
    entries.sort(key=lambda entry: entry[0].encode('utf-16be'))
    leaf, locations = directory(entries, block_size, indexed, 196608 // block_size)
    write(196608, leaf, 4096)
    # Inline directory root is an index with an empty lower separator.
    root = bytearray(288)
    root[16:20] = bytes([0x85, 1, 7, 2])
    u32(root, 20, 2)
    root[24] = 1
    root[32:40] = pxd(196608 // block_size, 4096 // block_size)
    root[40] = 255
    for slot in range(2, 9):
        root[slot * 32] = slot + 1 if slot < 8 else 255
        root[slot * 32 + 1] = 1
    raw = inode(2, 0x141ed, len(entries) * 8 if indexed else 256, 4096 // block_size, root, nlink=3)
    if indexed:
        raw[128:128 + len(locations)] = locations
        u32(raw, 120, len(entries) + 2)
    user(2, raw)
    subroot, subindex = directory([('nested.txt', 4099)], block_size, indexed, parent=2)
    raw = inode(4098, 0x141ed, 8 if indexed else 256, 0, subroot, parent_extent=163840, nlink=2)
    if indexed:
        raw[128:136] = subindex
        u32(raw, 120, 3)
    user(4098, raw)
    ea = struct.pack('<IBBH', 17, 0, 4, 4) + b'note\0test'
    raw = inode(4, 0x681a4, 12 * block_size + 37, 6,
                xmap([(0, 262144 // block_size, 2, 0), (6, 278528 // block_size, 1, 8),
                      (10, 294912 // block_size, 1, 0), (12, 311296 // block_size, 1, 0)]), nlink=2)
    raw[104] = 0x20
    u32(raw, 108, len(ea))
    raw[112:120] = pxd(360448 // block_size, 1)
    user(4, raw)
    write(360448, ea, block_size)
    for offset, count, factor in [(262144, 2, 17), (278528, 1, 19), (294912, 1, 23), (311296, 1, 29)]:
        write(offset, bytes((index * factor + 5) & 255 for index in range(count * block_size)), count * block_size)
    for number, offset, content, extent in [(8, 344064, b'unicode path', 131072), (4099, 327680, b'hello JFS', 163840),
                                              (7, 229376, b'../' + b'x' * 297, 131072)]:
        raw = inode(number, 0x6a1ff if number == 7 else 0x681a4, len(content), 1,
                    xmap([(0, offset // block_size, 1, 0)]), parent_extent=extent)
        if number == 8:
            raw[104] = 0x40
            u32(raw, 108, len(ea))
            raw[384:384 + len(ea)] = ea
        user(number, raw)
        write(offset, content, block_size)
    for number, content in [(5, b'big.bin'), (6, b'./' + b's' * 198)]:
        raw = inode(number, 0x6a1ff if number == 5 else 0x2a1ff, len(content), 0)
        raw[256:256 + len(content)] = content
        user(number, raw)
    user(9, inode(9, 0x681a4, 0, 0, xmap([])))

    control = memoryview(image)[65536:69632]
    u64(control, 0, total)
    u64(control, 8, total - len(used))
    u32(control, 16, (4096 // block_size).bit_length() - 1)
    page = memoryview(image)[65536 + 4 * 4096:65536 + 5 * 4096]
    u32(page, 0, total)
    u32(page, 4, total - len(used))
    for word in range(256):
        value = sum(0x80000000 >> bit for bit in range(32) if word * 32 + bit >= total or word * 32 + bit in used)
        u32(page, 2048 + word * 4, value)
        u32(page, 3072 + word * 4, value)
    return image


if __name__ == '__main__':
    OUTPUT.mkdir(exist_ok=True, parents=True)
    manifest = []
    for block_size in [512, 1024, 2048, 4096]:
        for indexed in [False, True]:
            raw = make(block_size, indexed)
            path = OUTPUT / f'multi-iag-{block_size}-{"indexed" if indexed else "legacy"}.zlib'
            path.write_bytes(zlib.compress(raw, 9))
            manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
                             'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'CC0-1.0', 'source': 'scripts/jfs-fixtures.py'})
    path = OUTPUT / 'jfsutils.img.zst'
    manifest.append({'file': path.name, 'bytes': 16777216, 'sha256': '22da608bc8e42989045d69c9bbe61f488a3add26ede6b1fd7f8c50b7a31ba2d5', 'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                     'source': 'https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/tests/fixtures/test_jfs.img.zst', 'license': 'AGPL-3.0-only'})
    (OUTPUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
