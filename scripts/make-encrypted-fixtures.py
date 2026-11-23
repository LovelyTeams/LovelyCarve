"""Reproduce tiny encrypted test images using Python cryptography (OpenSSL).

No production Rust encryption code is used. All keys/passwords are public test
data. The FAT12 filesystem contains HELLO.TXT with the payload below.
Requires cryptography >= 44; run only when intentionally regenerating fixtures.
"""
import base64
import hashlib
import json
from pathlib import Path
import struct
import zlib

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id

DEST = Path(__file__).resolve().parents[1] / 'src-tauri/tests/data/encrypted'
PASSWORD = b'lovelycarve-test'
MASTER = bytes(range(64))
SALT = bytes(range(32))
CONTENT = b'LovelyCarve encrypted evidence fixture.\n'


def xts(data, key, sector=512, base=0):
    result = bytearray()
    for offset in range(0, len(data), sector):
        cipher = Cipher(algorithms.AES(key), modes.XTS((base + offset // 512).to_bytes(16, 'little'))).encryptor()
        result += cipher.update(bytes(data[offset:offset + sector])) + cipher.finalize()
    return result


def filesystem():
    data = bytearray(128 * 512)
    data[:11] = b'\xeb\x3c\x90LCFIXTUR'
    struct.pack_into('<HBHBHHBHHHII', data, 11, 512, 1, 1, 1, 32, 128, 0xf8, 1, 32, 1, 0, 0)
    data[38] = 0x29
    data[43:54] = b'ENCRYPTTEST'
    data[54:62] = b'FAT12   '
    data[510:512] = b'\x55\xaa'
    data[512:517] = b'\xf8\xff\xff\xff\x0f'
    data[1024:1035] = b'HELLO   TXT'
    data[1035] = 0x20
    struct.pack_into('<HI', data, 1050, 2, len(CONTENT))
    data[2048:2048 + len(CONTENT)] = CONTENT
    return data


def af_material():
    filler = bytes([0x5a] * 64)
    accumulator = bytes(64)
    material = bytearray()
    for _ in range(7):
        material += filler
        mixed = bytes(a ^ b for a, b in zip(accumulator, filler))
        accumulator = b''.join(hashlib.sha256(struct.pack('>I', index) + mixed[index*32:index*32+32]).digest() for index in range(2))
    material += bytes(a ^ b for a, b in zip(accumulator, MASTER))
    return material


def luks1(plain):
    image = bytearray(4096)
    image[:8] = b'LUKS\xba\xbe\x00\x01'
    image[8:11] = b'aes'
    image[40:51] = b'xts-plain64'
    image[72:78] = b'sha256'
    struct.pack_into('>II', image, 104, 8, 64)
    image[112:132] = hashlib.pbkdf2_hmac('sha256', MASTER, SALT, 10, 20)
    image[132:164] = SALT
    struct.pack_into('>I', image, 164, 10)
    struct.pack_into('>II', image, 208, 0x00ac71f3, 10)
    image[216:248] = SALT
    struct.pack_into('>II', image, 248, 2, 8)
    image[1024:1536] = xts(af_material(), hashlib.pbkdf2_hmac('sha256', PASSWORD, SALT, 10, 64))
    return image + xts(plain, MASTER)


def luks2(plain):
    b64 = lambda data: base64.b64encode(data).decode()
    metadata = {
        'keyslots': {'0': {'type': 'luks2', 'key_size': 64,
            'af': {'type': 'luks1', 'stripes': 8, 'hash': 'sha256'},
            'area': {'type': 'raw', 'offset': '32768', 'size': '4096', 'encryption': 'aes-xts-plain64', 'key_size': 64},
            'kdf': {'type': 'argon2id', 'time': 2, 'memory': 32, 'cpus': 1, 'salt': b64(SALT)}}},
        'segments': {'0': {'type': 'crypt', 'offset': '36864', 'size': 'dynamic', 'iv_tweak': '0', 'encryption': 'aes-xts-plain64', 'sector_size': 4096}},
        'digests': {'0': {'type': 'pbkdf2', 'keyslots': ['0'], 'segments': ['0'], 'hash': 'sha256', 'iterations': 10, 'salt': b64(SALT), 'digest': b64(hashlib.pbkdf2_hmac('sha256', MASTER, SALT, 10, 32))}},
        'config': {'json_size': '12288', 'keyslots_size': '4096'}, 'tokens': {},
    }
    image = bytearray(36864)
    image[:8] = b'LUKS\xba\xbe\x00\x02'
    struct.pack_into('>QQ', image, 8, 16384, 1)
    image[72:78] = b'sha256'
    image[104:168] = bytes(range(64))
    encoded = json.dumps(metadata, separators=(',', ':')).encode()
    image[4096:4096 + len(encoded)] = encoded
    image[448:480] = hashlib.sha256(image[:16384]).digest()
    # Valid secondary metadata header, although recovery is not supported.
    image[16384:32768] = image[:16384]
    image[16384:16390] = b'SKUL\xba\xbe'
    struct.pack_into('>Q', image, 16384 + 256, 16384)
    image[16384 + 448:16384 + 512] = bytes(64)
    image[16384 + 448:16384 + 480] = hashlib.sha256(image[16384:32768]).digest()
    key = Argon2id(salt=SALT, length=64, iterations=2, lanes=1, memory_cost=32).derive(PASSWORD)
    image[32768:33280] = xts(af_material(), key)
    return image + xts(plain, MASTER, 4096)


def veracrypt_header(volume, start, hidden=0, flags=0):
    header = bytearray(448)
    header[:4] = b'VERA'
    struct.pack_into('>HH', header, 4, 5, 0x010b)
    struct.pack_into('>QQQQ', header, 28, hidden, volume, start, volume)
    struct.pack_into('>II', header, 60, flags, 512)
    header[192:256] = MASTER
    struct.pack_into('>I', header, 8, zlib.crc32(header[192:]))
    struct.pack_into('>I', header, 188, zlib.crc32(header[:188]))
    return header


def veracrypt(plain, hidden=False, password=PASSWORD):
    header = veracrypt_header(len(plain), 131072, len(plain) if hidden else 0)
    salt = bytes(range(64))
    key = hashlib.pbkdf2_hmac('sha512', password, salt, 16000, 64)  # PIM = 1
    image = bytearray(131072)
    start = 65536 if hidden else 0
    image[start:start+512] = salt + xts(header, key, 448)
    return image + xts(plain, MASTER, base=256)


KEYFILE = bytes((index * 31 + 7) % 256 for index in range(1000))


def keyfile_password(password, keyfile):
    """VeraCrypt's keyfile pool: a running CRC-32 of the keyfile added into 64 bytes, then the password."""
    table = []
    for value in range(256):
        for _ in range(8):
            value = (value >> 1) ^ 0xEDB88320 if value & 1 else value >> 1
        table.append(value)
    pool, crc, position = bytearray(64), 0xFFFFFFFF, 0
    for byte in keyfile:
        crc = table[(crc ^ byte) & 0xff] ^ (crc >> 8)
        for shift in (24, 16, 8, 0):
            pool[position] = (pool[position] + ((crc >> shift) & 0xff)) & 0xff
            position += 1
        position %= 64
    for index, byte in enumerate(password):
        pool[index] = (pool[index] + byte) & 0xff
    return bytes(pool)


def veracrypt_system(plain):
    """A disk whose MBR partition 1 (at 64 KiB) is system-encrypted: the header in sector 62,
    SHA-256 at the pre-boot count for PIM 1 (2048), data units numbered from the disk start."""
    start = 65536
    image = bytearray(start + len(plain) + 32768)
    struct.pack_into('<B3sB3sII', image, 446, 0x80, b'\x00\x02\x00', 0x07, b'\xfe\xff\xff',
                     start // 512, len(plain) // 512)
    image[510:512] = b'\x55\xaa'
    salt = bytes(range(64, 128))
    key = hashlib.pbkdf2_hmac('sha256', PASSWORD, salt, 2048, 64)
    image[31744:32256] = salt + xts(veracrypt_header(len(plain), start, flags=1), key, 448)
    image[start:start + len(plain)] = xts(plain, MASTER, base=start // 512)
    return image


if __name__ == '__main__':
    DEST.mkdir(parents=True, exist_ok=True)
    plain = filesystem()
    files = {'plain.img': plain, 'luks1.img': luks1(plain), 'luks2.img': luks2(plain),
             'veracrypt.hc': veracrypt(plain), 'hidden.hc': veracrypt(plain, True),
             'veracrypt-keyfile.hc': veracrypt(plain, password=keyfile_password(PASSWORD, KEYFILE)),
             'veracrypt-keyfile.bin': KEYFILE, 'veracrypt-system.img': veracrypt_system(plain)}
    for name, data in files.items():
        (DEST / name).write_bytes(data)
    (DEST / 'sha256.json').write_text(json.dumps({name: hashlib.sha256(data).hexdigest() for name, data in files.items()}, indent=2) + '\n')
