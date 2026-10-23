"""CC0-1.0 independent JFFS2 new-format fixtures using Python stdlib.

Wire layout: Linux v6.12 include/uapi/linux/jffs2.h; raw CRC-32 init/xor=0.
No JFFS2 writer/parser dependency. LZO is an independently encoded literal run.
"""
from pathlib import Path
import hashlib
import struct
import zlib

ROOT = Path(__file__).resolve().parent
BASE = bytes((index * 17 + 3) & 255 for index in range(4096))
TAIL = (b"independent tail\n" * 60)[:809]
PAYLOAD = BASE[:8] + b"OVERLAP!" + BASE[16:] + bytes(4096) + TAIL
assert len(PAYLOAD) == 9001


def crc(data):
    return zlib.crc32(data, 0xffffffff) ^ 0xffffffff


def compression(data, codec):
    if codec in (0, 4):
        return data
    if codec == 1:
        assert not any(data)
        return b""
    if codec == 2:
        # Greedy RTIME encoder, including overlapping back references.
        positions = [0] * 256
        result = bytearray()
        offset = 0
        while offset < len(data):
            value = data[offset]
            offset += 1
            previous = positions[value]
            positions[value] = offset
            repeat = 0
            while repeat < 255 and offset < len(data) and data[previous] == data[offset]:
                previous += 1
                offset += 1
                repeat += 1
            result.extend((value, repeat))
        return bytes(result)
    if codec == 6:
        return zlib.compress(data, 3)
    if codec == 7:
        if len(data) <= 238:
            return bytes([17 + len(data)]) + data + b"\x11\0\0"
        count = len(data) - 18
        return b"\0" + bytes((count - 1) // 255) + bytes([(count - 1) % 255 + 1]) + data + b"\x11\0\0"
    raise ValueError(codec)


class Writer:
    def __init__(self, order):
        self.order = order

    def pack(self, fmt, *args):
        return struct.pack(self.order + fmt, *args)

    def header(self, kind, size):
        prefix = self.pack("HHI", 0x1985, kind, size)
        return prefix + self.pack("I", crc(prefix))

    def inode(self, number, version, mode, size, offset=0, data=b"", codec=0):
        encoded = compression(data, codec)
        node = self.header(0xe002, 68 + len(encoded))
        node += self.pack("IIIHHIIIIIIIBBH", number, version, mode, 1000, 1001,
                          size, 1700000001, 1700000002, 1700000003, offset,
                          len(encoded), len(data), codec, 0, 0)
        assert len(node) == 60
        return node + self.pack("II", crc(encoded), crc(node)) + encoded

    def dirent(self, parent, version, number, name, kind=8):
        name = name.encode()
        node = self.header(0xe001, 40 + len(name))
        node += self.pack("IIIIBB2x", parent, version, number, 1700000004, len(name), kind)
        return node + self.pack("II", crc(node), crc(name)) + name

    def obsolete(self, node):
        result = bytearray(node)
        kind, = struct.unpack_from(self.order + "H", result, 2)
        struct.pack_into(self.order + "H", result, 2, kind & ~0x2000)
        return bytes(result)

    def auxiliary(self):
        name = b"fixture"
        value = b"value"
        data = name + b"\0" + value
        xattr = self.header(0xe008, (32 + len(data) + 3) & ~3)
        xattr += self.pack("IIBBH", 1, 1, 1, len(name), len(value))
        xattr += self.pack("I", crc(data))
        xattr += self.pack("I", crc(xattr)) + data
        xref = self.header(0xe009, 28) + self.pack("III", 2, 1, 2)
        xref += self.pack("I", crc(xref))
        marker = self.pack("II", 0, 0x02851885)
        summary = self.header(0x2006, 40) + self.pack("III", 0, 12, 0)
        summary += self.pack("II", crc(marker), crc(summary)) + marker
        return [xattr, xref, summary]

    def image(self, codec):
        # Intentionally out of physical version order; GC copy and obsolete inode.
        base = self.inode(2, 1, 0o100644, 4096, data=BASE, codec=codec)
        nodes = [
            self.header(0x2003, 12),
            self.inode(1, 1, 0o40755, 0),
            self.dirent(1, 4, 2, "payload.bin"),
            self.inode(2, 4, 0o100644, 9001, 8192, TAIL, codec),
            self.dirent(1, 3, 5, "deleted.txt"),
            self.inode(2, 2, 0o100644, 4096, 8, b"OVERLAP!", 0),
            base,
            self.inode(2, 3, 0o100644, 8192, 4096, bytes(4096), 1),
            self.dirent(1, 8, 0, "deleted.txt", 0),
            self.dirent(1, 5, 2, "alias.bin"),
            self.dirent(1, 6, 3, "子目录", 4),
            self.inode(3, 1, 0o40755, 0),
            self.dirent(3, 1, 2, "tail.txt"),
            self.dirent(1, 7, 4, "link", 10),
            self.inode(4, 1, 0o120777, 11, data=b"payload.bin"),
            self.dirent(1, 9, 5, "empty.txt"),
            self.inode(5, 1, 0o100600, 0),
            self.dirent(1, 10, 6, "cut.txt"),
            self.inode(6, 2, 0o100644, 5),
            self.inode(6, 1, 0o100644, 16, data=b"0123456789abcdef"),
            base,
            self.obsolete(self.inode(2, 999, 0o100644, 4, data=b"BAD!")),
        ] + self.auxiliary()
        output = bytearray()
        for node in nodes:
            output.extend(node)
            output.extend(b"\xff" * (-len(node) % 4))
        output.extend(b"\xff" * 24)
        output.extend(bytes(16))
        return bytes(output)


if __name__ == "__main__":
    ROOT.mkdir(exist_ok=True)
    for order, label in [("<", "le"), (">", "be")]:
        writer = Writer(order)
        for codec, name in [(0, "stored"), (2, "rtime"), (6, "zlib"), (7, "lzo")]:
            (ROOT / f"synthetic-{label}-{name}.jffs2").write_bytes(writer.image(codec))
    (ROOT / "payload.bin").write_bytes(PAYLOAD)
    for path in sorted(ROOT.glob("*.jffs2")) + [ROOT / "payload.bin"]:
        print(path.name, len(path.read_bytes()), hashlib.sha256(path.read_bytes()).hexdigest())
