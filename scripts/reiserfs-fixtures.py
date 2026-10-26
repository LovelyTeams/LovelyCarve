#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Independent ReiserFS layout fixtures, not a formatter or fsck oracle.

Encode mixed keys, internal nodes, an object spanning leaves, both bitmap
layouts, sparse pointers, packed tails, hardlinks and Unix names. No Rust
reader code is imported. The external reiserfsprogs fixture is kept unchanged.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib

OUTPUT = Path(__file__).resolve().parent.parent / 'src-tauri/vendor/reiserfs-reader/tests/fixtures'


def key(pair, offset, kind, version):
    if version:
        return struct.pack('<IIQ', *pair, offset | kind << 60)
    unique = [0, 0xfffffffe, 0xffffffff, 500][kind]
    return struct.pack('<IIII', *pair, offset, unique)


def stat(pair, mode, size, version, links=1):
    if version:
        body = struct.pack('<HHIQIIIIIII', mode, 0, links, size, 1000, 1000, 0, 4000000000, 1700000000, 8, 1)
    else:
        body = struct.pack('<HHHHIIIIII', mode, links, 1000, 1000, size, 0, 4000000000, 1700000000, 8, 0xffffffff)
    return (pair, 0, 0, version, 0xffff, body)


def directory(pair, parent, entries, version):
    # Hash order values are invented and monotonic; listing does not use the
    # selected hash algorithm to search by name.
    entries = [(b'.', pair, True), (b'..', parent, True)] + entries
    names = bytearray()
    heads = bytearray()
    encoded = []
    for name, target, visible in entries:
        encoded.append(name + b'\0' * (-len(name) % 8))
    location = 16 * len(entries) + sum(map(len, encoded))
    for index, ((name, target, visible), raw) in enumerate(zip(entries, encoded)):
        location -= len(raw)
        offset = index + 1 if index < 2 else (index + 1) * 128
        heads += struct.pack('<IIIHH', offset, *target, location, 4 if visible else 0)
    for raw in reversed(encoded):
        names += raw
    body = heads + names
    return [stat(pair, 0x41ed, len(body), version, 2), (pair, 1, 3, 0, len(entries), body)]


def make(block_size, version, old_layout=False, relocated=False):
    blocks = 1024
    # At 1K, exceed one bitmap so both distributed and old contiguous maps
    # participate. The sparse source compresses to a few KiB.
    if block_size == 1024:
        blocks = 9000
    image = bytearray(blocks * block_size)
    sb = 8192 if old_layout else 65536
    bitmap0 = sb // block_size + 1
    journal = bitmap0 + (blocks + block_size * 8 - 1) // (block_size * 8) + 1
    first_tree = journal + 5
    file_blocks = [blocks - 7, blocks - 6]
    expected = bytes((i * 17 + 5) & 255 for i in range(block_size)) + bytes(block_size) + bytes((i * 23 + 7) & 255 for i in range(block_size)) + b'packed tail'
    root, sub = (1, 2), (2, 5)
    big = (2, 4)
    long_name = '中文\\evidence'.encode()
    items = directory(root, (0, 1), [(b'big.bin', big, True), (b'alias.bin', big, True), (b'sub', sub, True),
        (b'link', (2, 7), True), (long_name, (2, 8), True), (b'.reiserfs_priv', (2, 9), True),
        (b'hidden', (0, 0), False), (b'Empty', (2, 10), True), (b'empty', (2, 11), True)], version)
    items += [stat(big, 0x81a4, len(expected), version, 2),
        (big, 1, 1, version, 0, struct.pack('<II', file_blocks[0], 0)),
        (big, 2 * block_size + 1, 1, version, 0, struct.pack('<I', file_blocks[1])),
        (big, 3 * block_size + 1, 2, version, 0xffff, b'packed tail' + b'\0' * 5)]
    items += directory(sub, root, [(b'nested', (88, 6), True)], version)
    items += [stat((2, 7), 0xa1ff, 7, version), ((2, 7), 1, 2, 0, 0, b'big.bin\0')]
    items += [stat((2, 8), 0x81a4, 5, version), ((2, 8), 1, 2, version, 0xffff, b'names\0\0\0')]
    items += directory((2, 9), root, [], version)
    items += [stat((2, 10), 0x81a4, 0, version), stat((2, 11), 0x81a4, 0, version)]
    items += [stat((88, 6), 0x81a4, 6, version), ((88, 6), 1, 2, version, 0xffff, b'nested\0\0')]
    items.sort(key=lambda item: (*item[0], item[1], item[2]))
    used = set(range(journal + 5)) | set(file_blocks)

    def page_leaf(group, number):
        page = bytearray(block_size)
        location = block_size
        for index, (pair, offset, kind, fmt, count, body) in enumerate(group):
            location -= len(body)
            pos = 24 + index * 24
            page[pos:pos + 24] = key(pair, offset, kind, fmt) + struct.pack('<HHHH', count, len(body), location, fmt)
            page[location:location + len(body)] = body
        free = location - 24 - len(group) * 24
        assert free >= 0
        struct.pack_into('<HHH', page, 0, 1, len(group), free)
        image[number * block_size:(number + 1) * block_size] = page
        used.add(number)
        first = group[0]
        return key(first[0], first[1], first[2], first[3]), number, block_size - 24 - free

    # Deliberately split the large file's items across leaves.
    groups = [items[:2], items[2:4], items[4:6]] + [items[i:i + 2] for i in range(6, len(items), 2)]
    leaves = [page_leaf(group, first_tree + index) for index, group in enumerate(groups)]

    def internal(children, number, level):
        page = bytearray(block_size)
        count = len(children) - 1
        occupied = count * 16 + len(children) * 8
        struct.pack_into('<HHH', page, 0, level, count, block_size - 24 - occupied)
        for index, child in enumerate(children[1:]):
            page[24 + index * 16:40 + index * 16] = child[0]
        for index, child in enumerate(children):
            struct.pack_into('<IHH', page, 24 + count * 16 + index * 8, child[1], child[2], 0)
        image[number * block_size:(number + 1) * block_size] = page
        used.add(number)
        return children[0][0], number, occupied

    middle = [internal(leaves[:6], first_tree + 40, 2), internal(leaves[6:], first_tree + 41, 2)]
    root_block = first_tree + 42
    internal(middle, root_block, 3)
    image[file_blocks[0] * block_size:(file_blocks[0] + 1) * block_size] = expected[:block_size]
    image[file_blocks[1] * block_size:(file_blocks[1] + 1) * block_size] = expected[2 * block_size:3 * block_size]
    header = memoryview(image)[sb:sb + 116]
    struct.pack_into('<III', header, 0, blocks, blocks - len(used), root_block)
    struct.pack_into('<III', header, 12, journal, 0, 3)
    struct.pack_into('<HHHH', header, 44, block_size, 2, 2, 1)
    header[52:62] = b'ReIsEr3Fs\0' if relocated else b'ReIsEr2Fs\0' if version else b'ReIsErFs\0\0'
    struct.pack_into('<IHHH', header, 64, 3, 4, (blocks + block_size * 8 - 1) // (block_size * 8), 2 if version else 0)
    if version:
        header[84:100] = bytes(range(16))
        header[100:116] = b'invented-reiser'.ljust(16, b'\0')
    for index in range((blocks + block_size * 8 - 1) // (block_size * 8)):
        bitmap = bitmap0 + index if old_layout or index == 0 else index * block_size * 8
        used.add(bitmap)
        for block in range(index * block_size * 8, min((index + 1) * block_size * 8, blocks)):
            if block in used:
                bit = block % (block_size * 8)
                image[bitmap * block_size + bit // 8] |= 1 << (bit % 8)
    return image


if __name__ == '__main__':
    OUTPUT.mkdir(exist_ok=True, parents=True)
    manifest = []
    for block_size, version, old, relocated in [(1024, 0, True, False), (1024, 1, False, False),
            (2048, 0, False, False), (4096, 1, False, True), (8192, 1, False, False)]:
        raw = make(block_size, version, old, relocated)
        path = OUTPUT / f'v{36 if version else 35}-{block_size}{"-old" if old else ""}{"-relocated" if relocated else ""}.zlib'
        path.write_bytes(zlib.compress(raw, 9))
        manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
            'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'CC0-1.0', 'source': 'scripts/reiserfs-fixtures.py'})
    path = OUTPUT / 'reiserfsprogs.img.zst'
    # zstandard is needed only to verify the retained external image's hash.
    import zstandard
    raw = zstandard.ZstdDecompressor().decompress(path.read_bytes())
    manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
        'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'AGPL-3.0-only',
        'source': 'https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/tests/fixtures/test_reiserfs_v3_6.img.zst'})
    (OUTPUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
