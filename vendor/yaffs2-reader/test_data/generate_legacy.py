"""Original CC0-1.0 YAFFS1 fixtures. Python stdlib, no production encoder."""
import hashlib
import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = b"Native YAFFS1 data.\n" * 61


def ecc(data):
    # Aggregate independent mask parity, then interleave row coordinates.
    column = sum((sum((v & mask).bit_count() for v in data) % 2) << (bit + 2)
                 for bit, mask in enumerate([0x55, 0xaa, 0x33, 0xcc, 0x0f, 0xf0]))
    row = complement = 0
    for index, value in enumerate(data):
        if value.bit_count() % 2:
            row ^= index
            complement ^= index ^ 255
    low = high = 0
    for axis in range(8):
        pair = ((row >> axis) & 1) * 2 + ((complement >> axis) & 1)
        if axis < 4:
            low |= pair << (axis * 2)
        else:
            high |= pair << ((axis - 4) * 2)
    return bytes([low ^ 255, high ^ 255, (column ^ 255) | 3])


class Encoder:
    def __init__(self, order):
        self.order = order
        self.pages = []

    def add(self, body, object_id, chunk=0, serial=0, deleted=False):
        count = len(body) if chunk else 1023
        body = body.ljust(512, b'\xff')
        assert len(body) == 512
        if self.order == '<':
            words = [chunk | serial << 20 | count << 22, object_id | 3 << 30]
            shift = 18
        else:
            words = [chunk << 12 | serial << 10 | count, object_id << 14 | 3]
            shift = 2
        raw = struct.pack(self.order + 'II', *words)
        checksum = 0
        for bit in range(64):
            if raw[bit // 8] & (1 << (bit % 8)):
                checksum ^= bit + 1
        words[1] |= checksum << shift
        tags = struct.pack(self.order + 'II', *words)
        spare = bytearray(b'\xff' * 16)
        for index, position in enumerate([0, 1, 2, 3, 6, 7, 11, 12]):
            spare[position] = tags[index]
        spare[8:11] = ecc(body[:256])
        spare[13:16] = ecc(body[256:])
        if deleted:
            spare[4] = 0
        self.pages.append(body + spare)

    def header(self, number, name, kind=1, parent=1, size=0, equivalent=0, alias='', serial=0, shadow=0):
        b = bytearray(b'\xff' * 512)
        struct.pack_into(self.order + 'II', b, 0, kind, parent)
        name = name.encode() + b'\0'
        b[10:10+len(name)] = name
        if kind != 4:
            mode = {1: 0o100644, 2: 0o120777, 3: 0o40755, 5: 0o20600}[kind]
            struct.pack_into(self.order + 'IIIIIII', b, 268, mode, 1000, 1001, 1700000000, 1700000000, 1700000000, size)
        if kind == 4:
            struct.pack_into(self.order + 'I', b, 296, equivalent)
        if kind == 2:
            alias = alias.encode() + b'\0'
            b[300:300+len(alias)] = alias
        if shadow:
            struct.pack_into(self.order + 'I', b, 504, shadow)
        self.add(bytes(b), number, serial=serial)


manifest = {'license': 'CC0-1.0', 'generator': 'Python stdlib independent YAFFS1 encoder', 'payloadSha256': hashlib.sha256(DATA).hexdigest(), 'files': {}}


def save(name, encoder):
    data = b''.join(encoder.pages)
    (ROOT / name).write_bytes(data)
    manifest['files'][name] = {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'pageSize': 512, 'spareSize': 16, 'endian': encoder.order}


for order, name in [('<', 'legacy-le.yaffs'), ('>', 'legacy-be.yaffs')]:
    e = Encoder(order)
    e.header(1, '', kind=3)
    e.header(256, '目录', kind=3)
    e.header(257, 'message', parent=256, size=len(DATA))
    for start in range(0, len(DATA), 512):
        e.add(DATA[start:start+512], 257, start // 512 + 1)
    e.header(258, 'alias', kind=4, equivalent=257)
    e.header(259, 'sym', kind=2, alias='目录/message')
    e.header(260, 'empty')
    e.header(261, 'console', kind=5)
    e.header(262, 'gone', parent=4)
    e.add(b'deleted stale content', 263, 1, deleted=True)
    e.add(b'orphan', 264, 1)
    save(name, e)

e = Encoder('<')
e.header(257, 'old', size=1200, serial=3)
e.add(b'A' * 512, 257, 1, serial=3)
e.add(b'B' * 512, 257, 2, deleted=True)
e.add(b'C' * 176, 257, 3, deleted=True)
e.header(257, 'current', size=800, serial=0)
e.add(b'X' * 512, 257, 1, serial=0)
e.add(b'y' * 100, 257, 2, serial=1)
e.add(b'Z' * 512, 257, 1, serial=0)  # same serial retains earlier page
e.header(300, 'replacement', size=3)
e.add(b'old', 300, 1)
e.header(257, 'replacement', size=800, serial=1, shadow=300)
save('legacy-versions.yaffs', e)
(ROOT / 'legacy-fixtures.json').write_text(json.dumps(manifest, indent=2) + '\n')
