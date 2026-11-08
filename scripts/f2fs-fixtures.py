# SPDX-License-Identifier: CC0-1.0
"""Invented F2FS checkpoint/NAT/inode fixture encoded independently with Python.

This is not a full mkfs implementation. The fixture tests the documented reader
structures; syzkaller's external mkfs.f2fs corpus separately checks compatibility.
"""
from pathlib import Path
import struct
import zlib

BLOCK = 4096
image = bytearray(8192 * BLOCK)
uuid = bytes(range(16))


def put(data, offset, fmt, *values):
    struct.pack_into('<' + fmt, data, offset, *values)


def crc(seed, data):
    return zlib.crc32(data, seed ^ 0xffffffff) ^ 0xffffffff


def block(number, data):
    image[number * BLOCK:(number + 1) * BLOCK] = data


sb = bytearray(3072)
put(sb, 0, 'IHH', 0xf2f52010, 1, 15)
for offset, value in [(8, 9), (12, 3), (16, 12), (20, 9), (24, 1), (28, 1),
                      (0x30, 15), (0x34, 2), (0x38, 2), (0x3c, 2), (0x40, 1),
                      (0x44, 8), (0x48, 512), (0x4c, 512), (0x50, 1536),
                      (0x54, 2560), (0x58, 3584), (0x5c, 4096), (0x60, 3),
                      (0x64, 1), (0x68, 2), (0x884, 0x168)]:
    put(sb, offset, 'I', value)
put(sb, 0x24, 'Q', 8192)
sb[0x6c:0x7c] = uuid
for offset in [1024, 5120]:
    image[offset:offset + len(sb)] = sb


def checkpoint(base, version):
    head = bytearray(BLOCK)
    put(head, 0, 'Q', version)
    put(head, 8, 'Q', 4096)
    put(head, 0x84, 'I', 5)  # UMOUNT + COMPACT_SUM
    for offset, value in [(0x88, 4), (0x8c, 1), (0x9c, 64), (0xa0, 64), (0xa4, 4092)]:
        put(head, offset, 'I', value)
    put(head, 4092, 'I', crc(0xf2f52010, head[:4092]))
    block(base, head)
    block(base + 3, head)


checkpoint(512, 1)
checkpoint(1024, 2)
nat = bytearray(BLOCK)


def inode(nid, mode, size, flags=0x20, xattr=0):
    data = bytearray(BLOCK)
    put(data, 0, 'H', mode)
    data[3] = flags
    put(data, 12, 'I', 1)
    put(data, 16, 'Q', size)
    for offset in [32, 40, 48]:
        put(data, offset, 'Q', 1700000000)
    for offset in [56, 60, 64]:
        put(data, offset, 'I', 123456789)
    put(data, 68, 'I', 7)
    put(data, 360, 'HH', 24, xattr)
    put(data, 372, 'QI', 1600000000, 987654321)
    put(data, 4072, 'II', nid, nid)
    return data


def save_inode(nid, data):
    seed = crc(0xffffffff, uuid)
    seed = crc(seed, struct.pack('<I', nid))
    seed = crc(seed, data[68:72])
    data[368:372] = bytes(4)
    put(data, 368, 'I', crc(seed, data))
    number = 4096 + nid - 3
    block(number, data)
    put(nat, nid * 9, 'BII', 0, nid, number)


def dentries(entries, slots, length, start):
    data = bytearray(length)
    slot = 0
    for name, nid, kind in entries:
        name = name.encode()
        count = (len(name) + 7) // 8
        for index in range(slot, slot + count):
            data[index // 8] |= 1 << (index % 8)
        put(data, start + slot * 11, 'IIHB', 0, nid, len(name), kind)
        offset = start + slots * 11 + slot * 8
        data[offset:offset + len(name)] = name
        slot += count
    return data


root = inode(3, 0o40755, 3648, flags=0x25, xattr=4)
entries = [('small.txt', 4, 1), ('big.bin', 5, 1), ('link', 6, 7),
           ('nested', 7, 2), ('alias.txt', 4, 1)]
length = 3648
slots = length * 8 // 153
root[388:4036] = dentries(entries, slots, length, length - slots * 19)
save_inode(3, root)
small = inode(4, 0o100644, 13, flags=0x23, xattr=4)
small[388:401] = b'hello F2FS!\r\n'
save_inode(4, small)
big = inode(5, 0o100644, 1000 * BLOCK + 37, flags=0x21, xattr=4)
for index in range(913):
    put(big, 384 + index * 4, 'I', 0 if index == 3 else 0xffffffff if index == 4 else 4300 + index)
put(big, 4052, 'I', 9)
save_inode(5, big)
direct = bytearray(BLOCK)
for index in range(88):
    put(direct, index * 4, 'I', 4300 + 913 + index)
put(direct, 4072, 'II', 9, 5)
block(4102, direct)
put(nat, 9 * 9, 'BII', 0, 5, 4102)
payload = bytes(range(256)) * 16
for index in range(1001):
    block(4300 + index, payload)
link = inode(6, 0o120777, 9, flags=0x22)
link[388:397] = b'small.txt'
save_inode(6, link)
directory = inode(7, 0o40755, 4096)
put(directory, 384, 'I', 4200)
block(4200, dentries([('测试.txt', 8, 1)], 214, 4096, 30))
save_inode(7, directory)
inner = inode(8, 0o100600, 13, flags=0x22)
inner[388:401] = b'hello F2FS!\r\n'
save_inode(8, inner)
block(2560, nat)
output = Path('vendor/f2fs-reader/tests/fixtures')
output.mkdir(parents=True, exist_ok=True)
(output / 'synthetic.f2fs.zlib').write_bytes(zlib.compress(image, 9))
print('F2FS fixture generated:', len(image), 'bytes')
