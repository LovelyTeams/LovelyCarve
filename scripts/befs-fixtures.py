#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Independent invented BFS layouts, encoded without importing Rust readers.

Not a native formatter/fsck oracle. Haiku's published packed structs define
the bytes; two endian orders and both native superblock positions are covered.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib

OUTPUT = Path(__file__).resolve().parent.parent / 'src-tauri/vendor/befs-reader/tests/fixtures'


def build(block_size=1024, big=False, blocks=2560, group_shift=None):
    order = '>' if big else '<'
    group_shift = group_shift or (block_size * 8).bit_length() - 1
    per_group = 1 << group_shift
    groups = (blocks + per_group - 1) // per_group
    bitmap_pages = (blocks + block_size * 8 - 1) // (block_size * 8)
    log_first = bitmap_pages + 1
    log_count = 32
    medium = bytearray(blocks * block_size)
    used = set(range(log_first + log_count))

    def pack(buffer, offset, code, *values):
        struct.pack_into(order + code, buffer, offset, *values)

    def run(first, length=1):
        return struct.pack(order + 'IHH', first >> group_shift, first & (per_group - 1), length)

    def write_block(first, data):
        assert len(data) % block_size == 0
        used.update(range(first, first + len(data) // block_size))
        medium[first * block_size:first * block_size + len(data)] = data

    def stream(direct=(), indirect=None, second=None, direct_length=0, indirect_length=0, double_length=0, length=0):
        raw = bytearray(144)
        for index, (first, count) in enumerate(direct):
            raw[index * 8:index * 8 + 8] = run(first, count)
        pack(raw, 96, 'q', direct_length)
        if indirect:
            raw[104:112] = run(*indirect)
        pack(raw, 112, 'q', indirect_length)
        if second:
            raw[120:128] = run(*second)
        pack(raw, 128, 'qq', double_length, length)
        return raw

    def one_stream(first, data):
        count = (len(data) + block_size - 1) // block_size
        if count:
            write_block(first, data + bytes(count * block_size - len(data)))
        return stream([(first, count)] if count else (), direct_length=count * block_size, length=len(data))

    def inode(number, parent, mode, data=None, attributes=0, small=(), flags=1, type_code=0):
        raw = bytearray(block_size)
        pack(raw, 0, 'I', 0x3bbe0ad9)
        raw[4:12] = run(number)
        pack(raw, 12, 'IIIIqq', 1000, 100, mode, flags, (-315619200 << 16) | 0x2345, (1700000000 << 16) | 0xf123)
        raw[44:52] = run(parent)
        if attributes:
            raw[52:60] = run(attributes)
        pack(raw, 60, 'II', type_code, block_size)
        if data is not None:
            raw[72:72 + len(data)] = data
        # This is reserved on BeOS: it must not automatically become ctime.
        pack(raw, 216, 'q', 0x123456789)
        at = 232
        for name, kind, payload in small:
            name = name.encode('utf8') if isinstance(name, str) else name
            size = 8 + len(name) + 3 + len(payload) + 1
            assert at + size + 8 <= block_size
            pack(raw, at, 'IHH', kind, len(name), len(payload))
            raw[at + 8:at + 8 + len(name)] = name
            raw[at + 11 + len(name):at + 11 + len(name) + len(payload)] = payload
            at += size
        write_block(number, raw)

    def node(entries, left=-1, right=-1, overflow=-1):
        raw = bytearray(1024)
        keys = b''.join(name for name, _ in entries)
        pack(raw, 0, 'qqqHH', left, right, overflow, len(entries), len(keys))
        raw[28:28 + len(keys)] = keys
        lengths = (28 + len(keys) + 7) & ~7
        end = 0
        for index, (name, value) in enumerate(entries):
            end += len(name)
            pack(raw, lengths + index * 2, 'H', end)
            pack(raw, lengths + len(entries) * 2 + index * 8, 'q', value)
        assert lengths + len(entries) * 10 <= 1024
        return raw

    def tree(entries, split=False):
        entries = sorted((name.encode('utf8'), value) for name, value in entries)
        count = 5 if split else 2
        raw = bytearray(count * 1024)
        pack(raw, 0, 'IIIIqqq', 0x69f6c2e8, 1024, 2 if split else 1, 0, 1024, 4096 if split else -1, count * 1024)
        if split:
            middle = len(entries) // 2
            raw[1024:2048] = node([(entries[middle - 1][0], 2048)], overflow=3072)
            raw[2048:3072] = node(entries[:middle], right=3072)
            raw[3072:4096] = node(entries[middle:], left=2048)
            pack(raw, 4096, 'qqq', -1, -1, -2)
        else:
            raw[1024:2048] = node(entries)
        return raw

    name = 'é' * 100 + '.bin'
    entries = [('.', 64), ('..', 64), ('big.bin', 67), ('alias.bin', 67),
               ('sub', 65), ('link', 68), ('longlink', 69), ('empty', 83),
               ('pipe', 84), ('中文\\evidence', 82), (name, 85)]
    inode(64, 64, 0x41ed, one_stream(150, tree(entries, split=True)))
    inode(65, 64, 0x41ed, one_stream(160, tree([('.', 65), ('..', 64), ('nested.txt', 66)])))
    inode(66, 65, 0x81a4, one_stream(180, b'nested evidence'))
    inode(68, 64, 0xa1ff, b'sub/nested.txt\0')
    target = b'relative/' + b'x' * 200
    inode(69, 64, 0xa1ff, one_stream(181, target), flags=0x41)
    inode(82, 64, 0x81a4, one_stream(90000 if blocks > 90000 else 9000 if blocks > 9000 else 182, b'names'))
    inode(83, 64, 0x81a4, stream())
    inode(84, 64, 0x11a4, stream())
    inode(85, 64, 0x81a4, one_stream(183, b'long filename'))

    direct = [(300 + 2 * index, 1) for index in range(12)]
    indirect = [(400 + 2 * index, 1) for index in range(6)]
    second = [(500, 4), (512, 4)]
    length = 26 * block_size - 37
    expected = bytes((index * 17 + 5) & 255 for index in range(length))
    logical = 0
    for first, count in direct + indirect + second:
        take = min(count * block_size, length - logical)
        write_block(first, expected[logical:logical + take] + bytes(count * block_size - take))
        logical += take
    table = bytearray(2 * block_size)
    # Per-block padding is permitted; ranges resume in the next array block.
    for index, value in enumerate(indirect):
        at = (index // 3) * block_size + (index % 3) * 8
        table[at:at + 8] = run(*value)
    write_block(100, table)
    outer = bytearray(4 * block_size)
    outer[:8] = run(114, 4)
    write_block(110, outer)
    inner = bytearray(4 * block_size)
    for index, value in enumerate(second):
        inner[index * 8:index * 8 + 8] = run(*value)
    write_block(114, inner)
    inode(67, 64, 0x81a4, stream(direct, (100, 2), (110, 4),
          12 * block_size, 18 * block_size, 26 * block_size, length),
          attributes=80, small=[(b'\x13', 0x43535452, b'big.bin'),
                                 ('BEOS:TYPE', 0x4d494d53, b'text/plain\0'),
                                 ('custom', 0x52415754, b'\x00\xffinline\x80')])
    inode(80, 67, 0x080041b6, one_stream(170, tree([('big-attribute', 81)])))
    attribute = bytes((index * 23 + 7) & 255 for index in range(12003))
    inode(81, 80, 0x100081b6, one_stream(1000, attribute), flags=1, type_code=0x52415754)

    sb = bytearray(512)
    sb[:9] = b'BeFS Test'
    pack(sb, 32, 'IIIIqqIIIIII', 0x42465331, 0x42494745, block_size,
         block_size.bit_length() - 1, blocks, len(used), block_size, 0xdd121031,
         per_group // (block_size * 8), group_shift, groups, 0x434c454e)
    sb[88:96] = run(log_first, log_count)
    pack(sb, 96, 'qqI', log_first, log_first, 0x15b6830e)
    sb[116:124] = run(64)
    offset = 0 if big else 512
    medium[offset:offset + 512] = sb
    for number in used:
        page = 1 + number // (block_size * 8)
        bit = number % (block_size * 8)
        at = page * block_size + (bit // 32) * 4
        value, = struct.unpack_from(order + 'I', medium, at)
        pack(medium, at, 'I', value | (1 << (bit % 32)))
    return bytes(medium)


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    cases = [(size, big, 2560, None) for size in (1024, 2048, 4096, 8192) for big in (False, True)]
    cases += [(1024, False, 10000, None), (1024, True, 100000, 16)]
    for size, big, blocks, shift in cases:
        name = f'{"be" if big else "le"}-{size}-{blocks}.zlib'
        raw = build(size, big, blocks, shift)
        compressed = zlib.compress(raw, 9)
        (OUTPUT / name).write_bytes(compressed)
        manifest.append({'file': name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
                         'compressed_sha256': hashlib.sha256(compressed).hexdigest(),
                         'license': 'CC0-1.0', 'source': 'scripts/befs-fixtures.py; independent encoding, not a native formatter oracle'})
    (OUTPUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8', newline='\n')


if __name__ == '__main__':
    main()
