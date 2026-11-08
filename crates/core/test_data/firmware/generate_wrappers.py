"""CC0-1.0 independent SafeLoader, uImage and CHK fixtures; Python stdlib only."""
from pathlib import Path
import hashlib
import struct
import zlib

ROOT = Path(__file__).resolve().parent
SALT = bytes.fromhex('7a2b15ed9b98596de504ab44ac2a9f4e')

def uimage(payload, kind=2):
    header = bytearray(64)
    struct.pack_into('>IIIIIII', header, 0, 0x27051956, 0, 0, len(payload), 0, 0, zlib.crc32(payload))
    header[28:32] = bytes([5, 5, kind, 0])
    struct.pack_into('>I', header, 4, zlib.crc32(header))
    return header + payload

def checksum(data):
    first = sum(data) & 0xffffffff
    second = sum((len(data)-index) * value for index, value in enumerate(data)) & 0xffffffff
    def fold(value):
        value = (value >> 16) + (value & 65535)
        return ((value >> 16) + value) & 65535
    return fold(first) | fold(second) << 16

def chk(data):
    header = bytearray(48)
    struct.pack_into('>II8sIIIIII', header, 0, 0x2a23245e, 48, bytes(8), checksum(data), 0, len(data), 0, checksum(data), 0)
    header[40:48] = b'SYNTH-ID'
    struct.pack_into('>I', header, 36, checksum(header))
    return header + data

def safeloader(kernel, rootfs):
    image = bytearray(b'\xff' * 0x1814)
    table = bytearray()
    for name, content in [('partition-table', b'synthetic flash layout'), ('support-list', b'SYNTHETIC'), ('os-image', kernel), ('file-system', rootfs)]:
        table.extend(f'fwup-ptn {name} base 0x{len(image)-0x1014:05x} size 0x{len(content):05x}\t\r\n'.encode())
        image.extend(content)
    table.append(0)
    image[0x1014:0x1014+len(table)] = table
    struct.pack_into('>I', image, 0, len(image))
    image[4:20] = hashlib.md5(SALT + image[20:]).digest()
    return image

if __name__ == '__main__':
    rootfs = (ROOT.parent / 'rom-images/synthetic-le.cramfs').read_bytes()
    kernel = uimage(b'Independent synthetic U-Boot kernel.\n')
    for name, data in [('synthetic-uimage.bin', kernel + rootfs), ('synthetic-safeloader.bin', safeloader(kernel, rootfs)), ('synthetic.chk', chk(kernel + rootfs))]:
        (ROOT / name).write_bytes(data)
