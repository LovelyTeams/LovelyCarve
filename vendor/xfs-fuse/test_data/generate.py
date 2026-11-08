"""Rebuild the synthetic V4 host-dispatch fixture; contains no evidence data."""
from pathlib import Path
import hashlib
import struct

BLOCK = 4096
image = bytearray(32 * BLOCK)

def put(offset, fmt, value):
    struct.pack_into('>' + fmt, image, offset, value)

def extent(logical, physical, count, unwritten=False):
    return ((int(unwritten) << 127) | (logical << 73) | (physical << 21) | count).to_bytes(16, 'big')

put(0, 'I', 0x58465342)
put(4, 'I', BLOCK)
put(8, 'Q', 32)
image[32:48] = bytes([0x11]) * 16
put(56, 'Q', 16)
put(84, 'I', 32)
put(88, 'I', 1)
put(100, 'H', 0x2004)
put(102, 'H', 512)
put(104, 'H', 512)
put(106, 'H', 8)
image[108:112] = b'test'
image[120:125] = bytes([12, 9, 9, 3, 5])

def inode(number, mode, form, size, records):
    offset = number * 512
    put(offset, 'H', 0x494e)
    put(offset + 2, 'H', mode)
    image[offset + 4:offset + 6] = bytes([2, form])
    put(offset + 8, 'I', 1000)
    put(offset + 12, 'I', 1001)
    put(offset + 16, 'I', 2)
    put(offset + 32, 'i', -2)
    put(offset + 36, 'I', 123)
    put(offset + 56, 'Q', size)
    put(offset + 76, 'I', records)
    image[offset + 83] = 2
    return offset + 100

names = [(b'data', 17), (b'link', 18), (b'\xff', 17), (b'\xfe', 17)]
directory = bytearray([len(names), 0, 0, 0, 0, 16])
for index, (name, number) in enumerate(names):
    directory += bytes([len(name)]) + struct.pack('>H', 32 + index * 32) + name + struct.pack('>I', number)
offset = inode(16, 0x41ed, 1, len(directory), 0)
image[offset:offset + len(directory)] = directory
offset = inode(17, 0x81a4, 2, 5 * BLOCK - 7, 3)
image[offset:offset + 48] = extent(0, 8, 1) + extent(2, 9, 1, True) + extent(4, 10, 1)
offset = inode(18, 0xa1ff, 1, 4, 0)
image[offset:offset + 4] = b'data'
image[8 * BLOCK:9 * BLOCK] = b'A' * BLOCK
image[9 * BLOCK:10 * BLOCK] = b'S' * BLOCK
image[10 * BLOCK:11 * BLOCK] = b'Z' * BLOCK
path = Path(__file__).with_name('minimal-v4.bin')
path.write_bytes(image)
print(path.name, hashlib.sha256(image).hexdigest())
