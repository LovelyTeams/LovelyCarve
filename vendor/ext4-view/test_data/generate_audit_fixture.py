"""Generate synthetic ext records for capability regression tests (no user data).

The layout is deliberately small and not mountable: allocation bitmaps are not
populated. This tests parser contracts, not kernel/e2fsck interoperability.
"""
from pathlib import Path
import struct

image = bytearray(64 * 1024)


def put(offset, fmt, *values):
    struct.pack_into('<' + fmt, image, offset, *values)


superblock = 1024
for offset, value in [(0, 32), (4, 64), (20, 1), (32, 64), (40, 32),
                      (76, 1), (96, 0x8442)]:
    put(superblock + offset, 'I', value)
put(superblock + 56, 'H', 0xef53)
put(superblock + 88, 'H', 256)
put(2048 + 8, 'I', 4)  # inode table, one block group


def inode(number, mode, size, flags=0):
    start = 4096 + (number - 1) * 256
    put(start, 'H', mode)
    put(start + 4, 'I', size)
    put(start + 26, 'H', 1)
    put(start + 32, 'I', flags)
    put(start + 128, 'H', 32)
    return start


def resident(number, mode, data, extra_attribute=False):
    start = inode(number, mode, len(data), 0x10000000)
    image[start + 40:start + 100] = data[:60].ljust(60, b'\0')
    put(start + 160, 'I', 0xea020000)
    put(start + 164, 'BBHIII', 4, 7, 68, 0, len(data[60:]), 0)
    image[start + 180:start + 184] = b'data'
    image[start + 232:start + 232 + len(data[60:])] = data[60:]
    if extra_attribute:
        put(start + 184, 'BBHIII', 4, 4, 64, 0, 1, 0)
        image[start + 200:start + 204] = b'flag'
        image[start + 228] = ord('y')
    return start


def record(number, name, kind, length):
    return struct.pack('<IHBB', number, length, len(name), kind) + name + bytes(length - 8 - len(name))


root = inode(2, 0x41ed, 1024)
put(root + 40, 'I', 20)
root_entries = [(2, b'.', 2), (2, b'..', 2), (12, b'inline', 1),
                (13, b'compact', 2), (15, b'legacy', 1), (16, b'attrs', 1),
                (18, b'link', 7), (19, b'unwritten', 1)]
cursor = 20 * 1024
for index, (number, name, kind) in enumerate(root_entries):
    length = ((8 + len(name) + 3) & ~3) if index != len(root_entries) - 1 else 21 * 1024 - cursor
    image[cursor:cursor + length] = record(number, name, kind, length)
    cursor += length

resident(12, 0x81a4, bytes(range(80)), True)
resident(13, 0x41ed, struct.pack('<I', 2) + record(12, b'base', 1, 56) + record(16, b'tail', 1, 20))
legacy = inode(15, 0x81a4, 14 * 1024)
for index in range(12):
    put(legacy + 40 + 4 * index, 'I', 40 + index)
    image[(40 + index) * 1024:(41 + index) * 1024] = bytes([index + 1]) * 1024
put(legacy + 88, 'I', 53)
put(53 * 1024, 'I', 54)
image[54 * 1024:55 * 1024] = b'L' * 1024

attributes = inode(16, 0x81a4, 0)
put(attributes + 104, 'I', 30)
put(30 * 1024, 'IIII', 0xea020000, 1, 1, 0)
put(30 * 1024 + 32, 'BBHIII', 5, 1, 0, 17, 1024, 0)
image[30 * 1024 + 48:30 * 1024 + 53] = b'large'
ea = inode(17, 0x81a4, 1024, 0x200000)
put(ea + 40, 'I', 31)
image[31 * 1024:32 * 1024] = bytes(range(256)) * 4
resident(18, 0xa1ff, b'/' + b'p' * 79)

unwritten = inode(19, 0x81a4, 2048, 0x80000)
put(unwritten + 40, 'HHHHI', 0xf30a, 2, 4, 0, 0)
put(unwritten + 52, 'IHHI', 0, 0x8001, 0, 55)
put(unwritten + 64, 'IHHI', 1, 1, 0, 56)
image[55 * 1024:56 * 1024] = b'S' * 1024  # stale bytes must not be returned
image[56 * 1024:57 * 1024] = b'W' * 1024

Path(__file__).with_name('audit_ext.bin').write_bytes(image)
