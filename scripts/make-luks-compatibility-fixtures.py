"""Independent LUKS1 cipher/IV/hash fixtures; no Rust reader code is used.

Requires pycryptodome and pyserpent==1.2.0 (generation only). Test keys are
public. CBC is unpadded; RC4 starts a fresh stream for each dm-crypt sector.
Run this script only when intentionally regenerating the committed corpus.
"""
import base64
import hashlib
import json
import struct
from pathlib import Path

from Crypto.Cipher import AES, ARC4, Blowfish
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from pyserpent import Serpent

DEST = Path(__file__).resolve().parents[1] / 'vendor/luks/tests/data/compatibility.json'
PASSWORD = b'LovelyCarve-\xe5\xaf\x86\xe7\xa0\x81'
SALT = bytes(range(32))
UUID = 'f4e55035-9816-4c35-a82d-e4c6cf214a55'
PLAIN = bytes((index * 17 + 7) % 256 for index in range(1536))


def xor(left, right):
    return bytes(a ^ b for a, b in zip(left, right))


def block_encrypt(name, key, data):
    if name == 'serpent':
        return Serpent(key).encrypt(data)
    module = AES if name == 'aes' else Blowfish
    return module.new(key, module.MODE_ECB).encrypt(data)


def encrypt(name, mode, key, data, base=0):
    chain, _, iv_mode = mode.partition('-')
    result = bytearray()
    block_size = 8 if name == 'blowfish' else 16
    for offset in range(0, len(data), 512):
        sector = base + offset // 512
        unit = data[offset:offset + 512]
        iv = bytes(block_size)
        if iv_mode == 'plain':
            iv = (sector & 0xffffffff).to_bytes(4, 'little') + bytes(block_size - 4)
        elif iv_mode == 'plain64':
            iv = sector.to_bytes(8, 'little') + bytes(block_size - 8)
        elif iv_mode == 'benbi':
            counter = (sector * (512 // block_size) + 1) & 0xffffffffffffffff
            iv = bytes(block_size - 8) + counter.to_bytes(8, 'big')
        elif iv_mode.startswith('essiv:'):
            salt = hashlib.new(iv_mode.split(':')[1], key).digest()
            iv = block_encrypt(name, salt, sector.to_bytes(8, 'little') + bytes(block_size - 8))
        if name == 'arc4':
            result += ARC4.new(key).encrypt(unit)
        elif chain == 'xts':
            encryptor = Cipher(algorithms.AES(key), modes.XTS(iv)).encryptor()
            result += encryptor.update(unit) + encryptor.finalize()
        else:
            for start in range(0, 512, block_size):
                block = unit[start:start + block_size]
                ciphertext = block_encrypt(name, key, xor(block, iv) if chain == 'cbc' else block)
                result += ciphertext
                if chain == 'cbc':
                    iv = ciphertext
    return bytes(result)


def af_material(master, hash_name, stripes=7):
    filler = bytes([0x5a] * len(master))
    accumulator = bytes(len(master))
    material = bytearray()
    digest_size = hashlib.new(hash_name).digest_size
    for _ in range(stripes - 1):
        material += filler
        mixed = xor(accumulator, filler)
        accumulator = b''.join(
            hashlib.new(hash_name, struct.pack('>I', index // digest_size) + mixed[index:index + digest_size]).digest()[:len(mixed[index:index + digest_size])]
            for index in range(0, len(master), digest_size))
    material += xor(accumulator, master)
    material += bytes((-len(material)) % 512)
    return bytes(material)


def image(name, mode, hash_name, key):
    header = bytearray(4096)
    header[:8] = b'LUKS\xba\xbe\x00\x01'
    header[8:8 + len(name)] = name.encode()
    header[40:40 + len(mode)] = mode.encode()
    header[72:72 + len(hash_name)] = hash_name.encode()
    struct.pack_into('>II', header, 104, 8, len(key))
    header[112:132] = hashlib.pbkdf2_hmac(hash_name, key, SALT, 10, 20)
    header[132:164] = SALT
    struct.pack_into('>I', header, 164, 10)
    header[168:204] = UUID.encode()
    for slot in range(8):
        struct.pack_into('>I', header, 208 + slot * 48, 0xdead)
    # Slot 1 proves that keyslots are tried after a wrong password slot.
    for slot, password in [(0, b'other-password'), (1, PASSWORD)]:
        start = 208 + slot * 48
        material = af_material(key, hash_name)
        area = 1024 + slot * len(material)
        struct.pack_into('>II', header, start, 0xac71f3, 10)
        header[start + 8:start + 40] = SALT
        struct.pack_into('>II', header, start + 40, area // 512, 7)
        slot_key = hashlib.pbkdf2_hmac(hash_name, password, SALT, 10, len(key))
        header[area:area + len(material)] = encrypt(name, mode, slot_key, material)
    payload_start = (len(header) + 511) // 512 * 512
    header += bytes(payload_start - len(header))
    struct.pack_into('>I', header, 104, payload_start // 512)
    return bytes(header) + encrypt(name, mode, key, PLAIN)


def image2(name, mode, key, area_mode, area_key_size):
    b64 = lambda data: base64.b64encode(data).decode()
    metadata = {
        'keyslots': {'0': {'type': 'luks2', 'key_size': len(key),
            'af': {'type': 'luks1', 'stripes': 7, 'hash': 'sha384'},
            'area': {'type': 'raw', 'offset': '32768', 'size': '4096', 'encryption': 'aes-' + area_mode, 'key_size': area_key_size},
            'kdf': {'type': 'pbkdf2', 'hash': 'sha224', 'iterations': 10, 'salt': b64(SALT)}}},
        'segments': {'0': {'type': 'crypt', 'offset': '36864', 'size': 'dynamic', 'iv_tweak': '0', 'encryption': name + '-' + mode, 'sector_size': 512}},
        'digests': {'0': {'type': 'pbkdf2', 'keyslots': ['0'], 'segments': ['0'], 'hash': 'sha224', 'iterations': 10, 'salt': b64(SALT), 'digest': b64(hashlib.pbkdf2_hmac('sha224', key, SALT, 10, 28))}},
        'config': {'json_size': '12288', 'keyslots_size': '4096'}, 'tokens': {},
    }
    header = bytearray(36864)
    header[:8] = b'LUKS\xba\xbe\x00\x02'
    struct.pack_into('>QQ', header, 8, 16384, 1)
    header[72:78] = b'sha256'
    header[104:168] = bytes(range(64))
    header[168:204] = UUID.encode()
    encoded = json.dumps(metadata, separators=(',', ':')).encode()
    header[4096:4096 + len(encoded)] = encoded
    header[448:480] = hashlib.sha256(header[:16384]).digest()
    header[16384:32768] = header[:16384]
    header[16384:16390] = b'SKUL\xba\xbe'
    struct.pack_into('>Q', header, 16384 + 256, 16384)
    header[16384 + 448:16384 + 512] = bytes(64)
    header[16384 + 448:16384 + 480] = hashlib.sha256(header[16384:32768]).digest()
    slot_key = hashlib.pbkdf2_hmac('sha224', PASSWORD, SALT, 10, area_key_size)
    material = encrypt('aes', area_mode, slot_key, af_material(key, 'sha384'))
    header[32768:32768 + len(material)] = material
    return bytes(header) + encrypt(name, mode, key, PLAIN)


if __name__ == '__main__':
    cases = []
    for name, lengths in [('aes', [16, 24, 32]), ('blowfish', [5, 16, 56]), ('serpent', [16, 24, 32])]:
        for length in lengths:
            key = bytes(range(length))
            for mode in ['ecb', 'cbc-null', 'cbc-plain', 'cbc-plain64', 'cbc-benbi', 'cbc-essiv:sha256']:
                for hash_name in ['sha224', 'sha384']:
                    cases.append({'cipher': name, 'mode': mode, 'hash': hash_name, 'key': key.hex(),
                                  'image': base64.b64encode(image(name, mode, hash_name, key)).decode()})
            if name != 'aes':
                mode = 'cbc-essiv:sha1'
                cases.append({'cipher': name, 'mode': mode, 'hash': 'sha1', 'key': key.hex(),
                              'image': base64.b64encode(image(name, mode, 'sha1', key)).decode()})
    for length in [5, 7, 16, 32, 256]:
        key = bytes(range(length))
        cases.append({'cipher': 'arc4', 'mode': 'ecb', 'hash': 'sha512', 'key': key.hex(),
                      'image': base64.b64encode(image('arc4', 'ecb', 'sha512', key)).decode()})
    for length in [32, 64]:
        key = bytes(range(length))
        for mode in ['xts-null', 'xts-plain', 'xts-plain64', 'xts-benbi']:
            cases.append({'cipher': 'aes', 'mode': mode, 'hash': 'sha256', 'key': key.hex(),
                          'image': base64.b64encode(image('aes', mode, 'sha256', key)).decode()})
    for name, mode, length, area_mode, area_size in [('aes', 'xts-plain64', 64, 'cbc-essiv:sha256', 16),
            ('blowfish', 'cbc-benbi', 56, 'xts-plain64', 32), ('serpent', 'cbc-plain64', 24, 'cbc-plain64', 32)]:
        key = bytes(range(length))
        cases.append({'cipher': name, 'mode': mode, 'hash': 'sha224', 'key': key.hex(),
                      'image': base64.b64encode(image2(name, mode, key, area_mode, area_size)).decode()})
    vectors = []
    for name, length in [('aes', 32), ('blowfish', 16), ('serpent', 32)]:
        for mode in ['cbc-plain', 'cbc-plain64', 'cbc-benbi', 'cbc-essiv:sha256']:
            key = bytes(range(length))
            vectors.append({'cipher': name, 'mode': mode, 'key': key.hex(), 'sector': 2**32 + 7,
                            'ciphertext': base64.b64encode(encrypt(name, mode, key, PLAIN[:1024], 2**32 + 7)).decode()})
    DEST.parent.mkdir(parents=True, exist_ok=True)
    DEST.write_text(json.dumps({'producer': 'Python hashlib/OpenSSL, PyCryptodome, pyserpent 1.2.0; public test data',
                               'containers': cases, 'vectors': vectors}, indent=2) + '\n', encoding='utf-8', newline='\n')
