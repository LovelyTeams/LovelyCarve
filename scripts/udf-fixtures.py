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


def fixture(block_size, extended, metadata_revision=None, vat_revision=None, sparable=False):
    base = 320 if sparable else 300
    partition_length = 256 if sparable else 300
    image = bytearray((600 if vat_revision else 640) * block_size)
    tree_ref = int(metadata_revision == 0x0250 or vat_revision is not None)
    physical_ref = 1 - tree_ref if metadata_revision or vat_revision else 0
    directory_block = 31 if metadata_revision else 20
    aed_block = 59 if metadata_revision else 60

    def put(block, data):
        image[block * block_size:block * block_size + len(data)] = data

    def put_backing(block, data):
        for index in range(0, len(data), block_size):
            physical = block + index // block_size
            target = 608 + physical - 32 if sparable and 32 <= physical < 48 else base + physical
            put(target, data[index:index + block_size])

    def translate_vat(block):
        return block if 40 <= block <= 51 else 64 + block

    def put_tree(block, data):
        if vat_revision:
            for index in range(0, len(data), block_size):
                physical = translate_vat(block + index // block_size)
                put_backing(physical, data[index:index + block_size])
            return
        if metadata_revision is None:
            put_backing(block, data)
            return
        assert block + (len(data) + block_size - 1) // block_size <= 64
        for index in range(0, len(data), block_size):
            logical = block + index // block_size
            payload = data[index:index + block_size]
            primary = 64 + logical if logical < 32 else 128 + logical - 32
            put(base + primary, payload)
            put(base + 160 + logical, payload)

    def tag(data, identifier, location, length):
        struct.pack_into('<HHBBHHHI', data, 0, identifier, 3 if extended else 2,
                         0, 0, 1, binascii.crc_hqx(data[16:length], 0), length - 16, location)
        data[4] = sum(data[:16]) & 255
        return data

    def domain():
        result = bytearray(32)
        result[1:20] = b'*OSTA UDF Compliant'
        struct.pack_into('<H', result, 24, metadata_revision or vat_revision or (0x0201 if extended or sparable else 0x0102))
        return result

    def long_ad(block, length, reference=0):
        return struct.pack('<IIHHI', length, block, reference, 0, 0)

    def short_ad(block, length):
        return struct.pack('<II', length, block)

    def allocation(block, length, reference=physical_ref):
        return long_ad(block, length, reference) if extended else short_ad(block, length)

    anchor = bytearray(block_size)
    struct.pack_into('<IIII', anchor, 16, 3 * block_size, 32, 3 * block_size, 48)
    for block in [256] if vat_revision else [256, 383, 639]:
        put(block, tag(anchor[:], 2, block, 512))

    pd = bytearray(block_size)
    struct.pack_into('<IHH', pd, 16, 1, 1, 7)
    pd[25:31] = b'+NSR03' if extended or sparable else b'+NSR02'
    struct.pack_into('<III', pd, 184, 1, base, partition_length)
    lvd = bytearray(block_size)
    struct.pack_into('<I', lvd, 16, 1)
    struct.pack_into('<I', lvd, 212, block_size)
    lvd[216:248] = domain()
    lvd[248:264] = long_ad(0, block_size, tree_ref)
    physical = struct.pack('<BBHH', 1, 6, 1, 7)
    maps = [physical]
    if metadata_revision:
        metadata = bytearray(64)
        metadata[:4] = bytes([2, 64, 0, 0])
        metadata[5:28] = b'*UDF Metadata Partition'
        struct.pack_into('<H', metadata, 28, metadata_revision)
        struct.pack_into('<HHIIIIHB', metadata, 36, 1, 7, 10, 11, 0xffffffff, 32, 32, 1)
        maps.insert(tree_ref, metadata)
    if vat_revision:
        virtual = bytearray(64)
        virtual[:2] = bytes([2, 64])
        virtual[5:27] = b'*UDF Virtual Partition'
        struct.pack_into('<H', virtual, 28, vat_revision)
        struct.pack_into('<HH', virtual, 36, 1, 7)
        maps.append(virtual)
    if sparable:
        sparing = bytearray(64)
        sparing[:2] = bytes([2, 64])
        sparing[5:28] = b'*UDF Sparable Partition'
        struct.pack_into('<H', sparing, 28, 0x0201)
        struct.pack_into('<HHHBBIII', sparing, 36, 1, 7, 16, 2, 0, 64, 80, 96)
        maps = [sparing]
        for sequence, location in enumerate([80, 96], 1):
            copy = bytearray(block_size)
            copy[17:35] = b'*UDF Sparing Table'
            struct.pack_into('<HHIII', copy, 48, 1, 0, sequence, 32, 608)
            put(location, tag(copy, 0, location, 64))
    table = b''.join(maps)
    struct.pack_into('<II', lvd, 264, len(table), len(maps))
    lvd[440:440 + len(table)] = table
    for start in [32, 48]:
        put(start, tag(pd[:], 5, start, 512))
        put(start + 1, tag(lvd[:], 6, start + 1, 440 + len(table)))
        put(start + 2, tag(bytearray(block_size), 8, start + 2, 512))
    fsd = bytearray(block_size)
    fsd[400:416] = long_ad(1, block_size, tree_ref)
    fsd[416:448] = domain()
    put_tree(0, tag(fsd, 256, 0, 512))

    def file_entry(block, kind, size, data, inline=False, unknown_time=False, short=False, physical=False):
        header = 216 if extended else 176
        entry = bytearray(block_size)
        struct.pack_into('<H', entry, 20, 4)
        struct.pack_into('<H', entry, 24, 1)
        entry[27] = kind
        struct.pack_into('<H', entry, 34, 3 if inline else int(extended and not short))
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
        tag(entry, 266 if extended else 261, block, header + len(data))
        if physical:
            put_backing(block, entry)
        else:
            put_tree(block, entry)

    if metadata_revision:
        file_entry(10, 250, 64 * block_size, short_ad(64, 32 * block_size)
                   + short_ad(128, 32 * block_size), short=True, physical=True)
        file_entry(11, 251, 64 * block_size, short_ad(160, 64 * block_size),
                   short=True, physical=True)

    if vat_revision:
        entries = b''.join(struct.pack('<I', translate_vat(block)) for block in range(64))
        if vat_revision == 0x0150:
            tail = bytearray(36)
            tail[1:23] = b'*UDF Virtual Alloc Tbl'
            struct.pack_into('<I', tail, 32, 0xffffffff)
            vat = entries + tail
            file_entry(partition_length - 1, 0, len(vat), vat, inline=True, physical=True)
        else:
            header = bytearray(152)
            struct.pack_into('<HH', header, 0, len(header), 0)
            struct.pack_into('<I', header, 132, 0xffffffff)
            struct.pack_into('<HHH', header, 144, 0x0201, 0x0201, 0x0201)
            vat = header + entries
            put_backing(200, vat)
            file_entry(partition_length - 1, 248, len(vat), short_ad(200, len(vat)),
                       short=True, physical=True)

    last_extent = 47 if sparable else 50
    file_entry(2, 5, 4 * block_size + 37,
               allocation(40, block_size) + allocation(aed_block, 0xc0000000 | block_size, tree_ref))
    continuation = bytearray(block_size)
    more = (allocation(41, 0x40000000 | block_size)
            + allocation(0, 0x80000000 | block_size)
            + allocation(last_extent, block_size + 37))
    struct.pack_into('<II', continuation, 16, 0, len(more))
    continuation[24:24 + len(more)] = more
    put_tree(aed_block, tag(continuation, 258, aed_block, 24 + len(more)))
    put_backing(40, bytes((index * 17 + 3) & 255 for index in range(block_size)))
    put_backing(41, b'\xaa' * block_size)  # allocated but unrecorded: must read zero
    put_backing(last_extent, bytes((index * 19 + 5) & 255 for index in range(block_size + 37)))
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
        record[20:36] = long_ad(block, block_size, tree_ref)
        record[38:38 + len(name)] = name
        return tag(record, 257, location, length)

    sub = fid('', 1, 10, 4) + fid('nested.txt', 3, 0, 4)
    file_entry(4, 4, len(sub), sub, inline=True)
    directory = bytearray()
    names = [('', 1, 10), ('big.bin', 2, 0), ('alias.bin', 2, 0),
             ('中文.txt', 3, 0), ('sub', 4, 2), ('link', 5, 0),
             ('deleted', 299, 4), ('unknown.txt', 6, 0), ('huge.bin', 7, 0)]
    if metadata_revision:
        names += [('discarded' * 7, 63, 4)] * 25 + [('late.txt', 3, 0)]
    for name, block, flags in names:
        directory += fid(name, block, flags, directory_block + len(directory) // block_size)
    allocation_descriptor = (short_ad(directory_block, len(directory)) if metadata_revision
                             else allocation(directory_block, len(directory), tree_ref))
    file_entry(1, 4, len(directory), allocation_descriptor, short=bool(metadata_revision))
    put_tree(directory_block, directory)
    return image


if __name__ == '__main__':
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for block_size, extended in [(512, True), (1024, False), (2048, False), (4096, True)]:
        image = fixture(block_size, extended)
        (OUTPUT / f'synthetic-{block_size}.zlib').write_bytes(zlib.compress(image, 9))
    for revision in [0x0250, 0x0260]:
        image = fixture(2048, True, revision)
        (OUTPUT / f'metadata-{revision:x}.zlib').write_bytes(zlib.compress(image, 9))
    for revision in [0x0150, 0x0201]:
        image = fixture(2048, revision >= 0x0200, vat_revision=revision)
        (OUTPUT / f'vat-{revision:x}.zlib').write_bytes(zlib.compress(image, 9))
    (OUTPUT / 'sparable.zlib').write_bytes(zlib.compress(fixture(2048, True, sparable=True), 9))
    revision = 'fabaad6af0c28b80535307b339b84110f3a3c12e'
    records = []
    mkudffs_source = ('https://archive.ubuntu.com/ubuntu/pool/universe/u/udftools/'
                      'udftools_2.3-1_amd64.deb')
    mkudffs_commands = {
        'mkudffs-vat150': 'mkudffs --new-file --media-type=cdr --udfrev=1.50 --vat --label=VAT150 vat150.img 2048',
        'mkudffs-vat201': 'mkudffs --new-file --media-type=cdr --udfrev=2.01 --vat --label=VAT201 vat201.img 2048',
        'mkudffs-sparable': 'mkudffs --new-file --media-type=cdrw --udfrev=2.01 --spartable=2 --label=SPARABLE sparable.img 4096',
    }
    for path in sorted(OUTPUT.glob('*.zlib')):
        stored = path.read_bytes()
        image = zlib.decompress(stored)
        source = 'scripts/udf-fixtures.py, invented ECMA-167 fields'
        license_id = 'CC0-1.0'
        generation = 'Python standard library, no filesystem reader'
        if path.stem.startswith('udf'):
            source = (f'https://github.com/lamco-admin/lamfold/blob/{revision}'
                      f'/crates/lamfold-udf/tests/fixtures/{path.stem}.img')
            license_id = 'MIT OR Apache-2.0'
            generation = 'mkudffs and Linux UDF kernel; lossless zlib copy'
        elif path.stem in mkudffs_commands:
            source = mkudffs_source
            generation = mkudffs_commands[path.stem] + '; lossless zlib copy'
        records.append({
            'file': path.name,
            'sha256': hashlib.sha256(stored).hexdigest(),
            'logical_bytes': len(image),
            'logical_sha256': hashlib.sha256(image).hexdigest(),
            'source': source,
            'license': license_id,
            'generation': generation,
        })
    (OUTPUT / 'manifest.json').write_text(json.dumps(records, indent=2) + '\n', encoding='utf-8')
