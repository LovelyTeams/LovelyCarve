#!/usr/bin/env python3
"""CC0-1.0: independently packed bcachefs 1.39 test image, no formatter.

The prose format specification and the MIT reader's documented byte layout
define the fields. Deliberately tiny trees expose corruption offsets; native
formatter / mounted-image audits remain independent tests.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib

MAGIC = bytes.fromhex('c68573f666ce90a9d96a60cf803df7ef')
UUID = bytes.fromhex('112233445566778899aabbccddeeff00')
MAXIMUM = (1 << 64) - 1
SNAPSHOT = (1 << 32) - 1
IMAGE_SIZE = 8 * 1024 * 1024
ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'vendor/bcachefs-reader/tests/fixtures'


def crc(data, initial=0xffffffff, final=0xffffffff):
    value = initial
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = (value >> 1) ^ (0x82f63b78 if value & 1 else 0)
    return value ^ final


def pad(data):
    return data + bytes((-len(data)) % 8)


def position(inode=0, offset=0, snapshot=0):
    return struct.pack('<IQQ', snapshot, offset, inode)


def key(kind, inode, offset, value=b'', size=0, snapshot=SNAPSHOT):
    value = pad(value)
    return (bytes([5 + len(value) // 8, 1, kind, 0]) + bytes(12)
            + struct.pack('<I', size) + position(inode, offset, snapshot) + value)


def varint(value):
    value &= MAXIMUM
    for length in range(1, 9):
        if value < 1 << (7 * length):
            return ((value << length) | ((1 << (length - 1)) - 1)).to_bytes(length, 'little')
    return b'\xff' + struct.pack('<Q', value)


def inode(number, mode, size=0, snapshot=SNAPSHOT):
    # Times are signed nanosecond deltas from the superblock's 2020 base.
    times = [(-315619200 * 10**9 + 123456789) - 1577836800 * 10**9,
             1700000000 * 10**9 + 987654321 - 1577836800 * 10**9,
             1600000000 * 10**9 + 456789123 - 1577836800 * 10**9,
             1500000000 * 10**9 + 111222333 - 1577836800 * 10**9]
    fields = [item for time in times for item in [time, 0]] + [1000, 1001, 2, 7, 0]
    flags = 9 << 24 | mode << 36
    value = struct.pack('<QQQQQQ', 0, 0, flags, (size + 511) // 512, size, 0)
    return key(29, 0, number, value + b''.join(varint(item) for item in fields), snapshot=snapshot)


def dirent(directory, name, target, kind, snapshot=SNAPSHOT):
    name = name.encode('utf-8')
    slot = crc(bytes(8) + name, final=0)
    return key(10, directory, slot, struct.pack('<QB', target, kind) + name, snapshot=snapshot)


def pointer(sector, generation=3, flags=0):
    return struct.pack('<Q', 1 | flags << 1 | sector << 4 | generation << 56)


def node(tree, keys):
    keys.sort(key=lambda item: (struct.unpack_from('<Q', item, 32)[0],
                                struct.unpack_from('<Q', item, 24)[0],
                                struct.unpack_from('<I', item, 20)[0]))
    body = b''.join(keys)
    header = bytearray(160)
    struct.pack_into('<QQ', header, 16, 0x90135c78b99e07f5 ^ int.from_bytes(UUID[:8], 'little'), tree)
    header[52:72] = position(MAXIMUM, MAXIMUM, SNAPSHOT)
    header[80:82] = bytes([5, 6])
    struct.pack_into('<QQIHH', header, 136, 7, 10, 1, 0, len(body) // 8)
    data = header + body
    struct.pack_into('<I', data, 0, crc(data[16:]))
    assert len(data) <= 32768
    return data + bytes(32768 - len(data))


def field(kind, data):
    data = pad(data)
    return struct.pack('<II', 1 + len(data) // 8, kind) + data


def build():
    image = bytearray(IMAGE_SIZE)
    content = b'Pure Rust bcachefs evidence\n'
    tree = {index: [] for index in [0, 1, 2, 4, 7, 8, 9]}
    for number, mode, size in [(4096, 0o40755, 0), (5000, 0o100640, len(content)),
                               (5001, 0o100600, 5 * 1024**3 + 37),
                               (5002, 0o120777, 9), (5003, 0o10644, 0),
                               (5004, 0o100644, 513), (5005, 0o100644, 513)]:
        tree[1].append(inode(number, mode, size))
    for name, number, kind in [('hello.txt', 5000, 8), ('alias.txt', 5000, 8), ('sparse.bin', 5001, 8),
                                ('link', 5002, 10), ('pipe', 5003, 1), ('checksum.bin', 5004, 8),
                                ('reflink.bin', 5005, 8), ('中文\\证据.txt', 5000, 8), ('sv', 2, 16), ('snap', 3, 16)]:
        tree[2].append(dirent(4096, name, number, kind))
    tree[0].append(key(17, 5000, 1, content, size=1))
    tree[0].append(key(17, 5002, 1, b'hello.txt', size=1))
    # One allocated last sector after a >5 GiB hole; another is unwritten.
    last = (5 * 1024**3) // 512
    tree[0].append(key(6, 5001, last + 1, pointer(4096), size=1))
    tree[0].append(key(6, 5001, 2048, pointer(5000, flags=4), size=2048))
    tail = b'TAIL!' + bytes(507)
    image[4096*512:4097*512] = tail
    checked = bytes((index * 17 + 3) % 256 for index in range(513)) + bytes(511)
    image[4100*512:4102*512] = checked
    checksum_word = 2 | 1 << 2 | 1 << 9 | 5 << 24 | crc(checked, initial=0, final=0) << 32
    checked_entries = struct.pack('<Q', checksum_word) + pointer(4100)
    tree[0].append(key(6, 5004, 2, checked_entries, size=2))
    tree[0].append(key(15, 5005, 2, struct.pack('<QQ', 100, 0), size=2))
    tree[7].append(key(16, 0, 102, struct.pack('<Q', 1) + checked_entries, size=2, snapshot=0))
    for bucket in [16, 19]:
        allocated = bytearray(32); allocated[12] = 3
        tree[4].append(key(27, 0, bucket, allocated, snapshot=0))
    for subvolume, snapshot, root in [(1, SNAPSHOT, 4096), (2, SNAPSHOT-1, 6000), (3, SNAPSHOT-2, 6000)]:
        tree[8].append(key(21, 0, subvolume, struct.pack('<QQ', snapshot << 32, root), snapshot=0))
        parent = 0 if snapshot == SNAPSHOT else SNAPSHOT
        tree[9].append(key(22, 0, snapshot, struct.pack('<Q', parent << 32), snapshot=0))
        if subvolume != 1:
            tree[1].extend([inode(6000, 0o40755, snapshot=snapshot), inode(6001, 0o100644, 4, snapshot=snapshot)])
            tree[2].append(dirent(6000, 'version.txt', 6001, 8, snapshot))
            tree[0].append(key(17, 6001, 1, b'LIVE' if subvolume==2 else b'PAST', size=1, snapshot=snapshot))
    roots = bytearray(16)
    for index, (tree_id, keys) in enumerate(tree.items()):
        sector = 1024 + index * 64
        image[sector*512:(sector+64)*512] = node(tree_id, keys)
        sectors_written = (160 + sum(len(entry) for entry in keys) + 511) // 512
        ptr = struct.pack('<QQHH', 0, 7, sectors_written, 0) + position() + pointer(sector)
        root = key(18, MAXIMUM, MAXIMUM, ptr, snapshot=SNAPSHOT)
        roots += struct.pack('<HBBB3x', len(root)//8, tree_id, 0, 1) + root
    members = bytearray(56)
    struct.pack_into('<H', members, 0, 48)
    members[8:24] = UUID
    struct.pack_into('<QHH', members, 24, 64, 2, 256)
    fields = field(11, members) + field(6, roots)
    header = bytearray(752)
    struct.pack_into('<HH', header, 16, 1063, 1060)
    header[24:40] = MAGIC; header[40:56] = UUID; header[56:72] = UUID
    header[72:80] = b'INVENTED'
    struct.pack_into('<QQHBBI', header, 104, 8, 10, 1, 0, 1, len(fields)//8)
    struct.pack_into('<QIIQ', header, 128, 1577836800*10**9, 0, 1, 2 | 1<<2 | 64<<12)
    layout = bytearray(512); layout[:16] = MAGIC; layout[17:19] = bytes([11, 2]); struct.pack_into('<QQ', layout, 24, 8, 2056)
    header[240:752] = layout; image[3584:4096] = layout
    for sector in [8, 2056]:
        copy = header + fields
        struct.pack_into('<Q', copy, 104, sector)
        struct.pack_into('<I', copy, 0, crc(copy[16:]))
        image[sector*512:sector*512+len(copy)] = copy
    return bytes(image)


if __name__ == '__main__':
    image = build()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / 'invented.zlib').write_bytes(zlib.compress(image, 9))
    (OUTPUT / 'manifest.json').write_text(json.dumps({'license':'CC0-1.0', 'size':len(image), 'sha256':hashlib.sha256(image).hexdigest(), 'generation':'scripts/bcachefs-fixtures.py'}, indent=2)+'\n', encoding='utf-8')
    print('Created independent bcachefs image:', len(image), 'bytes')
