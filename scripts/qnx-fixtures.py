# SPDX-License-Identifier: CC0-1.0
"""Independent field-encoded QNX4 / QNX6 fixtures; not a filesystem formatter."""
from pathlib import Path
import struct
import zlib

OUTPUT = Path('src-tauri/vendor/qnx-reader/tests/fixtures')
OUTPUT.mkdir(parents=True, exist_ok=True)


def crc_be(data):
    # Polynomial division with the non-reflected ISO polynomial, seed zero.
    crc = 0
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ (0x04c11db7 if crc & 0x80000000 else 0)) & 0xffffffff
    return crc


def qnx6(order, block_size, base, mmi):
    blocks = 256
    image = bytearray(base + 8192 + blocks * block_size)
    data_start = base + 4096

    def pack(data, offset, fmt, *values):
        struct.pack_into(order + fmt, data, offset, *values)

    def tree(size, pointers, levels):
        raw = bytearray(80)
        pack(raw, 0, 'Q', size)
        pack(raw, 8, '16I', *(pointers + [0xffffffff] * (16 - len(pointers))))
        raw[72] = levels
        return raw

    def sb(serial):
        raw = bytearray(512)
        pack(raw, 0, 'I', 0x68191122)
        pack(raw, 8, 'Q', serial)
        start = 40 if mmi else 48
        if not mmi:
            pack(raw, 28, 'HH', 4, 3)
        pack(raw, start, '6I', block_size, 32, 20, blocks, 180, 0)
        start = 64 if mmi else 72
        raw[start:start + 80] = tree(4096, list(range(4096 // block_size)), 0)
        raw[start + 80:start + 160] = tree(32, [8], 0)
        raw[start + 160:start + 240] = tree(block_size, [9], 0)
        pack(raw, 4, 'I', crc_be(raw[8:]))
        return raw

    image[base:base + 512] = sb(4)
    second = data_start + blocks * block_size
    image[second:second + 512] = sb(9)

    def block(index, raw):
        image[data_start + index * block_size:data_start + index * block_size + len(raw)] = raw

    def inode(number, mode, size, pointers, levels=0):
        raw = bytearray(128)
        pack(raw, 0, 'Q', size)
        pack(raw, 16, '4I', 1600000000, 1700000000, 1700000001, 1700000002)
        pack(raw, 32, 'H', mode)
        pack(raw, 36, '16I', *(pointers + [0xffffffff] * (16 - len(pointers))))
        raw[100] = levels
        raw[101] = 1 if mode & 0xf000 == 0x4000 else 3
        offset = data_start + (number - 1) * 128
        image[offset:offset + 128] = raw

    longname = b'this_name_is_longer_than_twenty_seven.txt'
    names = [(1, b'.'), (1, b'..'), (2, b'small.txt'), (3, b'big.bin'),
             (2, b'alias.txt'), (4, b'link'), (5, b'nested'), (6, longname)]
    inode(1, 0o40755, len(names) * 32, [10])
    inode(2, 0o100644, 12, [11])
    inode(3, 0o100644, 20 * block_size + 37, [12], 1)
    inode(4, 0o120777, 9, [13])
    inode(5, 0o40755, 96, [14])
    inode(6, 0o100600, 12, [11])

    def directory(entries):
        raw = bytearray(block_size)
        for index, (node, name) in enumerate(entries):
            offset = index * 32
            pack(raw, offset, 'I', node)
            if len(name) <= 27:
                raw[offset + 4] = len(name)
                raw[offset + 5:offset + 5 + len(name)] = name
            else:
                raw[offset + 4] = 255
                # The name is record zero in the longname file.
                checksum = 0
                for byte in name:
                    checksum = (((checksum >> 1) + byte) ^ (0x80000000 if checksum & 1 else 0)) & 0xffffffff
                pack(raw, offset + 12, 'I', checksum)
        return raw

    block(10, directory(names))
    block(11, b'hello QNX!\r\n')
    indirect = bytearray(b'\xff' * block_size)
    for index in range(21):
        pack(indirect, index * 4, 'I', 0xffffffff if index == 3 else 32 + index)
        block(32 + index, bytes((position * 7 + index) % 256 for position in range(block_size)))
    block(12, indirect)
    block(13, b'small.txt')
    block(14, directory([(5, b'.'), (1, b'..'), (2, '测试.txt'.encode())]))
    raw = bytearray(block_size)
    pack(raw, 0, 'H', len(longname))
    raw[2:2 + len(longname)] = longname
    block(9, raw)
    suffix = 'mmi' if mmi else 'bootless' if base == 0 else str(block_size)
    (OUTPUT / f'qnx6-{suffix}-{"le" if order == "<" else "be"}.zlib').write_bytes(zlib.compress(image, 9))


def qnx4():
    image = bytearray(256 * 512)

    def inode(name, mode, size, first, count, extents=1, chain=0):
        raw = bytearray(64)
        raw[:16] = name[:16].ljust(16, b'\0')
        struct.pack_into('<IIII', raw, 16, size, first, count, chain)
        struct.pack_into('<4I', raw, 32, 1600000000, 1700000000, 1700000001, 1700000002)
        struct.pack_into('<HH', raw, 48, extents, mode)
        raw[63] = 1
        return raw

    def block(number, raw):
        offset = (number - 1) * 512
        image[offset:offset + len(raw)] = raw

    block(2, inode(b'/', 0o40755, 512, 3, 1))
    root = bytearray(512)
    entries = [inode(b'.bitmap', 0o100444, 32, 5, 1),
               inode(b'small.txt', 0o100644, 12, 6, 1),
               inode(b'big.bin', 0o100644, 1300, 7, 1, 3, 8),
               inode(b'link', 0o120777, 9, 12, 1),
               inode(b'nested', 0o40755, 64, 13, 1)]
    for index, raw in enumerate(entries):
        root[index * 64:(index + 1) * 64] = raw
    # Two names point at the same inode inside .inodes storage (block four).
    for index, name in [(5, b'a_long_name_for_qnx4.txt'), (6, b'alias.txt')]:
        raw = bytearray(64)
        raw[:48] = name.ljust(48, b'\0')
        struct.pack_into('<IB', raw, 48, 4, 1)
        raw[63] = 8
        root[index * 64:(index + 1) * 64] = raw
    block(3, root)
    block(4, b'IamTHE.inodeFILE\0')
    image[3 * 512 + 64:3 * 512 + 128] = inode(b'long', 0o100600, 12, 6, 1)
    block(6, b'hello QNX!\r\n')
    payload = bytes((index * 13 + 7) % 256 for index in range(1300))
    block(7, payload[:512]); block(9, payload[512:1024]); block(11, payload[1024:])
    for number, previous, next_block, data_block in [(8, 0, 10, 9), (10, 8, 0, 11)]:
        raw = bytearray(512)
        struct.pack_into('<IIB', raw, 0, next_block, previous, 1)
        struct.pack_into('<II', raw, 16, data_block, 1)
        raw[496:503] = b'IamXblk'
        block(number, raw)
    block(12, b'small.txt')
    block(13, inode('测试.txt'.encode(), 0o100644, 12, 6, 1))
    (OUTPUT / 'qnx4.zlib').write_bytes(zlib.compress(image, 9))


qnx4()
for order, block_size, base, mmi in [('<', 512, 8192, False), ('>', 1024, 8192, False),
                                     ('<', 2048, 8192, False), ('>', 4096, 8192, False),
                                     ('<', 1024, 0, False), ('<', 1024, 0, True)]:
    qnx6(order, block_size, base, mmi)
print('QNX synthetic fixtures generated')
