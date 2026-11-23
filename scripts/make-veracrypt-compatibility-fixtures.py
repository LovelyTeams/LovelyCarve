"""Write the independent VeraCrypt / TrueCrypt compatibility corpus.

Small synthetic containers built with Python's hashlib/hmac (PBKDF2 over
SHA-512, SHA-256, BLAKE2s, Whirlpool, RIPEMD-160), `cryptography` (AES,
Camellia, Argon2id) and the `twofish` package; XTS, the headers, keyfile
mixing and every layout are written here from VeraCrypt's format, without the
Rust reader. All passwords and keys are public test data.

Run from the repository root, then `just test-veracrypt`:

    python scripts/make-veracrypt-compatibility-fixtures.py
"""
import base64
import hashlib
import hmac
import json
from pathlib import Path
import struct
import zlib

from cryptography.hazmat.decrepit.ciphers.algorithms import Camellia
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id
from twofish import Twofish

DEST = Path(__file__).resolve().parents[1] / 'src-tauri/vendor/veracrypt/tests/data/compatibility.json'
PASSWORD = 'LovelyCarve-密码'.encode()
HIDDEN_PASSWORD = 'hidden-密码'.encode()
LONG_PASSWORD = ('LovelyCarve-' * 6).encode()  # 72 bytes: the 128-byte keyfile pool
KEYFILES = {
    'photo': bytes((index * 31 + 7) % 256 for index in range(5000)),
    'notes': b'LovelyCarve keyfile\n' * 9,
}
MASK = (1 << 128) - 1


# ── ciphers and XTS ──────────────────────────────────────────────────────────

def ecb(name, key):
    if name == 'aes':
        return Cipher(algorithms.AES(key), modes.ECB()).encryptor().update
    if name == 'camellia':
        return Cipher(Camellia(key), modes.ECB()).encryptor().update
    if name == 'twofish':
        return Twofish(key).encrypt
    raise ValueError(name)


def xts_layer(name, key1, key2, data, first_unit):
    encrypt, tweak_cipher = ecb(name, key1), ecb(name, key2)
    out = bytearray()
    for start in range(0, len(data), 512):
        unit = data[start:start + 512]
        number = first_unit + start // 512
        tweak = int.from_bytes(tweak_cipher(number.to_bytes(8, 'little') + bytes(8)), 'little')
        for offset in range(0, len(unit), 16):
            block = int.from_bytes(unit[offset:offset + 16], 'little') ^ tweak
            sealed = int.from_bytes(encrypt(block.to_bytes(16, 'little')), 'little') ^ tweak
            out += sealed.to_bytes(16, 'little')
            tweak = ((tweak << 1) ^ (0x87 if tweak >> 127 else 0)) & MASK
    return bytes(out)


def xts(chain, key, data, first_unit):
    """Encrypt as VeraCrypt: cipher j of n uses key[32j:] and key[32(n+j):]; the first cipher of the table order encrypts first."""
    count = len(chain)
    for index, name in enumerate(chain):
        data = xts_layer(name, key[32 * index:32 * index + 32],
                         key[32 * (count + index):32 * (count + index) + 32], data, first_unit)
    return data


def check_xts():
    key = bytes(range(64))
    data = bytes(index % 251 for index in range(1024))
    expected = b''
    for unit in range(2):
        encryptor = Cipher(algorithms.AES(key), modes.XTS((9 + unit).to_bytes(16, 'little'))).encryptor()
        expected += encryptor.update(data[512 * unit:512 * unit + 512]) + encryptor.finalize()
    assert xts(['aes'], key, data, 9) == expected, 'XTS disagrees with OpenSSL'


# ── key derivation ───────────────────────────────────────────────────────────

def pbkdf2_blake2s(password, salt, iterations, length):
    out = b''
    index = 1
    while len(out) < length:
        u = hmac.new(password, salt + index.to_bytes(4, 'big'), hashlib.blake2s).digest()
        total = bytearray(u)
        for _ in range(iterations - 1):
            u = hmac.new(password, u, hashlib.blake2s).digest()
            total = bytearray(a ^ b for a, b in zip(total, u))
        out += bytes(total)
        index += 1
    return out[:length]


def argon2_cost(pim):
    pim = pim or 12
    memory_mib = min(64 + (pim - 1) * 32, 1024)
    passes = 3 + (pim - 1) // 3 if pim <= 31 else 13 + (pim - 31)
    return passes, memory_mib * 1024


def derive(prf, password, salt, iterations, length):
    if prf == 'argon2id':
        passes, memory = iterations
        return Argon2id(salt=salt, length=192, iterations=passes, lanes=1, memory_cost=memory).derive(password)[:length]
    if prf == 'blake2s':
        return pbkdf2_blake2s(password, salt, iterations, length)
    return hashlib.pbkdf2_hmac(prf, password, salt, iterations, length)


def volume_iterations(prf, pim):
    if prf == 'argon2id':
        return argon2_cost(pim)
    if pim == 0:
        return 655331 if prf == 'ripemd160' else 500000
    return 15000 + pim * 1000


def boot_iterations(prf, pim):
    if prf in ('sha512', 'whirlpool'):
        return volume_iterations(prf, pim)
    if pim == 0:
        return 327661 if prf == 'ripemd160' else 200000
    return pim * 2048


# ── keyfiles ─────────────────────────────────────────────────────────────────

CRC_TABLE = []
for value in range(256):
    for _ in range(8):
        value = (value >> 1) ^ 0xEDB88320 if value & 1 else value >> 1
    CRC_TABLE.append(value)


def keyfile_password(password, names):
    size = 64 if len(password) <= 64 else 128
    pool = bytearray(size)
    for name in names:
        data = KEYFILES[name][:1 << 20]
        crc, position = 0xFFFFFFFF, 0
        for index, byte in enumerate(data):
            crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >> 8)
            assert crc ^ 0xFFFFFFFF == zlib.crc32(data[:index + 1]) or index > 64
            for shift in (24, 16, 8, 0):
                pool[position] = (pool[position] + ((crc >> shift) & 0xff)) & 0xff
                position += 1
            if position >= size:
                position = 0
    for index, byte in enumerate(password):
        pool[index] = (pool[index] + byte) & 0xff
    return bytes(pool)


# ── headers and containers ───────────────────────────────────────────────────

def plaintext(length, seed):
    return bytes((index * 29 + seed) % 256 for index in range(length))


def master_key(seed, length=192):
    return bytes((index * 73 + seed) % 256 for index in range(length))


def header(fields):
    data = bytearray(448)
    data[0:4] = fields.get('magic', b'VERA')
    version = fields.get('version', 5)
    struct.pack_into('>HH', data, 4, version, fields.get('required', 0x010b))
    struct.pack_into('>QQQQ', data, 28, fields.get('hidden', 0), fields['volume'],
                     fields['start'], fields.get('length', fields['volume']))
    struct.pack_into('>II', data, 60, fields.get('flags', 0), fields.get('sector', 512))
    data[192:448] = fields['master'][:256].ljust(256, b'\0')
    struct.pack_into('>I', data, 8, zlib.crc32(bytes(data[192:448])))
    if version >= 4:
        struct.pack_into('>I', data, 188, zlib.crc32(bytes(data[:188])))
    return bytes(data)


def header_sector(fields, chain, prf, password, iterations, salt_seed):
    salt = bytes((index * 11 + salt_seed) % 256 for index in range(64))
    key = derive(prf, password, salt, iterations, 64 * len(chain))
    return salt + xts(chain, key, header(fields), 0)


class Container:
    def __init__(self, name, size):
        self.name, self.size, self.segments, self.checks = name, size, [], []

    def put(self, offset, data):
        self.segments.append({'offset': offset, 'data': base64.b64encode(data).decode()})

    def check(self, request, expect):
        self.checks.append({'request': request, 'expect': expect})

    def json(self):
        return {'name': self.name, 'size': self.size, 'segments': self.segments, 'checks': self.checks}


def request(password=PASSWORD, keyfiles=(), pim=1, prf=None, header='auto', system=None):
    return {'password': password.decode(), 'keyfiles': list(keyfiles), 'pim': pim, 'prf': prf,
            'header': header, 'system': system}


def opened(name, prf, iterations, cipher, offset, length, seed, header_kind='normal', compare=None, **extra):
    """What a check expects; `compare` is the plaintext byte range to compare with the seed's pattern."""
    start, end = compare or (0, length)
    expect = {'flavor': name, 'header': header_kind, 'prf': prf, 'iterations': iterations,
              'cipher': cipher, 'view_start': offset, 'size': length, 'seed': seed,
              'compare': [start, end], 'sector_size': 512, 'complete': True, 'plaintext_boot_sector': False}
    expect.update(extra)
    return expect


def plain_volume(name, chain, prf, pim, seed, cipher_name, *, password=PASSWORD, keyfiles=(), prf_filter=None,
                 sector=512, flags=0, length=8192):
    """A V2 container: header at 0, data at 128 KiB, backups at the end."""
    data = plaintext(length, seed)
    master = master_key(seed)
    fields = {'volume': length, 'start': 131072, 'master': master, 'sector': sector, 'flags': flags}
    secret = keyfile_password(password, keyfiles) if keyfiles else password
    iterations = volume_iterations(prf, pim)
    container = Container(name, 131072 + length + 131072)
    container.put(0, header_sector(fields, chain, prf, secret, iterations, seed))
    container.put(131072, xts(chain, master, data, 131072 // 512))
    request_iterations = iterations[0] if prf == 'argon2id' else iterations
    expect = opened('VeraCrypt', prf, request_iterations, cipher_name, 131072, length, seed, sector_size=sector)
    container.check(request(password, keyfiles, pim, prf_filter), expect)
    return container


def containers():
    yield plain_volume('sha512-aes', ['aes'], 'sha512', 1, 3, 'AES')
    yield plain_volume('sha256-aes', ['aes'], 'sha256', 2, 5, 'AES')
    yield plain_volume('blake2s-aes', ['aes'], 'blake2s', 1, 7, 'AES')
    yield plain_volume('whirlpool-twofish', ['twofish'], 'whirlpool', 1, 9, 'Twofish')
    # RIPEMD-160 with a PIM: 15000 + 1000 * PIM, as every VeraCrypt volume PRF.
    yield plain_volume('ripemd160-pim-aes', ['aes'], 'ripemd160', 3, 11, 'AES')
    yield plain_volume('argon2id-aes', ['aes'], 'argon2id', 1, 13, 'AES')
    yield plain_volume('sha512-camellia', ['camellia'], 'sha512', 1, 15, 'Camellia')
    yield plain_volume('sha512-aes-twofish', ['twofish', 'aes'], 'sha512', 1, 17, 'AES-Twofish')
    yield plain_volume('keyfiles', ['aes'], 'sha512', 1, 19, 'AES', keyfiles=('photo', 'notes'), prf_filter='sha512')
    yield plain_volume('keyfile-only', ['aes'], 'sha512', 1, 21, 'AES', password=b'', keyfiles=('notes',),
                       prf_filter='sha512')
    yield plain_volume('long-password-keyfile', ['aes'], 'sha256', 1, 23, 'AES', password=LONG_PASSWORD,
                       keyfiles=('photo',), prf_filter='sha256')
    yield plain_volume('sector-4096', ['aes'], 'sha512', 1, 25, 'AES', sector=4096, prf_filter='sha512')
    yield plain_volume('in-place', ['aes'], 'sha512', 1, 27, 'AES', flags=2, prf_filter='sha512')

    # Unfinished non-system in-place encryption is refused.
    master = master_key(29)
    container = Container('in-place-unfinished', 131072 + 8192)
    fields = {'volume': 8192, 'start': 131072, 'length': 4096, 'flags': 2, 'master': master}
    container.put(0, header_sector(fields, ['aes'], 'sha512', PASSWORD, 16000, 29))
    container.check(request(prf='sha512'), {'error': 'unsupported'})
    yield container

    # An outer volume with a hidden volume inside its data area.
    outer, hidden = plaintext(65536, 31), plaintext(8192, 33)
    container = Container('hidden', 131072 + 65536 + 131072)
    outer_master, hidden_master = master_key(31), master_key(33)
    container.put(0, header_sector({'volume': 65536, 'start': 131072, 'master': outer_master},
                                   ['aes'], 'sha512', PASSWORD, 16000, 31))
    hidden_fields = {'volume': 8192, 'start': 131072 + 32768, 'hidden': 8192, 'master': hidden_master}
    container.put(65536, header_sector(hidden_fields, ['aes'], 'sha512', HIDDEN_PASSWORD, 16000, 33))
    outer_cipher = bytearray(xts(['aes'], outer_master, outer, 131072 // 512))
    outer_cipher[32768:32768 + 8192] = xts(['aes'], hidden_master, hidden, (131072 + 32768) // 512)
    container.put(131072, bytes(outer_cipher))
    container.check(request(HIDDEN_PASSWORD, prf='sha512'),
                    opened('VeraCrypt', 'sha512', 16000, 'AES', 131072 + 32768, 8192, 33, 'hidden'))
    container.check(request(HIDDEN_PASSWORD, prf='sha512', header='normal'), {'error': 'authentication'})
    container.check(request(prf='sha512'), {'flavor': 'VeraCrypt', 'header': 'normal', 'view_start': 131072,
                                            'size': 65536})
    yield container

    # Primary headers wiped; the embedded backups still open both volumes.
    size = 131072 + 16384 + 131072
    container = Container('backup', size)
    data, hidden = plaintext(16384, 35), plaintext(4096, 37)
    master, hidden_master = master_key(35), master_key(37)
    container.put(size - 131072, header_sector({'volume': 16384, 'start': 131072, 'master': master},
                                               ['aes'], 'sha512', PASSWORD, 16000, 35))
    container.put(size - 65536, header_sector({'volume': 4096, 'start': 131072 + 8192, 'hidden': 4096,
                                               'master': hidden_master},
                                              ['aes'], 'sha512', HIDDEN_PASSWORD, 16000, 37))
    sealed = bytearray(xts(['aes'], master, data, 131072 // 512))
    sealed[8192:12288] = xts(['aes'], hidden_master, hidden, (131072 + 8192) // 512)
    container.put(131072, bytes(sealed))
    container.check(request(prf='sha512'), {'error': 'authentication'})
    # The hidden volume occupies the outer data from 8 KiB on.
    container.check(request(prf='sha512', header='backup'),
                    opened('VeraCrypt', 'sha512', 16000, 'AES', 131072, 16384, 35, 'backupNormal', compare=(0, 8192)))
    container.check(request(HIDDEN_PASSWORD, prf='sha512', header='backup'),
                    opened('VeraCrypt', 'sha512', 16000, 'AES', 131072 + 8192, 4096, 37, 'backupHidden'))
    yield container

    # TrueCrypt 7: 'TRUE', its own PBKDF2 counts and no PIM.
    for name, prf, iterations, chain, cipher, seed in [
        ('truecrypt-ripemd160-aes', 'ripemd160', 2000, ['aes'], 'AES', 39),
        ('truecrypt-whirlpool-twofish-aes', 'whirlpool', 1000, ['twofish', 'aes'], 'AES-Twofish', 41),
    ]:
        data, master = plaintext(8192, seed), master_key(seed)
        container = Container(name, 131072 + 8192)
        fields = {'magic': b'TRUE', 'required': 0x0700, 'volume': 8192, 'start': 131072, 'master': master}
        container.put(0, header_sector(fields, chain, prf, PASSWORD, iterations, seed))
        container.put(131072, xts(chain, master, data, 131072 // 512))
        container.check(request(pim=0, prf=prf), opened('TrueCrypt', prf, iterations, cipher, 131072, 8192, seed))
        yield container

    # TrueCrypt 5: header version 3 (no field checksum), data right after the
    # 512-byte header, the hidden header 1536 bytes before the end.
    size = 32768
    container = Container('truecrypt-5-layout', size)
    data, hidden = plaintext(size - 512, 43), plaintext(4096, 45)
    master, hidden_master = master_key(43), master_key(45)
    old = {'magic': b'TRUE', 'version': 3, 'required': 0x0500, 'volume': 0, 'start': 0, 'length': 0}
    container.put(0, header_sector(dict(old, master=master), ['aes'], 'ripemd160', PASSWORD, 2000, 43))
    sealed = bytearray(xts(['aes'], master, data, 1))
    hidden_start = size - 1536 - 4096
    sealed[hidden_start - 512:hidden_start - 512 + 4096] = xts(['aes'], hidden_master, hidden, hidden_start // 512)
    legacy_header = size - 1536 - 512
    sealed[legacy_header:legacy_header + 512] = header_sector(dict(old, master=hidden_master, hidden=4096),
                                                              ['aes'], 'ripemd160', HIDDEN_PASSWORD, 2000, 45)
    assert len(sealed) == size - 512
    container.put(512, bytes(sealed))
    # The hidden volume and its header sit in the outer volume's free space.
    container.check(request(pim=0, prf='ripemd160'),
                    opened('TrueCrypt', 'ripemd160', 2000, 'AES', 512, size - 512, 43, compare=(0, hidden_start - 512)))
    container.check(request(HIDDEN_PASSWORD, pim=0, prf='ripemd160'),
                    opened('TrueCrypt', 'ripemd160', 2000, 'AES', hidden_start, 4096, 45, 'legacyHidden'))
    yield container

    # System encryption: the header in sector 62 of the drive, boot counts,
    # absolute data unit numbers.
    for name, prf, seed, encrypted, plain_boot in [
        ('system-sha256', 'sha256', 47, 16384, False),
        ('system-blake2s-paused', 'blake2s', 49, 4096, True),
    ]:
        drive_size, start, length = 65536 + 16384 + 4096, 65536, 16384
        data, master = plaintext(length, seed), master_key(seed)
        container = Container(name, drive_size)
        container.put(0, b'\x55\xaa')
        fields = {'volume': length, 'start': start, 'length': encrypted, 'flags': 1, 'master': master}
        container.put(31744, header_sector(fields, ['aes'], prf, PASSWORD, boot_iterations(prf, 1), seed))
        stored = bytearray(xts(['aes'], master, data[:encrypted], start // 512) + data[encrypted:])
        if plain_boot:
            stored[:8] = b'\xebR\x90NTFS '
        container.put(start, bytes(stored))
        partition = {'offset': start, 'length': length}
        # A plaintext NTFS boot sector reads as stored, as VeraCrypt does.
        expect = opened('VeraCrypt', prf, 2048, 'AES', start, length, seed, 'system', complete=encrypted == length,
                        plaintext_boot_sector=plain_boot, compare=(512 if plain_boot else 0, length))
        container.check(request(header='system', system=partition), expect)
        container.check(request(header='system', system='drive', prf=prf),
                        {'flavor': 'VeraCrypt', 'header': 'system', 'view_start': 0, 'size': drive_size})
        container.check(request(header='system', system={'offset': 0, 'length': start}, prf=prf),
                        {'error': 'corrupt'})
        yield container

    # TrueCrypt system encryption: RIPEMD-160 at 1000 iterations.
    container = Container('truecrypt-system', 65536 + 8192)
    data, master = plaintext(8192, 51), master_key(51)
    fields = {'magic': b'TRUE', 'required': 0x0500, 'volume': 8192, 'start': 65536, 'flags': 1, 'master': master}
    container.put(31744, header_sector(fields, ['aes'], 'ripemd160', PASSWORD, 1000, 51))
    container.put(65536, xts(['aes'], master, data, 65536 // 512))
    container.check(request(pim=0, prf='ripemd160', header='system', system={'offset': 65536, 'length': 8192}),
                    opened('TrueCrypt', 'ripemd160', 1000, 'AES', 65536, 8192, 51, 'system'))
    yield container

    # Refusals: a newer header version and a data area past the source.
    container = Container('header-version-6', 131072 + 4096)
    fields = {'version': 6, 'volume': 4096, 'start': 131072, 'master': master_key(53)}
    container.put(0, header_sector(fields, ['aes'], 'sha512', PASSWORD, 16000, 53))
    container.check(request(prf='sha512'), {'error': 'unsupported'})
    container.check(request(b'wrong', prf='sha512'), {'error': 'authentication'})
    yield container
    container = Container('truncated', 131072 + 4096)
    fields = {'volume': 8192, 'start': 131072, 'master': master_key(55)}
    container.put(0, header_sector(fields, ['aes'], 'sha512', PASSWORD, 16000, 55))
    container.check(request(prf='sha512'), {'error': 'corrupt'})
    yield container


if __name__ == '__main__':
    check_xts()
    corpus = {
        'password': PASSWORD.decode(),
        'keyfiles': {name: base64.b64encode(data).decode() for name, data in KEYFILES.items()},
        'containers': [container.json() for container in containers()],
    }
    DEST.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(corpus, ensure_ascii=False, indent=1) + '\n'
    DEST.write_bytes(text.encode('utf-8'))
    print(f'{DEST}: {len(corpus["containers"])} containers, sha256 {hashlib.sha256(text.encode("utf-8")).hexdigest()}')
