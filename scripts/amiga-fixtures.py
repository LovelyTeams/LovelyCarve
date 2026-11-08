#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Deterministic OFS/FFS fixtures made by independent amitools 0.8.1.

Install amitools or put its source on PYTHONPATH. No Rust formatter is used.
Only invented names and deterministic byte patterns are written to the images.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib
from amitools.fs.ADFSVolume import ADFSVolume
from amitools.fs.FSString import FSString
from amitools.fs.FileName import FileName
from amitools.fs.MetaInfo import MetaInfo
from amitools.fs.RootMetaInfo import RootMetaInfo
from amitools.fs.TimeStamp import TimeStamp
from amitools.fs.blkdev.BlockDevice import BlockDevice
from amitools.fs.rdb.RDisk import RDisk

OUTPUT = Path(__file__).resolve().parent.parent / 'vendor/amiga-reader/tests/fixtures'
STAMP = TimeStamp(12345, 678, 2999)


class Memory(BlockDevice):
    def __init__(self, blocks=1760, block_size=512, reserved=2):
        self._set_geometry(cyls=1, heads=1, sectors=blocks, block_bytes=block_size, reserved=reserved)
        self.data = bytearray(self.num_bytes)

    def read_block(self, number, num_blks=1):
        assert 0 <= number and number + num_blks <= self.num_blocks
        return bytes(self.data[number * self.block_bytes:(number + num_blks) * self.block_bytes])

    def write_block(self, number, data, num_blks=1):
        assert 0 <= number and number + num_blks <= self.num_blocks and len(data) == self.block_bytes * num_blks
        self.data[number * self.block_bytes:(number + num_blks) * self.block_bytes] = data


def create(variant, block_size=512, blocks=1760, links=False):
    medium = Memory(blocks, block_size)
    populate(medium, variant, links=links)
    return medium.data


def populate(medium, variant, links=False):
    block_size = medium.block_bytes
    volume = ADFSVolume(medium)
    volume.create(FSString('Fixture'), RootMetaInfo(STAMP, STAMP, STAMP), dos_type=0x444f5300 + variant)
    root = volume.get_root_dir()
    metadata = MetaInfo(protect=0x55, mod_ts=STAMP, comment=FSString('evidence comment'))
    body = bytes((index * 17 + 5) & 255 for index in range(200013))
    big = root.create_file(FSString('Café.bin'), body, metadata, update_ts=False)
    sub = root.create_dir(FSString('sub'), metadata, update_ts=False)
    sub.create_file(FSString('nested.txt'), b'nested evidence', metadata, update_ts=False)
    root.create_file(FSString('empty'), b'', metadata, update_ts=False)
    root.create_file(FSString('back\\slash'), b'Unix-safe evidence name', metadata, update_ts=False)
    if variant >= 6:
        long_metadata = MetaInfo(protect=0x55, mod_ts=STAMP, comment=FSString('LN'))
        short_long_name = 'é' * 48 + '.bin'
        long_node = root.create_file(FSString(short_long_name), b'long filename', long_metadata, update_ts=False)
    for index in range(100):
        sub.create_file(FSString(f'item_{index:03}'), f'item {index:03}'.encode(), metadata, update_ts=False)
    link_nodes = []
    if links:
        for name in ['alias.bin', 'softlink', 'dirlink']:
            link_nodes.append(root.create_file(FSString(name), b'', metadata, update_ts=False))
    volume.close()
    if variant >= 6:
        # amitools 0.8.1's LNFS writer repeats the name as the inline comment
        # (EntryBlock._write_nac_modts), overrunning the block for >55 bytes.
        # Extend the already valid 52-byte name with an independent minimal
        # NaC edit, keeping the same hash bucket and checking the result with
        # amitools' unmodified reader. This is not a patch to that reader.
        slots = block_size // 4 - 56
        old_hash = FileName(FSString(short_long_name), is_intl=True, is_longname=True).hash(slots)
        long_name = next('é' * 99 + f'{candidate:04x}.bin' for candidate in range(65536)
            if FileName(FSString('é' * 99 + f'{candidate:04x}.bin'), is_intl=True, is_longname=True).hash(slots) == old_hash)
        raw = bytearray(medium.read_block(long_node.block.blk_num))
        encoded = long_name.encode('latin1')
        raw[block_size - 184:block_size - 72] = (bytes([len(encoded)]) + encoded + b'\0').ljust(112, b'\0')
        struct.pack_into('>I', raw, 20, 0)
        words = struct.unpack('>' + 'I' * (block_size // 4), raw)
        struct.pack_into('>I', raw, 20, (-sum(words)) & 0xffffffff)
        medium.write_block(long_node.block.blk_num, raw)
    # Read using the independent implementation again, including extension
    # blocks, before retaining a fixture for the Rust reader.
    reopened = ADFSVolume(medium)
    reopened.open()
    assert reopened.valid
    assert reopened.read_file(FSString('Café.bin')) == body
    if variant >= 6:
        assert reopened.read_file(FSString(long_name)) == b'long filename'
    reopened.close()
    if links:
        # Independent field-level link fixtures over an amitools-created
        # allocation and hash layout. amitools 0.8.1 cannot read these link
        # entry types, so only the base files above are oracle-checked.
        def edit(node, changes, payload=None):
            raw = bytearray(medium.read_block(node.block.blk_num))
            for offset, value in changes.items():
                struct.pack_into('>I', raw, offset % block_size, value & 0xffffffff)
            if payload is not None:
                raw[24:block_size - 200] = (payload + b'\0').ljust(block_size - 224, b'\0')
            struct.pack_into('>I', raw, 20, 0)
            words = struct.unpack('>' + 'I' * (block_size // 4), raw)
            struct.pack_into('>I', raw, 20, (-sum(words)) & 0xffffffff)
            medium.write_block(node.block.blk_num, raw)
        alias, softlink, dirlink = link_nodes
        edit(alias, {-4: -4, -44: big.block.blk_num, -92: 12346})
        edit(big, {-40: alias.block.blk_num})
        edit(softlink, {-4: 3}, b'Work:Caf\xe9.bin')
        edit(dirlink, {-4: 4, -44: sub.block.blk_num})
        edit(sub, {-40: dirlink.block.blk_num})


def create_rdb():
    medium = Memory(100 * 2 * 63)
    medium._set_geometry(cyls=100, heads=2, sectors=63, block_bytes=512)
    disk = RDisk(medium)
    disk.create(medium.get_geometry())
    for name, low, high, variant, block_size in [('DH0', 1, 39, 3, 1024), ('DH1', 40, 70, 7, 4096),
            ('UNKNOWN', 71, 80, None, 512), ('DH3', 81, 99, 0, 512)]:
        disk.add_partition(FSString(name), (low, high), dos_type=0x53465300 if variant is None else 0x444f5300 + variant, fs_block_size=block_size)
        if variant is not None:
            part = disk.get_partition(len(disk.parts) - 1).create_blkdev()
            part.open()
            populate(part, variant)
    return medium.data


if __name__ == '__main__':
    OUTPUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    for variant, block_size, blocks in [(v, 512, 1760) for v in range(8)] + [(3, 1024, 4096), (7, 4096, 1760), (1, 512, 110000)]:
        raw = create(variant, block_size, blocks)
        path = OUTPUT / f'dos{variant}-{block_size}-{blocks}.zlib'
        path.write_bytes(zlib.compress(raw, 9))
        manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
            'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'CC0-1.0',
            'source': 'scripts/amiga-fixtures.py using amitools 0.8.1 (GPL-2.0-or-later oracle, not a runtime dependency)'})
    raw = create_rdb()
    path = OUTPUT / 'rdb-multiblock.zlib'
    path.write_bytes(zlib.compress(raw, 9))
    manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
        'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'CC0-1.0',
        'source': 'scripts/amiga-fixtures.py using amitools 0.8.1, four RDB partitions, mixed block sizes and unused track-sector tails'})
    raw = create(3, links=True)
    path = OUTPUT / 'links-dos3.zlib'
    path.write_bytes(zlib.compress(raw, 9))
    manifest.append({'file': path.name, 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
        'compressed_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'license': 'CC0-1.0',
        'source': 'scripts/amiga-fixtures.py, independent hard/softlink field edits over an amitools-created base'})
    (OUTPUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
