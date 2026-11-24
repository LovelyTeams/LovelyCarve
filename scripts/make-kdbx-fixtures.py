"""Write the independent KDBX compatibility corpus for the kdbx reader.

Small synthetic KeePass databases (KDBX 3.0, 3.1, 4.0, 4.1) built here from
the format rules with Python's hashlib/hmac/gzip, `cryptography` (AES,
ChaCha20, Argon2id with a secret and associated data), `argon2-cffi`
(Argon2d, versions 0x10 / 0x13), PyCryptodome (Salsa20 and ChaCha20 inner
streams) and the `twofish` package; the Rust reader is not used. All
passwords and keys are public test data.

Run from the repository root, then `just test-kdbx`:

    python scripts/make-kdbx-fixtures.py
"""
import base64
import gzip
import hashlib
import hmac
import json
from pathlib import Path
import struct
from xml.sax.saxutils import escape, quoteattr

from argon2.low_level import Type, hash_secret_raw
from Cryptodome.Cipher import ChaCha20 as CryptodomeChaCha20, Salsa20
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id
from twofish import Twofish

DEST = Path(__file__).resolve().parents[1] / 'src-tauri/vendor/kdbx/tests/data/synthetic.json'
SIG = struct.pack('<II', 0x9AA2D903, 0xB54BFB67)
CIPHERS = {
    'AES-256': bytes.fromhex('31c1f2e6bf714350be5805216afc5aff'),
    'Twofish': bytes.fromhex('ad68f29f576f4bb9a36ad47af965346c'),
    'ChaCha20': bytes.fromhex('d6038a2b8b6f4cb5a524339a31dbb59a'),
}
KDFS = {
    'AES-KDF': bytes.fromhex('c9d9f39a628a4460bf740d08c18a4fea'),
    'Argon2d': bytes.fromhex('ef636ddf8c29444b91f7a9a403e30a0c'),
    'Argon2id': bytes.fromhex('9e298b1956db4773b23dfc3ec6f0a1e6'),
}
STREAMS = {'none': 0, 'Salsa20': 2, 'ChaCha20': 3}
EPOCH = 62135596800


def pattern(length, seed):
    return bytes((index * 31 + seed) % 256 for index in range(length))


def sha256(data):
    return hashlib.sha256(data).digest()


# ── keys ─────────────────────────────────────────────────────────────────────

def keyfile_data(kind, seed):
    """(file contents, key material) for one keyfile form."""
    key = pattern(32, seed)
    if kind == 'xml1':
        text = ('<?xml version="1.0" encoding="utf-8"?>\n<KeyFile>\n\t<Meta>\n\t\t<Version>1.00</Version>\n'
                f'\t</Meta>\n\t<Key>\n\t\t<Data>{base64.b64encode(key).decode()}</Data>\n\t</Key>\n</KeyFile>\n')
        return text.encode(), key
    if kind == 'xml2':
        hex_key = key.hex().upper()
        groups = ' '.join(hex_key[index:index + 8] for index in range(0, 64, 8))
        text = ('<?xml version="1.0" encoding="utf-8"?>\n<KeyFile>\n\t<Meta>\n\t\t<Version>2.0</Version>\n'
                f'\t</Meta>\n\t<Key>\n\t\t<Data Hash="{sha256(key)[:4].hex().upper()}">\n\t\t\t{groups}\n'
                '\t\t</Data>\n\t</Key>\n</KeyFile>\n')
        return text.encode(), key
    if kind == 'binary32':
        return key, key
    if kind == 'hashed':
        contents = pattern(100, seed)
        return contents, sha256(contents)
    raise ValueError(kind)


def composite_key(password, keyfile_key):
    parts = b''
    if password is not None:
        parts += sha256(password.encode())
    if keyfile_key is not None:
        parts += keyfile_key
    return sha256(parts)


def aes_kdf(composite, seed, rounds):
    encryptor = Cipher(algorithms.AES(seed), modes.ECB()).encryptor()
    key = composite
    for _ in range(rounds):
        key = encryptor.update(key)
    return sha256(key)


def transform(composite, kdf):
    if kdf['name'] == 'AES-KDF':
        return aes_kdf(composite, kdf['seed'], kdf['rounds'])
    if kdf.get('secret') or kdf.get('data'):
        assert kdf['name'] == 'Argon2id' and kdf['version'] == 0x13
        return Argon2id(salt=kdf['salt'], length=32, iterations=kdf['iterations'],
                        lanes=kdf['parallelism'], memory_cost=kdf['memory'] // 1024,
                        ad=kdf.get('data'), secret=kdf.get('secret')).derive(composite)
    kind = Type.D if kdf['name'] == 'Argon2d' else Type.ID
    return hash_secret_raw(composite, kdf['salt'], time_cost=kdf['iterations'],
                           memory_cost=kdf['memory'] // 1024, parallelism=kdf['parallelism'],
                           hash_len=32, type=kind, version=kdf['version'])


# ── ciphers and streams ─────────────────────────────────────────────────────

def pkcs7(data):
    padding = 16 - len(data) % 16
    return data + bytes([padding]) * padding


def encrypt(cipher, key, iv, data):
    if cipher == 'AES-256':
        encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
        return encryptor.update(pkcs7(data)) + encryptor.finalize()
    if cipher == 'Twofish':
        block_cipher = Twofish(key)
        out, previous = bytearray(), iv
        padded = pkcs7(data)
        for index in range(0, len(padded), 16):
            block = bytes(a ^ b for a, b in zip(padded[index:index + 16], previous))
            previous = block_cipher.encrypt(block)
            out += previous
        return bytes(out)
    if cipher == 'ChaCha20':
        encryptor = Cipher(algorithms.ChaCha20(key, b'\0\0\0\0' + iv), None).encryptor()
        return encryptor.update(data)
    raise ValueError(cipher)


class InnerStream:
    def __init__(self, name, key):
        self.name = name
        if name == 'Salsa20':
            self.cipher = Salsa20.new(key=sha256(key), nonce=bytes.fromhex('e830094b97205d2a'))
        elif name == 'ChaCha20':
            digest = hashlib.sha512(key).digest()
            self.cipher = CryptodomeChaCha20.new(key=digest[:32], nonce=digest[32:44])
        else:
            self.cipher = None

    def protect(self, data):
        return data if self.cipher is None else self.cipher.encrypt(data)


# ── XML ──────────────────────────────────────────────────────────────────────

def element(tag, *children, text=None, protect=False, **attributes):
    return {'tag': tag, 'children': list(children), 'text': text, 'protect': protect, 'attributes': attributes}


def serialize(node, stream, depth=0):
    indent = '\t' * depth
    attributes = ''.join(f' {key}={quoteattr(value)}' for key, value in node['attributes'].items())
    text = node['text']
    if node['protect']:
        raw = text.encode() if isinstance(text, str) else (text or b'')
        attributes += ' Protected="True"'
        text = base64.b64encode(stream.protect(raw)).decode()
    if node['children']:
        inner = ''.join(serialize(child, stream, depth + 1) for child in node['children'])
        return f'{indent}<{node["tag"]}{attributes}>\n{inner}{indent}</{node["tag"]}>\n'
    if text is None or text == '':
        return f'{indent}<{node["tag"]}{attributes} />\n'
    return f'{indent}<{node["tag"]}{attributes}>{escape(text)}</{node["tag"]}>\n'


def uuid_text(seed):
    return base64.b64encode(pattern(16, seed)).decode()


def uuid_hex(seed):
    return pattern(16, seed).hex().upper()


def time_text(unix, kdbx4):
    if kdbx4:
        return base64.b64encode(struct.pack('<q', unix + EPOCH)).decode()
    import datetime
    return datetime.datetime.fromtimestamp(unix, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')


def times(unix, kdbx4, expires=False):
    return element('Times',
                   element('CreationTime', text=time_text(unix, kdbx4)),
                   element('LastModificationTime', text=time_text(unix + 60, kdbx4)),
                   element('LastAccessTime', text=time_text(unix + 120, kdbx4)),
                   element('ExpiryTime', text=time_text(unix + 3600, kdbx4)),
                   element('Expires', text='True' if expires else 'False'),
                   element('UsageCount', text='2'),
                   element('LocationChanged', text=time_text(unix, kdbx4)))


def string_field(key, value, protect=False):
    return element('String', element('Key', text=key), element('Value', text=value, protect=protect))


def entry_xml(spec, kdbx4, v41, history=()):
    children = [element('UUID', text=uuid_text(spec['uuid'])), element('IconID', text='0')]
    if v41:
        children.append(element('QualityCheck', text='False'))
        children.append(element('PreviousParentGroup', text=uuid_text(250)))
    children.append(element('Tags', text=';'.join(spec.get('tags', []))))
    children.append(times(spec['time'], kdbx4, spec.get('expires', False)))
    for key, value, protect in spec['fields']:
        children.append(string_field(key, value, protect))
    for name, reference in spec.get('attachments', []):
        children.append(element('Binary', element('Key', text=name), element('Value', Ref=str(reference))))
    children.append(element('AutoType', element('Enabled', text='True'),
                            element('DataTransferObfuscation', text='0'),
                            element('Association', element('Window', text='Login*'),
                                    element('KeystrokeSequence', text='{PASSWORD}{ENTER}'))))
    if history:
        children.append(element('History', *[entry_xml(version, kdbx4, v41) for version in history]))
    return element('Entry', *children)


def group_xml(spec, kdbx4, v41):
    children = [element('UUID', text=uuid_text(spec['uuid'])), element('Name', text=spec['name']),
                element('Notes', text=spec.get('notes', '')), element('IconID', text='48'),
                times(spec.get('time', 1_600_000_000), kdbx4),
                element('IsExpanded', text='True'), element('EnableAutoType', text='null'),
                element('EnableSearching', text='null')]
    if v41:
        children.append(element('Tags', text='case;7'))
    for entry in spec.get('entries', []):
        children.append(entry_xml(entry, kdbx4, v41, entry.get('history', ())))
    for group in spec.get('groups', []):
        children.append(group_xml(group, kdbx4, v41))
    return element('Group', *children)


def document(case, header_hash, stream):
    kdbx4, v41 = case['major'] >= 4, case['major'] == 4 and case['minor'] >= 1
    meta = [element('Generator', text='LovelyCarve fixture'),
            element('DatabaseName', text=case['name_meta']),
            element('DatabaseNameChanged', text=time_text(1_600_000_000, kdbx4))]
    if header_hash is not None:
        meta.insert(1, element('HeaderHash', text=base64.b64encode(header_hash).decode()))
    meta.append(element('MemoryProtection', element('ProtectPassword', text='True')))
    if v41:
        meta.append(element('CustomIcons', element('Icon', element('UUID', text=uuid_text(240)),
                                                   element('Data', text=base64.b64encode(b'\x89PNG fake').decode()),
                                                   element('Name', text='custom'),
                                                   element('LastModificationTime', text=time_text(1_600_000_000, True)))))
    recycle = case.get('recycle_bin')
    meta.append(element('RecycleBinEnabled', text='True' if recycle else 'False'))
    meta.append(element('RecycleBinUUID', text=uuid_text(recycle) if recycle else base64.b64encode(bytes(16)).decode()))
    if not kdbx4 and case.get('xml_binaries'):
        binaries = []
        for identifier, data, mode in case['xml_binaries']:
            if mode == 'protected':
                binaries.append(element('Binary', text=data, protect=True, ID=str(identifier)))
            elif mode == 'compressed':
                binaries.append(element('Binary', text=base64.b64encode(gzip.compress(data, mtime=0)).decode(),
                                        ID=str(identifier), Compressed='True'))
            else:
                binaries.append(element('Binary', text=base64.b64encode(data).decode(), ID=str(identifier)))
        meta.append(element('Binaries', *binaries))
    item = [element('Key', text='lovelycarve'), element('Value', text='fixture')]
    if v41:
        item.append(element('LastModificationTime', text=time_text(1_600_000_000, True)))
    meta.append(element('CustomData', element('Item', *item)))
    deleted = [element('DeletedObject', element('UUID', text=uuid_text(seed)),
                       element('DeletionTime', text=time_text(1_600_000_500, kdbx4)))
               for seed in case.get('deleted', [])]
    root = element('KeePassFile', element('Meta', *meta),
                   element('Root', group_xml(case['root'], kdbx4, v41), element('DeletedObjects', *deleted)))
    return ('<?xml version="1.0" encoding="utf-8" standalone="yes"?>\n' + serialize(root, stream)).encode()


# ── containers ───────────────────────────────────────────────────────────────

def variant_dictionary(items):
    out = struct.pack('<H', 0x0100)
    for kind, key, value in items:
        out += bytes([kind]) + struct.pack('<i', len(key)) + key.encode() + struct.pack('<i', len(value)) + value
    return out + b'\0'


def kdf_dictionary(kdf):
    items = [(0x42, '$UUID', KDFS[kdf['name']])]
    if kdf['name'] == 'AES-KDF':
        items += [(0x05, 'R', struct.pack('<Q', kdf['rounds'])), (0x42, 'S', kdf['seed'])]
    else:
        items += [(0x42, 'S', kdf['salt']), (0x04, 'P', struct.pack('<I', kdf['parallelism'])),
                  (0x05, 'M', struct.pack('<Q', kdf['memory'])), (0x05, 'I', struct.pack('<Q', kdf['iterations'])),
                  (0x04, 'V', struct.pack('<I', kdf['version']))]
        if kdf.get('secret'):
            items.append((0x42, 'K', kdf['secret']))
        if kdf.get('data'):
            items.append((0x42, 'A', kdf['data']))
    return variant_dictionary(items)


def kdbx3_file(case, composite):
    seed = case['seed']
    master_seed, transform_seed = pattern(32, seed + 1), pattern(32, seed + 2)
    iv = pattern(12 if case['cipher'] == 'ChaCha20' else 16, seed + 3)
    stream_key, start = pattern(32, seed + 4), pattern(32, seed + 5)
    rounds = case['kdf']['rounds']

    def field(identifier, data):
        return bytes([identifier]) + struct.pack('<H', len(data)) + data

    header = SIG + struct.pack('<HH', case['minor'], case['major'])
    header += field(2, CIPHERS[case['cipher']]) + field(3, struct.pack('<I', 1 if case['gzip'] else 0))
    header += field(4, master_seed) + field(5, transform_seed) + field(6, struct.pack('<Q', rounds))
    header += field(7, iv) + field(8, stream_key) + field(9, start)
    header += field(10, struct.pack('<I', STREAMS[case['stream']])) + field(0, b'\r\n\r\n')

    transformed = aes_kdf(composite, transform_seed, rounds)
    master = sha256(master_seed + transformed)
    hash_in_meta = sha256(header) if case['minor'] >= 1 else None
    xml = document(case, hash_in_meta, InnerStream(case['stream'], stream_key))
    payload = gzip.compress(xml, mtime=0) if case['gzip'] else xml
    blocks, size = b'', case['block_size']
    chunks = [payload[index:index + size] for index in range(0, len(payload), size)]
    for index, chunk in enumerate(chunks):
        blocks += struct.pack('<I', index) + sha256(chunk) + struct.pack('<i', len(chunk)) + chunk
    blocks += struct.pack('<I', len(chunks)) + bytes(32) + struct.pack('<i', 0)
    return header + encrypt(case['cipher'], master, iv, start + blocks), transformed


def kdbx4_file(case, composite):
    seed = case['seed']
    master_seed = pattern(32, seed + 1)
    iv = pattern(12 if case['cipher'] == 'ChaCha20' else 16, seed + 3)
    stream_key = pattern(64, seed + 4)

    def field(identifier, data):
        return bytes([identifier]) + struct.pack('<i', len(data)) + data

    header = SIG + struct.pack('<HH', case['minor'], case['major'])
    header += field(2, CIPHERS[case['cipher']]) + field(3, struct.pack('<I', 1 if case['gzip'] else 0))
    header += field(4, master_seed) + field(7, iv) + field(11, kdf_dictionary(case['kdf']))
    if case.get('public_custom_data'):
        header += field(12, variant_dictionary([(0x18, 'plugin', 'value'.encode()), (0x04, 'n', struct.pack('<I', 7))]))
    if case.get('unknown_field'):
        header += field(42, b'?')
    header += field(0, b'\r\n\r\n')

    transformed = transform(composite, case['kdf'])
    master = sha256(master_seed + transformed)
    hmac_key = hashlib.sha512(master_seed + transformed + b'\x01').digest()

    def block_key(index):
        return hashlib.sha512(struct.pack('<Q', index) + hmac_key).digest()

    header_hmac = hmac.new(block_key(2 ** 64 - 1), header, hashlib.sha256).digest()
    inner = bytes([1]) + struct.pack('<i', 4) + struct.pack('<I', STREAMS[case['stream']])
    inner += bytes([2]) + struct.pack('<i', len(stream_key)) + stream_key
    for data, protected in case.get('inner_binaries', []):
        inner += bytes([3]) + struct.pack('<i', len(data) + 1) + bytes([1 if protected else 0]) + data
    inner += bytes([0]) + struct.pack('<i', 0)
    plain = inner + document(case, None, InnerStream(case['stream'], stream_key))
    payload = encrypt(case['cipher'], master, iv, gzip.compress(plain, mtime=0) if case['gzip'] else plain)
    blocks, size = b'', case['block_size']
    chunks = [payload[index:index + size] for index in range(0, len(payload), size)] + [b'']
    for index, chunk in enumerate(chunks):
        mac = hmac.new(block_key(index), struct.pack('<Q', index) + struct.pack('<i', len(chunk)) + chunk,
                       hashlib.sha256).digest()
        blocks += mac + struct.pack('<i', len(chunk)) + chunk
    return header + sha256(header) + header_hmac + blocks, transformed


# ── the corpus ───────────────────────────────────────────────────────────────

def entry(uuid, title, username, password, time, *, fields=(), tags=(), attachments=(), history=(), expires=False):
    base = [('Title', title, False), ('UserName', username, False), ('Password', password, True),
            ('URL', f'https://{title.lower()}.example', False), ('Notes', f'{title} 备注\nline 2', False)]
    return {'uuid': uuid, 'time': time, 'fields': base + list(fields), 'tags': list(tags),
            'attachments': list(attachments), 'history': list(history), 'expires': expires}


def tree(protected_pin):
    old = entry(11, 'Mail', 'alice', 'old-密码', 1_500_000_000)
    older = entry(11, 'Mail', 'alice', 'older', 1_400_000_000)
    return {
        'uuid': 1, 'name': 'Root', 'notes': 'root notes', 'time': 1_400_000_000,
        'entries': [entry(10, 'Bank', 'bob', 'bank-密码', 1_600_000_000,
                          fields=[('PIN', '1234', protected_pin), ('otp', 'otpauth://totp/x?secret=ABC', True)],
                          tags=['money', '银行'], attachments=[('statement.pdf', 0), ('photo.jpg', 1)])],
        'groups': [
            {'uuid': 2, 'name': 'Email', 'entries': [entry(11, 'Mail', 'alice', 'mail-密码', 1_600_000_100,
                                                           history=[older, old], expires=True)],
             'groups': [{'uuid': 3, 'name': 'Archive', 'entries': [
                 entry(12, 'Ref', '{REF:U@I:' + uuid_hex(11) + '}', '{REF:P@I:' + uuid_hex(11) + '}', 1_600_000_200)]}]},
            {'uuid': 4, 'name': 'Recycle Bin', 'entries': [entry(13, 'Gone', 'eve', 'gone', 1_600_000_300)]},
        ],
    }


BINARY_A, BINARY_B = pattern(700, 9), b'photo bytes \x00\xff' * 20
CASES = [
    {'name': 'kdbx31-aes-salsa20-gzip', 'major': 3, 'minor': 1, 'cipher': 'AES-256', 'gzip': True,
     'stream': 'Salsa20', 'kdf': {'name': 'AES-KDF', 'rounds': 64}, 'block_size': 512,
     'password': 'LovelyCarve-密码', 'keyfile': None,
     'xml_binaries': [(0, BINARY_A, 'compressed'), (1, BINARY_B, 'protected')],
     'recycle_bin': 4, 'deleted': [20, 21]},
    {'name': 'kdbx31-twofish-plain-stream', 'major': 3, 'minor': 1, 'cipher': 'Twofish', 'gzip': False,
     'stream': 'none', 'kdf': {'name': 'AES-KDF', 'rounds': 32}, 'block_size': 4096,
     'password': None, 'keyfile': 'xml1',
     'xml_binaries': [(0, BINARY_A, 'plain'), (1, BINARY_B, 'protected')]},
    {'name': 'kdbx30-chacha20-chacha20', 'major': 3, 'minor': 0, 'cipher': 'ChaCha20', 'gzip': True,
     'stream': 'ChaCha20', 'kdf': {'name': 'AES-KDF', 'rounds': 16}, 'block_size': 300,
     'password': 'pw', 'keyfile': 'binary32',
     'xml_binaries': [(0, BINARY_A, 'protected'), (1, BINARY_B, 'compressed')]},
    {'name': 'kdbx40-aeskdf-twofish', 'major': 4, 'minor': 0, 'cipher': 'Twofish', 'gzip': True,
     'stream': 'ChaCha20', 'kdf': {'name': 'AES-KDF', 'rounds': 64, 'seed': pattern(32, 77)}, 'block_size': 256,
     'password': 'LovelyCarve-密码', 'keyfile': 'xml2', 'public_custom_data': True,
     'inner_binaries': [(BINARY_A, True), (BINARY_B, False)], 'deleted': [22]},
    {'name': 'kdbx41-argon2d-chacha20', 'major': 4, 'minor': 1, 'cipher': 'ChaCha20', 'gzip': True,
     'stream': 'ChaCha20', 'kdf': {'name': 'Argon2d', 'salt': pattern(32, 78), 'parallelism': 2,
                                   'memory': 64 * 1024, 'iterations': 2, 'version': 0x13},
     'block_size': 1 << 20, 'password': 'LovelyCarve-密码', 'keyfile': None, 'unknown_field': True,
     'inner_binaries': [(BINARY_A, False), (BINARY_B, True)], 'recycle_bin': 4},
    {'name': 'kdbx40-argon2id-secret-salsa20', 'major': 4, 'minor': 0, 'cipher': 'AES-256', 'gzip': False,
     'stream': 'Salsa20', 'kdf': {'name': 'Argon2id', 'salt': pattern(16, 79), 'parallelism': 1,
                                  'memory': 32 * 1024, 'iterations': 3, 'version': 0x13,
                                  'secret': b'pepper-secret', 'data': b'associated'},
     'block_size': 1024, 'password': None, 'keyfile': 'hashed',
     'inner_binaries': [(BINARY_A, False), (BINARY_B, False)]},
    {'name': 'kdbx40-argon2d-v10-empty-password', 'major': 4, 'minor': 0, 'cipher': 'AES-256', 'gzip': True,
     'stream': 'ChaCha20', 'kdf': {'name': 'Argon2d', 'salt': pattern(16, 80), 'parallelism': 1,
                                   'memory': 16 * 1024, 'iterations': 2, 'version': 0x10},
     'block_size': 1 << 20, 'password': '', 'keyfile': None,
     'inner_binaries': [(BINARY_A, True), (BINARY_B, True)]},
]


def expectations(case):
    kdbx4 = case['major'] >= 4

    def entry_view(spec, path):
        fields = {key: [value, protect] for key, value, protect in spec['fields']}
        return {'path': path, 'title': spec['fields'][0][1], 'fields': fields, 'tags': spec['tags'],
                'attachments': [name for name, _ in spec['attachments']],
                'history': len(spec.get('history', [])), 'expires': spec['expires'],
                'created': spec['time']}

    entries, groups = [], []

    def walk(group, path):
        groups.append('/'.join(path))
        for spec in group.get('entries', []):
            entries.append(entry_view(spec, '/'.join(path)))
        for child in group.get('groups', []):
            walk(child, path + [child['name']])

    walk(case['root'], [])
    binaries = case.get('inner_binaries') if kdbx4 else [(data, mode == 'protected') for _, data, mode in case['xml_binaries']]
    return {
        'version': f"{case['major']}.{case['minor']}", 'cipher': case['cipher'], 'kdf': case['kdf']['name'],
        'compression': 'GZip' if case['gzip'] else 'none', 'inner_stream': case['stream'],
        'header_hash': 'matches' if kdbx4 or case['minor'] >= 1 else 'absent',
        'header_warnings': 1 if case.get('unknown_field') else 0,
        'database_name': case['name_meta'], 'groups': groups, 'entries': entries,
        'recycle_bin': 'Recycle Bin' if case.get('recycle_bin') else None,
        'deleted': len(case.get('deleted', [])),
        'binaries': [{'sha256': sha256(data).hex(), 'length': len(data), 'protected': protected}
                     for data, protected in binaries],
        'reference': ['alice', 'mail-密码'],
    }


def main():
    out = []
    for case in CASES:
        case['root'] = tree(protected_pin=case['major'] >= 4)
        case['name_meta'] = f"{case['name']} 数据库"
        if case['kdf']['name'] == 'AES-KDF' and 'seed' not in case['kdf']:
            case['kdf']['seed'] = pattern(32, case['name'].__len__())
        case['seed'] = len(out) * 10
        keyfile_contents, keyfile_key = keyfile_data(case['keyfile'], 60 + len(out)) if case['keyfile'] else (None, None)
        composite = composite_key(case['password'], keyfile_key)
        if case['major'] >= 4:
            data, transformed = kdbx4_file(case, composite)
        else:
            data, transformed = kdbx3_file(case, composite)
        out.append({
            'name': case['name'],
            'file': base64.b64encode(data).decode(),
            'password': case['password'],
            'keyfile': base64.b64encode(keyfile_contents).decode() if keyfile_contents else None,
            'transformed_key': transformed.hex(),
            'expect': expectations(case),
        })
    DEST.write_text(json.dumps({'cases': out}, ensure_ascii=False, indent=1) + '\n', encoding='utf-8', newline='\n')
    print(f'{len(out)} databases -> {DEST}')
    print(hashlib.sha256(DEST.read_bytes()).hexdigest())


if __name__ == '__main__':
    main()
