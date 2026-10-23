#!/usr/bin/env python3
"""CC0 invented ECMA-167 fixtures, encoded without using the Rust reader.

The public mkudffs/kernel images remain the independent positive oracle.
These small images exercise explicit geometry, sparse extents and corruption.
"""
import binascii
import hashlib
import json
from pathlib import Path
import struct
import zlib

OUTPUT = Path(__file__).resolve().parent.parent / 'src-tauri/vendor/lamfold-udf/tests/fixtures'


def fixture(block_size, extended):
    image = bytearray(640 * block_size)
    base = 300

    def put(block, data):
        image[block * block_size:block * block_size + len(data)] = data

    def tag(data, identifier, location, length):
        struct.pack_into('<HHBBHHHI', data, 0, identifier, 3 if extended else 2,
                         0, 0, 1, binascii.crc_hqx(data[16:length], 0), length - 16, location)
        data[4] = sum(data[:16]) & 255
        return data

    def domain():
        result = bytearray(32)
        result[1:20] = b'*OSTA UDF Compliant'
        struct.pack_into('<H', result, 24, 0x0201 if extended else 0x0102)
        return result

    def long_ad(block, length):
        return struct.pack('<IIHHI', length, block, 0, 0, 0)

    def allocation(block, length):
        return long_ad(block, length) if extended else struct.pack('<II', length, block)

    anchor = bytearray(block_size)
    struct.pack_into('<IIII', anchor, 16, 3 * block_size, 32, 3 * block_size, 48)
    for block in [256, 383, 639]:
        put(block, tag(anchor[:], 2, block, 512))

    pd = bytearray(block_size)
    struct.pack_into('<IHH', pd, 16, 1, 1, 7)
    pd[25:31] = b'+NSR03' if extended else b'+NSR02'
    struct.pack_into('<III', pd, 184, 1, base, 300)
    lvd = bytearray(block_size)
    struct.pack_into('<I', lvd, 16, 1)
    struct.pack_into('<I', lvd, 212, block_size)
    lvd[216:248] = domain()
    lvd[248:264] = long_ad(0, block_size)
    struct.pack_into('<II', lvd, 264, 6, 1)
    lvd[440:446] = struct.pack('<BBHH', 1, 6, 1, 7)
    for start in [32, 48]:
        put(start, tag(pd[:], 5, start, 512))
        put(start + 1, tag(lvd[:], 6, start + 1, 446))
        put(start + 2, tag(bytearray(block_size), 8, start + 2, 512))
    fsd = bytearray(block_size)
    fsd[400:416] = long_ad(1, block_size)
    fsd[416:448] = domain()
    put(base, tag(fsd, 256, 0, 512))

    def file_entry(block, kind, size, data, inline=False, unknown_time=False):
        header = 216 if extended else 176
        entry = bytearray(block_size)
        struct.pack_into('<H', entry, 20, 4)
        struct.pack_into('<H', entry, 24, 1)
        entry[27] = kind
        struct.pack_into('<H', entry, 34, 3 if inline else int(extended))
        struct.pack_into('<I', entry, 44, 0x1084)  # read for user, group, other
        struct.pack_into('<H', entry, 48, 1)
        struct.pack_into('<Q', entry, 56, size)
        if extended:
            struct.pack_into('<Q', entry, 64, size)
        time = struct.pack('<HH8B', 0x1801 if unknown_time else 0x11e0,
                           2024, 2, 29, 15, 30, 20, 12, 34, 56)
        start = 80 if extended else 72
        for offset in range(start, start + (48 if extended else 36), 12):
            entry[offset:offset + 12] = time
        struct.pack_into('<II', entry, header - 8, 0, len(data))
        entry[header:header + len(data)] = data
        assert header + len(data) <= block_size
        put(base + block, tag(entry, 266 if extended else 261, block, header + len(data)))

    file_entry(2, 5, 4 * block_size + 37,
               allocation(40, block_size) + allocation(60, 0xc0000000 | block_size))
    continuation = bytearray(block_size)
    more = (allocation(41, 0x40000000 | block_size)
            + allocation(0, 0x80000000 | block_size)
            + allocation(50, block_size + 37))
    struct.pack_into('<II', continuation, 16, 0, len(more))
    continuation[24:24 + len(more)] = more
    put(base + 60, tag(continuation, 258, 60, 24 + len(more)))
    put(base + 40, bytes((index * 17 + 3) & 255 for index in range(block_size)))
    put(base + 41, b'\xaa' * block_size)  # allocated but unrecorded: must read zero
    put(base + 50, bytes((index * 19 + 5) & 255 for index in range(block_size + 37)))
    file_entry(3, 5, 10, b'hello UDF\n', inline=True)
    target = bytes([3, 0, 0, 0, 5, 8, 0, 0, 8]) + b'big.bin'
    file_entry(5, 12, len(target), target, inline=True)
    file_entry(6, 5, 7, b'unknown', inline=True, unknown_time=True)
    file_entry(7, 5, 0x3ffff000, allocation(0, 0xbffff000))

    def fid(name, block, flags, location):
        name = (b'\x10' + name.encode('utf-16be')) if any(ord(char) > 255 for char in name) else (b'\x08' + name.encode('latin-1'))
        if flags & 8:
            name = b''
        length = 38 + len(name)
        record = bytearray((length + 3) & ~3)
        struct.pack_into('<HBB', record, 16, 1, flags, len(name))
        record[20:36] = long_ad(block, block_size)
        record[38:38 + len(name)] = name
        return tag(record, 257, location, length)

    sub = fid('', 1, 10, 4) + fid('nested.txt', 3, 0, 4)
    file_entry(4, 4, len(sub), sub, inline=True)
    directory = bytearray()
    for name, block, flags in [('', 1, 10), ('big.bin', 2, 0), ('alias.bin', 2, 0),
                               ('中文.txt', 3, 0), ('sub', 4, 2), ('link', 5, 0),
                               ('deleted', 299, 4), ('unknown.txt', 6, 0), ('huge.bin', 7, 0)]:
        directory += fid(name, block, flags, 20 + len(directory) // block_size)
    file_entry(1, 4, len(directory), allocation(20, len(directory)))
    put(base + 20, directory)
    return image


if __name__ == '__main__':
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for block_size, extended in [(512, True), (1024, False), (2048, False), (4096, True)]:
        image = fixture(block_size, extended)
        (OUTPUT / f'synthetic-{block_size}.zlib').write_bytes(zlib.compress(image, 9))
    revision = 'fabaad6af0c28b80535307b339b84110f3a3c12e'
    records = []
    for path in sorted(OUTPUT.glob('*.zlib')):
        stored = path.read_bytes()
        image = zlib.decompress(stored)
        upstream = path.stem.startswith('udf')
        records.append({
            'file': path.name,
            'sha256': hashlib.sha256(stored).hexdigest(),
            'logical_bytes': len(image),
            'logical_sha256': hashlib.sha256(image).hexdigest(),
            'source': (f'https://github.com/lamco-admin/lamfold/blob/{revision}/crates/lamfold-udf/tests/fixtures/{path.stem}.img'
                       if upstream else 'scripts/udf-fixtures.py, invented ECMA-167 fields'),
            'license': 'MIT OR Apache-2.0' if upstream else 'CC0-1.0',
            'generation': 'mkudffs and Linux UDF kernel; lossless zlib copy' if upstream else 'Python standard library, no filesystem reader',
        })
    (OUTPUT / 'manifest.json').write_text(json.dumps(records, indent=2) + '\n', encoding='utf-8')
