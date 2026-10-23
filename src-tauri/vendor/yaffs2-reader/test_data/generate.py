"""Independent Python YAFFS2 encoder. Original CC0-1.0 synthetic fixtures."""
import hashlib
import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = b"Native YAFFS2 fixture.\n" * 300


class Encoder:
    def __init__(self, page=2048, spare=64, order="<", offset=0, ecc=False):
        self.page, self.spare, self.order, self.offset, self.ecc = page, spare, order, offset, ecc
        self.pages = []

    def add(self, body, object_id, chunk=0, count=65535, sequence=0x1000, extra=None):
        data = body + bytes([255]) * (self.page - len(body))
        if extra is not None:
            kind, parent, size, shrink, shadow = extra
            object_id |= kind << 28
            chunk = 0x80000000 | parent | (0x40000000 if shrink else 0) | (0x20000000 if shadow else 0)
            count = size
        tags = struct.pack(self.order + "IIII", sequence, object_id, chunk, count)
        spare = bytearray([255] * self.spare)
        spare[self.offset:self.offset+16] = tags
        if self.ecc:
            # Independent column-mask parity, with ECC words deliberately LE
            # even when the object's numeric metadata uses BE.
            column = 0
            for bit, mask in enumerate([0x55, 0xaa, 0x33, 0xcc, 0x0f, 0xf0]):
                column |= (sum((value & mask).bit_count() for value in tags) % 2) << bit
            line = 0
            prime = 0
            for index, value in enumerate(tags):
                if value.bit_count() % 2:
                    line ^= index
                    prime ^= (~index) & 0xffffffff
            spare[self.offset+16] = column
            struct.pack_into("<II", spare, self.offset+20, line, prime)
        self.pages.append(data + spare)

    def header(self, number, name, kind=1, parent=1, size=0, equivalent=0, alias="", shrink=False, shadow=0, sequence=0x1000, extra=False, modern=False):
        body = bytearray([255] * 512)
        struct.pack_into(self.order + "II", body, 0, kind, parent)
        name = name.encode() + b"\0"
        body[10:10+len(name)] = name
        if kind != 4:
            mode = {1: 0o100644, 2: 0o120777, 3: 0o40755, 5: 0o20600}[kind]
            timestamp = (1 << 32) + 45678 if modern else 1700000000
            struct.pack_into(self.order + "IIIIIII", body, 268, mode, 1000, 1001, timestamp & 0xffffffff, timestamp & 0xffffffff, timestamp & 0xffffffff, size & 0xffffffff)
            if modern:
                for at in [464, 472, 480]:
                    struct.pack_into(self.order + "II", body, at, timestamp & 0xffffffff, timestamp >> 32)
        if kind == 4:
            struct.pack_into(self.order + "I", body, 296, equivalent)
        if kind == 2:
            link = alias.encode() + b"\0"
            body[300:300+len(link)] = link
        struct.pack_into(self.order + "I", body, 496, size >> 32)
        struct.pack_into(self.order + "II", body, 504, shadow, int(shrink))
        info = (kind, parent, equivalent if kind == 4 else size if kind == 1 else 0, shrink, bool(shadow)) if extra else None
        self.add(body, number, sequence=sequence, extra=info)

    def data(self, number, chunk, data, sequence=0x1000):
        self.add(data, number, chunk, len(data), sequence)

    def bytes(self):
        return b"".join(self.pages)


manifest = {"license": "CC0-1.0", "generator": "Python stdlib struct; no production reader or writer", "payloadSha256": hashlib.sha256(DATA).hexdigest(), "files": {}}


def save(name, encoder):
    data = encoder.bytes()
    (ROOT / name).write_bytes(data)
    manifest["files"][name] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "pageSize": encoder.page, "spareSize": encoder.spare, "tagsOffset": encoder.offset, "endian": encoder.order}


for name, encoder in [("base-le.yaffs", Encoder()), ("base-be-ecc.yaffs", Encoder(4096, 128, ">", 2, True))]:
    encoder.header(256, "目录", kind=3, extra=True)
    encoder.header(257, "hello", parent=256, size=len(DATA), extra=True)
    for index in range(0, len(DATA), encoder.page):
        encoder.data(257, index // encoder.page + 1, DATA[index:index+encoder.page])
    encoder.header(258, "alias", kind=4, equivalent=257, extra=True)
    encoder.header(259, "link", kind=2, alias="目录/hello", extra=True)
    encoder.header(260, "empty", extra=True)
    encoder.header(261, "console", kind=5, extra=True)
    encoder.header(262, "gone", parent=4)
    encoder.pages.append(bytes([255]) * (encoder.page + encoder.spare))
    save(name, encoder)

encoder = Encoder()
encoder.header(257, "renamed", size=4096, sequence=0x1003)
encoder.header(257, "old", size=4096)
encoder.data(257, 1, b"A" * 2048)
encoder.data(257, 2, b"B" * 2048)
encoder.data(257, 1, b"X" * 100, sequence=0x1001)
encoder.header(257, "middle", size=100, shrink=True, sequence=0x1001)
encoder.data(257, 3, b"tail-end", sequence=0x1004)
save("versions.yaffs", encoder)

encoder = Encoder()
encoder.header(300, "new", size=3)
encoder.data(300, 1, b"old")
encoder.header(257, "old", size=3)
encoder.data(257, 1, b"yes")
encoder.header(257, "new", size=3, shadow=300, sequence=0x1001, extra=True)
save("shadow.yaffs", encoder)

encoder = Encoder()
encoder.header(257, "long", size=(1 << 32) + 3, modern=True)
encoder.data(257, (1 << 32) // encoder.page + 1, b"end")
save("sparse-64bit.yaffs", encoder)

encoder = Encoder()
encoder.header(257, "original", size=7)
encoder.data(257, 1, b"survive")
encoder.header(258, "promoted", kind=4, equivalent=257)
encoder.header(259, "remaining", kind=4, equivalent=257)
# YAFFS unlink promotes one hardlink's path to the original object, then
# deletes that hardlink object. A remaining link still targets object 257.
encoder.header(257, "promoted", size=7, sequence=0x1001)
encoder.header(258, "deleted", kind=4, parent=4, equivalent=257, sequence=0x1001)
save("unlink-hardlink.yaffs", encoder)

(ROOT / "fixtures.json").write_text(json.dumps(manifest, indent=2) + "\n")
