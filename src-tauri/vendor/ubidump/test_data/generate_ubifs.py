"""CC0-1.0 independent UBIFS v5 fixtures; Python stdlib, no filesystem writer.

Layouts: Linux v6.12 ubifs-media.h and key.h. Compression id 2 is raw DEFLATE.
LZO literals and Zstd raw-block frames are encoded independently below.
"""
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parent
LEB = 28672
COUNT = 8
PAYLOAD = bytes((index * 17 + 3) & 255 for index in range(4096)) + bytes(4096) + (b"independent tail\n" * 60)[:809]
assert len(PAYLOAD) == 9001


def key(inode, kind=0, value=0):
    return struct.pack("<II", inode, (kind << 29) | value)


def name_hash(name):
    value = 0
    for byte in name:
        signed = byte if byte < 128 else byte - 256
        value = ((value + (signed << 4) + (signed >> 4)) * 11) & 0xffffffff
    value &= 0x1fffffff
    return value + 3 if value <= 2 else value


def node(kind, body, sequence):
    raw = bytearray(struct.pack("<IIQIBB2x", 0x06101831, 0, sequence, 24 + len(body), kind, 0) + body)
    struct.pack_into("<I", raw, 4, (~zlib.crc32(raw[8:])) & 0xffffffff)
    return raw


def inode(number, mode, size, links=1, data=b"", flags=0):
    body = bytearray(136)
    body[:8] = key(number)
    struct.pack_into("<QQqqq", body, 16, 1, size, -123, 1700000001, 1700000002)
    struct.pack_into("<10I", body, 56, 123456789, 234567890, 345678901, links, 1001, 1002, mode, flags, len(data), 0)
    struct.pack_into("<H", body, 104, 2)  # default compression; inline data stays raw
    return key(number), node(0, body + data, number)


def entry(parent, target, name, kind=0, xattr=False):
    name = name.encode()
    k = key(parent, 3 if xattr else 2, name_hash(name))
    body = k + bytes(8) + struct.pack("<QBBHI", target, 0, kind, len(name), 0) + name + b"\0"
    return k, node(3 if xattr else 2, body, 100 + target)


def compress(data, codec):
    if codec == 0:
        return data
    if codec == 1:
        if len(data) <= 238:
            prefix = bytes([17 + len(data)])
        else:
            zeroes, last = divmod(len(data) - 18, 255)
            if last == 0:
                zeroes, last = zeroes - 1, 255
            prefix = b"\0" + bytes(zeroes) + bytes([last])
        return prefix + data + bytes.fromhex("110000")
    if codec == 2:
        encoder = zlib.compressobj(wbits=-15)
        return encoder.compress(data) + encoder.flush()
    assert codec == 3
    if len(data) < 256:
        header = b"\x20" + bytes([len(data)])
    else:
        header = b"\x60" + struct.pack("<H", len(data) - 256)
    return bytes.fromhex("28b52ffd") + header + ((len(data) << 3) | 1).to_bytes(3, "little") + data


def image(codec, entropy=False):
    leaves = [inode(1, 0o40755, 600, 3), inode(2, 0o100644, len(PAYLOAD), 5),
              inode(3, 0o40755, 120, 2), inode(4, 0o120777, 11, data=b"payload.bin"),
              inode(5, 0o100600, 0), inode(6, 0o100600, 10, data=b"user-value", flags=0x20)]
    for name, target, kind in [("payload.bin", 2, 0), ("alias.bin", 2, 0), ("Ak", 2, 0), ("B`", 2, 0),
                                ("子目录", 3, 1), ("relative-link", 4, 2), ("empty", 5, 0)]:
        leaves.append(entry(1, target, name, kind))
    assert name_hash(b"Ak") == name_hash(b"B`")
    leaves.extend([entry(3, 2, "tail.txt"), entry(2, 6, "user.test", xattr=True)])
    for block in [0, 2]:
        content = PAYLOAD[block * 4096:(block + 1) * 4096]
        k = key(2, 1, block)
        block_codec = 0 if block == 0 and codec in (1, 3) else codec
        encoded = compress(content, block_codec)
        if entropy:
            block_codec = 3
            encoded = (ROOT / f"zstd-{block}.frame").read_bytes()
        body = k + bytes(8) + struct.pack("<IHH", len(content), block_codec, 0) + encoded
        leaves.append((k, node(1, body, 200 + block)))
    leaves.sort(key=lambda leaf: struct.unpack("<II", leaf[0]))
    volume = bytearray(b"\xff" * (LEB * COUNT))
    locations = {}
    cursor = 0
    def store(data, block=6):
        nonlocal cursor
        position = cursor if block == 6 else 0
        assert position + len(data) <= LEB
        volume[block * LEB + position:block * LEB + position + len(data)] = data
        result = (block, position, len(data))
        if block == 6:
            cursor = (position + len(data) + 7) & ~7
        return result
    branches = []
    for k, data in leaves:
        location = store(data)
        branches.append((k, location))
        locations.setdefault(str(data[20]), []).append(location)
    level = 0
    while len(branches) > 3:
        parents = []
        for start in range(0, len(branches), 3):
            children = branches[start:start+3]
            body = struct.pack("<HH", len(children), level) + b"".join(struct.pack("<III", *loc) + k for k, loc in children)
            location = store(node(9, body, 400 + level))
            parents.append((children[0][0], location))
        branches = parents
        level += 1
    root_body = struct.pack("<HH", len(branches), level) + b"".join(struct.pack("<III", *loc) + k for k, loc in branches)
    root = store(node(9, root_body, 500), 7)
    superblock = bytearray(4096 - 24)
    struct.pack_into("<IIIIIQIIIIIIII", superblock, 4, 0, 512, LEB, COUNT, COUNT, 16384, 1, 1, 1, 1, 3, 0, 5, codec)
    struct.pack_into("<I", superblock, 80, 1)  # time_gran is full offset 104
    volume[:4096] = node(6, superblock, 1)
    for block, offset, sequence in [(1, 512, 600), (2, 1024, 601)]:
        master = bytearray(512 - 24)
        struct.pack_into("<QQIIIII", master, 0, 6, 7, 2, 3, *root)
        struct.pack_into("<I", master, 140, COUNT)  # full master offset 164
        volume[block * LEB + offset:block * LEB + offset + 512] = node(7, master, sequence)
    locations['root'] = root
    return volume, locations


if __name__ == "__main__":
    ROOT.mkdir(parents=True, exist_ok=True)
    for codec, name in enumerate(["stored", "lzo", "deflate", "zstd"]):
        data, locations = image(codec)
        (ROOT / f"synthetic-{name}.ubifs").write_bytes(data)
    (ROOT / "payload.bin").write_bytes(PAYLOAD)
    if (ROOT / 'zstd-0.frame').exists() and (ROOT / 'zstd-2.frame').exists():
        (ROOT / 'synthetic-zstd-compressed.ubifs').write_bytes(image(3, True)[0])
