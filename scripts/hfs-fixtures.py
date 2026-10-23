#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Encode invented HFS fixtures independently of the Rust reader (no HFS writer).

Two allocation sizes, fragmented catalog and self-overflow B-trees, both file
forks, empty/resource-only files, threads and MacRoman/host-special names.
The separately retained hfsutils image is an external implementation oracle.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib

OUTPUT = Path(__file__).resolve().parent.parent / 'src-tauri/vendor/classic-hfs/tests/fixtures'


def put16(data, offset, value):
    struct.pack_into('>H', data, offset, value)


def put32(data, offset, value):
    struct.pack_into('>I', data, offset, value)


def extent_record(extents):
    return b''.join(struct.pack('>HH', *extent) for extent in extents + [(0, 0)] * (3 - len(extents)))


def node(kind, height, records, previous=0, following=0):
    data = bytearray(512)
    struct.pack_into('>IIBBHH', data, 0, following, previous, kind, height, len(records), 0)
    offset = 14
    for index, record in enumerate(records):
        put16(data, 510 - index * 2, offset)
        data[offset:offset + len(record)] = record
        offset += len(record)
    assert offset <= 512 - 2 * (len(records) + 1)
    put16(data, 510 - len(records) * 2, offset)
    return data


def tree_header(total, used, records, first, last, depth, maxkey):
    header = bytearray(106)
    struct.pack_into('>HIIIIHHII', header, 0, depth, 1 if records else 0, records, first, last, 512, maxkey, total, total - used)
    bitmap = bytearray(256)
    for index in range(used):
        bitmap[index // 8] |= 0x80 >> (index % 8)
    return node(1, 0, [header, bytes(128), bitmap])


def key(parent, name):
    raw = struct.pack('>BIB', 0, parent, len(name)) + name
    return bytes([len(raw)]) + raw + bytes((len(raw) + 1) % 2)


def file_record(cnid, data_size=0, data_extents=None, resource_size=0, resource_extents=None, block_size=1024):
    data = bytearray(102)
    data[0] = 2
    data[2] = 0x80  # used catalog record
    data[4:12] = b'TEXTttxt'
    put32(data, 20, cnid)
    for offset, size, ext in [(26, data_size, data_extents or []), (36, resource_size, resource_extents or [])]:
        put32(data, offset, size)
        put32(data, offset + 4, ((size + block_size - 1) // block_size) * block_size)
        data[74 if offset == 26 else 86:(74 if offset == 26 else 86) + 12] = extent_record(ext)
    for offset in [44, 48, 52]:
        put32(data, offset, 3_800_000_000 + offset)
    return data


def directory(cnid, valence):
    data = bytearray(70)
    data[0] = 1
    put16(data, 4, valence)
    put32(data, 6, cnid)
    for offset in [10, 14, 18]:
        put32(data, offset, 3_800_000_000 + offset)
    return data


def thread(parent, name, directory=True):
    data = bytearray(46)
    data[0] = 3 if directory else 4
    put32(data, 10, parent)
    data[14] = len(name)
    data[15:15 + len(name)] = name
    return data


def make(block_size):
    data = bytearray(2048 + 256 * block_size + 1024)
    mdb = memoryview(data)[1024:1536]
    put16(mdb, 0, 0x4244)
    put32(mdb, 2, 3_800_000_000)
    put32(mdb, 6, 3_800_000_001)
    put16(mdb, 10, 0x0100)
    put16(mdb, 14, 3)
    put16(mdb, 18, 256)
    put32(mdb, 20, block_size)
    put32(mdb, 24, block_size)
    put16(mdb, 28, 4)
    put32(mdb, 30, 25)
    name = b'Rust HFS'
    mdb[36] = len(name)
    mdb[37:37 + len(name)] = name
    extra_count = 3 if block_size == 512 else 1
    cat_extra = [(44, extra_count), (48, extra_count), (52, extra_count)]
    xt_size, cat_size = 4 * block_size, (3 + extra_count * 3) * block_size
    put32(mdb, 130, xt_size)
    mdb[134:146] = extent_record([(0, 1), (2, 1), (4, 1)])
    put32(mdb, 146, cat_size)
    mdb[150:162] = extent_record([(8, 1), (10, 1), (12, 1)])
    used_blocks = set()

    def write_fork(content, extents):
        cursor = 0
        for start, count in extents:
            used_blocks.update(range(start, start + count))
            amount = min(count * block_size, len(content) - cursor)
            location = 2048 + start * block_size
            data[location:location + amount] = content[cursor:cursor + amount]
            cursor += amount
        assert cursor == len(content)

    # xtree's final block and catalog's last three blocks need overflow records.
    overflow = [(3, 0, 3, [(6, 1)]), (4, 0, 3, cat_extra),
                (16, 0, 3, [(22, 1)]), (16, 255, 3, [(30, 1)])]
    records = [struct.pack('>BBIH', 7, kind, cnid, start) + extent_record(ext) for cnid, kind, start, ext in overflow]
    xtree = bytearray(xt_size)
    xtree[:512] = tree_header(xt_size // 512, 2, 4, 1, 1, 1, 7)
    xtree[512:1024] = node(255, 1, records)
    write_fork(xtree, [(0, 1), (2, 1), (4, 1), (6, 1)])
    args = {'block_size': block_size}
    catalog = [(1, name, directory(2, 8)), (2, b'', thread(1, name)),
               (2, b'.', file_record(23, **args)), (2, b'..', file_record(24, **args)),
               (2, b'Caf\x8e.bin', file_record(16, 3 * block_size + 37, [(16, 1), (18, 1), (20, 1)],
                                             3 * block_size + 19, [(24, 1), (26, 1), (28, 1)], **args)),
               (2, b'a/b', file_record(21, 5, [(36, 1)], **args)),
               (2, b'a\\b', file_record(22, 6, [(38, 1)], **args)),
               (2, b'empty', file_record(17, **args)),
               (2, b'only-rsrc', file_record(18, resource_size=5, resource_extents=[(32, 1)], **args)),
               (2, b'sub', directory(19, 1)), (16, b'', thread(2, b'Caf\x8e.bin', False)),
               (19, b'', thread(2, b'sub')), (19, b'nested.txt', file_record(20, 13, [(34, 1)], **args))]
    # ASCII punctuation precedes letters in HFS collation; the single accent
    # name has no competing base name, so this fixture needs no Rust sort table.
    catalog.sort(key=lambda item: (item[0], item[1].lower()))
    leaves = [catalog[index:index + 2] for index in range(0, len(catalog), 2)]
    ctree = bytearray(cat_size)
    ctree[:512] = tree_header(cat_size // 512, 2 + len(leaves), len(catalog), 2, 1 + len(leaves), 2, 37)
    index_records = []
    for index, entries in enumerate(leaves):
        number = index + 2
        parent, name, _ = entries[0]
        raw_key = struct.pack('>BIB', 0, parent, len(name)) + name
        index_records.append(bytes([37]) + raw_key + bytes(37 - len(raw_key)) + struct.pack('>I', number))
        ctree[number * 512:(number + 1) * 512] = node(255, 1, [key(parent, name) + record for parent, name, record in entries],
                                                   number - 1 if index else 0, number + 1 if index + 1 < len(leaves) else 0)
    ctree[512:1024] = node(0, 2, index_records)
    write_fork(ctree, [(8, 1), (10, 1), (12, 1)] + cat_extra)
    write_fork(bytes((index * 17 + 3) & 255 for index in range(3 * block_size + 37)), [(16, 1), (18, 1), (20, 1), (22, 1)])
    write_fork(bytes((index * 23 + 5) & 255 for index in range(3 * block_size + 19)), [(24, 1), (26, 1), (28, 1), (30, 1)])
    write_fork(b'RSRC!', [(32, 1)])
    write_fork(b'nested bytes!', [(34, 1)])
    write_fork(b'slash', [(36, 1)])
    write_fork(b'backsl', [(38, 1)])
    for block in used_blocks:
        data[1536 + block // 8] |= 0x80 >> (block % 8)
    put16(mdb, 34, 256 - len(used_blocks))
    data[-1024:-512] = mdb
    return data


if __name__ == '__main__':
    OUTPUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    for block_size in [512, 1024, 4096]:
        content = make(block_size)
        path = OUTPUT / f'fragmented-{block_size}.zlib'
        path.write_bytes(zlib.compress(content, 9))
        manifest.append({'file': path.name, 'bytes': len(content), 'sha256': hashlib.sha256(content).hexdigest(),
                         'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'source': 'scripts/hfs-fixtures.py', 'license': 'CC0-1.0'})
    path = OUTPUT / 'hfsutils.img.zst'
    manifest.append({'file': path.name, 'bytes': 819200, 'sha256': 'ceb7029adeb408de1c8dc4da0f887e825a0bbc7f139b017bbf277d0f0b1f8e07', 'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                     'source': 'https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/tests/fixtures/test_hfs.img.zst', 'license': 'AGPL-3.0-only'})
    (OUTPUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
