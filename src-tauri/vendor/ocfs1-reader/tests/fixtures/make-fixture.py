"""Write a synthetic OracleCFS 1.2 volume from Oracle's disk.txt offsets."""
from pathlib import Path
from struct import pack_into
import zlib

SECTOR = 512
CLUSTER = 0x20000
DATA = 0x10000
BITMAP = 0xC000
ROOT = 0x50000
SECOND = 0x70000
CHILD = 0x90000
TREE = 0xD0000
LEAF1 = TREE + SECTOR
LEAF2 = LEAF1 + SECTOR
DIRECT = 0x110000
LINK = 0x130000
RUNS = (0x150000, 0x190000, 0x1B0000, 0x1D0000)
raw = bytearray(4 * 1024 * 1024)
clusters = (len(raw) - DATA) // CLUSTER


def allocated(address, size):
    for index in range((address - DATA) // CLUSTER, (address + size - 1 - DATA) // CLUSTER + 1):
        raw[BITMAP + index // 8] |= 1 << (index % 8)


def directory(address, indices, next_address=(1 << 64) - 1, first=True):
    allocated(address, 128 * 1024)
    raw[address + 48:address + 56] = b"DIRNV20\0"
    pack_into("<QQ", raw, address + 76, address, next_address)
    raw[address + 118] = 255
    raw[address + 120] = len(indices)
    raw[address + 121] = first
    raw[address + 123:address + 123 + len(indices)] = bytes(indices)


def file(parent, index, name, size, allocation, records, *, attr=0x40, depth=-1, last=0, flags=1):
    address = parent + (index + 1) * SECTOR
    data = name.encode()
    raw[address + 48:address + 52] = b"FIL\0"
    pack_into("<IB", raw, address + 56, depth == -1, 0 if attr == 1 else len(records))
    pack_into("<i", raw, address + 64, depth)
    raw[address + 68:address + 68 + len(data)] = data
    pack_into("<H", raw, address + 324, len(data))
    pack_into("<QQQQ", raw, address + 328, size, allocation, 1059525553, 1059525675)
    pack_into("<QQQIIII", raw, address + 432, parent, address, last, flags, 0, attr, 0o640)
    for i, record in enumerate(records):
        pack_into("<QQQ", raw, address + 360 + i * 24, *record)
    return address


def group(address, parent, records, next_leaf=0, leaf=True):
    allocated(address, SECTOR)
    raw[address:address + 8] = b"EXTDAT1\0" if leaf else b"EXTHDR2\0"
    pack_into("<IIIII", raw, address + 8, len(records), int(leaf), int(leaf), 1 if leaf else 2, 0)
    pack_into("<QQ", raw, address + 32, address, next_leaf)
    pack_into("<Q", raw, address + 64, parent)
    for i, record in enumerate(records):
        pack_into("<QQQ", raw, address + 72 + i * 24, *record)


pack_into("<II", raw, 0, 2, 1)
raw[8:18] = b"OracleCFS\0"
pack_into("<QQQQQQQQQQQQQQ", raw, 272,
          len(raw), 0, BITMAP, 0x4000, 0x8000, 0, DATA, 0, ROOT, 0, CLUSTER, 32, clusters, 0)
pack_into("<IIII", raw, 424, 0o755, 1000, 1000, 0xFFFFFFFF)
raw[512 + 48:512 + 56] = b"ocfs-lab"
pack_into("<H", raw, 512 + 112, 8)
raw[512 + 114:512 + 130] = bytes.fromhex("4d04cae0ac856c26a44ef18360a8e08e")
pack_into("<H", raw, 512 + 130, 16)
directory(ROOT, [3, 0, 5, 1], SECOND)
directory(SECOND, [0], first=False)
directory(CHILD, [7])

file(ROOT, 0, "archive", 128 * 1024, 128 * 1024, [(0, 0, CHILD)], attr=1)
file(ROOT, 1, "shortcut", 24, CLUSTER, [(0, CLUSTER, LINK)], attr=0x100)
file(ROOT, 3, "deleted", 0, 0, [], flags=0)
file(ROOT, 5, "control.bin", 715, CLUSTER, [(0, CLUSTER, DIRECT)])
file(SECOND, 0, "empty.dbf", 0, 0, [])
tree_file = file(CHILD, 7, "archive-file.dbf", 4 * CLUSTER - 71, 4 * CLUSTER,
                 [(0, 4 * CLUSTER, TREE)], depth=1, last=LEAF2)
group(TREE, tree_file, [(0, 2 * CLUSTER, LEAF1), (2 * CLUSTER, 2 * CLUSTER, LEAF2)], leaf=False)
group(LEAF1, TREE, [(0, CLUSTER, RUNS[0]), (CLUSTER, CLUSTER, RUNS[1])], next_leaf=LEAF2)
group(LEAF2, TREE, [(2 * CLUSTER, CLUSTER, RUNS[2]), (3 * CLUSTER, CLUSTER, RUNS[3])])

for i, address in enumerate(RUNS):
    allocated(address, CLUSTER)
    raw[address:address + CLUSTER] = bytes((j * 29 + j // 193 + i * 7) & 255 for j in range(CLUSTER))
allocated(DIRECT, CLUSTER)
raw[DIRECT:DIRECT + 715] = bytes((i ^ 0xA5) & 255 for i in range(715))
allocated(LINK, CLUSTER)
raw[LINK:LINK + 24] = b"archive/archive-file.dbf"
Path(__file__).with_name("oraclecfs-12.zlib").write_bytes(zlib.compress(raw, 9))
