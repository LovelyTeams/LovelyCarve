"""Deterministic, invented APFS structures; no source evidence or third-party data.

Run from any directory with Python 3. Files are deliberately tiny reader fixtures,
not mountable macOS filesystems (no allocator/extent-reference trees).
"""
from pathlib import Path
import struct

BLOCK = 4096
COUNT = 16


def put(block, offset, fmt, *values):
    struct.pack_into('<' + fmt, block, offset, *values)


def checksum(block):
    first = second = 0
    for (word,) in struct.iter_unpack('<I', block[8:]):
        first = (first + word) % 0xffffffff
        second = (second + first) % 0xffffffff
    check1 = 0xffffffff - (first + second) % 0xffffffff
    check2 = 0xffffffff - (first + check1) % 0xffffffff
    put(block, 0, 'II', check1, check2)
    return block


def object_block(oid, kind, xid=10):
    block = bytearray(BLOCK)
    put(block, 8, 'QQII', oid, xid, kind, 0)
    return block


def nx(xid):
    block = object_block(1, 0x80000001, xid)
    block[32:36] = b'NXSB'
    put(block, 36, 'IQ', BLOCK, COUNT)
    put(block, 64, 'Q', 2)
    block[72:88] = bytes(range(16))
    put(block, 104, 'IIQQ', 1, 0, 1, 0)
    put(block, 160, 'Q', 2)
    put(block, 180, 'I', 100)
    put(block, 184, 'QQQ', 100, 0, 101)
    return checksum(block)


def omap(oid, root):
    block = object_block(oid, 0x4000000b)
    put(block, 48, 'Q', root)
    return checksum(block)


def tree(oid, records, fixed=False):
    block = object_block(oid, 0x40000002 if fixed else 2)
    width = 4 if fixed else 8
    put(block, 32, 'HHIHH', 3 | (4 if fixed else 0), 0, len(records), 0, width * len(records))
    key_base = 56 + width * len(records)
    key_used = value_used = 0
    value_end = BLOCK - 40
    for index, (key, value) in enumerate(records):
        value_used += len(value)
        if fixed:
            put(block, 56 + index * width, 'HH', key_used, value_used)
        else:
            put(block, 56 + index * width, 'HHHH', key_used, len(key), value_used, len(value))
        block[key_base + key_used:key_base + key_used + len(key)] = key
        block[value_end - value_used:value_end - value_used + len(value)] = value
        key_used += len(key)
    assert key_base + key_used <= value_end - value_used
    put(block, value_end, 'IIIIIIQQ', 0, BLOCK, 16 if fixed else 0, 16 if fixed else 0, 0, 0, len(records), 1)
    return checksum(block)


def mapping(oid, xid, physical, flags=0):
    return struct.pack('<QQ', oid, xid), struct.pack('<IIQ', flags, BLOCK, physical)


def volume(oid, hashed, locked=False):
    block = object_block(oid, 13)
    block[32:36] = b'APSB'
    put(block, 56, 'Q', 9 if hashed else 0)
    put(block, 88, 'Q', 10)
    put(block, 116, 'IIIQQ', 2, 2, 2, 7, 200)
    put(block, 124, 'I', 0x40000002)
    put(block, 152, 'Q', 15)
    put(block, 216, 'Q', 1)
    put(block, 176, 'QQQQQ', 1000, 5, 1, 1, 0)
    block[240:256] = bytes(range(16, 32))
    put(block, 264, 'Q', 0 if locked else 1)
    block[704:712] = b'TestVol\0'
    put(block, 964, 'H', 64)
    block[1008:1024] = bytes(range(32, 48))
    return checksum(block)


def key(oid, kind, suffix=b''):
    return struct.pack('<Q', oid | kind << 60) + suffix


def inode(private, size, mode=0o100644, compressed=False):
    value = bytearray(92)
    put(value, 0, 'QQqqqqQiIIIIIHHQ', 2, private, -1, 123456789, 234567890, 345678901,
        0, 1, 0, 0, 0x20 if compressed else 0, 501, 20, mode, 0, size)
    value += struct.pack('<HHBBHQQQQQ', 1, 40, 8, 0, 40, size, 8192, 0, 0, 0)
    return bytes(value)


def attr(name, data, stream=False):
    suffix = struct.pack('<H', len(name) + 1) + name.encode() + b'\0'
    value = struct.pack('<HH', 1 if stream else 2, len(data)) + data
    return suffix, value


def catalog(hashed, label='plain.bin'):
    names = [(label, 20, 8), ('clone.bin', 21, 8), ('compressed.txt', 22, 8), ('link', 23, 10), ('hole.bin', 24, 8)]
    records = [(key(2, 3), inode(2, 0, 0o040755))]
    for name, oid, kind in names:
        encoded = name.encode() + b'\0'
        suffix = struct.pack('<I' if hashed else '<H', len(encoded)) + encoded
        records.append((key(2, 9, suffix), struct.pack('<QqH', oid, 987654321, kind)))
    for oid, private, size, mode, compressed in [(20, 70, 5000, 0o100644, False), (21, 70, 5000, 0o100644, False),
            (22, 72, 0, 0o100644, True), (23, 73, 0, 0o120777, False), (24, 74, 8192, 0o100644, False)]:
        records.append((key(oid, 3), inode(private, size, mode, compressed)))
    for oid, name, data, stream in [(20, 'user.small', b'value', False), (20, 'user.stream', struct.pack('<QQQQQQ', 80, 6, BLOCK, 0, 0, 0), True),
            (22, 'com.apple.decmpfs', struct.pack('<IIQ', 0x636d7066, 1, 16) + b'hello compressed', False),
            (23, 'com.apple.fs.symlink', b'../plain.bin\0', False)]:
        suffix, value = attr(name, data, stream)
        records.append((key(oid, 4, suffix), value))
    for oid, logical, size, physical in [(70, 0, BLOCK, 12), (70, BLOCK, BLOCK, 13), (74, 0, 8192, 0), (80, 0, BLOCK, 14)]:
        records.append((key(oid, 8, struct.pack('<Q', logical)), struct.pack('<QQQ', size, physical, 0)))
    records.sort(key=lambda pair: (struct.unpack('<Q', pair[0][:8])[0] & 0x0fffffffffffffff, pair[0][7] >> 4, pair[0][8:]))
    return tree(200, records)


def generate(hashed):
    blocks = [bytearray(BLOCK) for _ in range(COUNT)]
    blocks[0], blocks[1], blocks[2] = nx(5), nx(10), omap(2, 3)
    blocks[3] = tree(3, [mapping(100, 5, 4), mapping(100, 10, 5), mapping(101, 10, 6)], True)
    blocks[4], blocks[5], blocks[6] = volume(100, hashed), volume(100, hashed), volume(101, hashed, True)
    blocks[7] = omap(7, 8)
    blocks[8] = tree(8, [mapping(200, 5, 9), mapping(200, 10, 10), mapping(200, 11, 11)], True)
    blocks[9], blocks[10], blocks[11] = catalog(hashed, 'old.bin'), catalog(hashed), catalog(hashed, 'future.bin')
    blocks[12], blocks[13] = bytearray(b'A' * BLOCK), bytearray(b'B' * BLOCK)
    blocks[14][:6] = b'ATTR!!'
    snapshot = struct.pack('<QQqqQIIH', 0, 4, -1, 123456789, 0, 0, 0, 9) + b'snap-one\0'
    blocks[15] = tree(15, [(key(5, 1), snapshot)])
    return b''.join(blocks)


if __name__ == '__main__':
    directory = Path(__file__).resolve().parent
    for hashed, name in [(True, 'basic.apfs'), (False, 'case-sensitive.apfs')]:
        (directory / name).write_bytes(generate(hashed))
