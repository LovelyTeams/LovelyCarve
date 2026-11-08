#!/usr/bin/env python3
"""CC0-1.0 independent invented OCFS2 encoder; not a native mkfs oracle."""
import hashlib
import json
import pathlib
import struct
import zlib


def build(mode):
    block_size, cluster_size = 1024, 4096
    raw = bytearray(16 * 1024 * 1024)
    checks = {}
    inode_blocks = set()
    meta_blocks = {769}
    discontiguous = mode == "discontiguous-ecc"
    ecc = mode != "plain"
    generation = 0x10203040
    uuid_hash = 0x6D5A7531

    def put(buffer, offset, code, *values):
        struct.pack_into("<" + code, buffer, offset, *values)

    def block(number):
        return memoryview(raw)[number * block_size:(number + 1) * block_size]

    def inode(number, file_mode, flags=1, dynamic=0, size=0, clusters=0):
        data = block(number)
        data[:8] = b"INODE01\0"
        slot, bit, group = (65535, number - 8, 0) if number < 136 else (0, number - 512, 0)
        if number == 10241:
            bit, group = 129, 512
        put(data, 8, "IHH", 1, slot, bit)
        put(data, 20, "IIIQHHI", clusters, 1000, 1000, size, file_mode, 1, flags)
        put(data, 48, "QQQQQ", (-315619200) & ((1 << 64) - 1), 1700000001, 1700000000, 0, number)
        put(data, 96, "IIII", generation, 123, 789, 456)
        put(data, 118, "H", dynamic)
        put(data, 152, "Q", group)
        inode_blocks.add(number)
        checks[number] = 128
        return data

    def inline(number, payload, file_mode=0x81A4, flags=1):
        data = inode(number, file_mode, flags, 1, len(payload))
        put(data, 192, "H", block_size - 200)
        data[200:200 + len(payload)] = payload
        return data

    def extents(data, values):
        put(data, 192, "HHHHQ", 0, 51, len(values), 0, 0)
        for index, (logical, count, physical, flags) in enumerate(values):
            put(data, 208 + index * 16, "IHBBQ", logical, count, 0, flags, physical)

    def directory(number, parent, entries, flags=1):
        entries = [(".", number, 2), ("..", parent, 2)] + entries
        data = bytearray(block_size - 200)
        position = 0
        for index, (name, target, kind) in enumerate(entries):
            name = name.encode()
            length = ((12 + len(name) + 3) // 4) * 4 if index + 1 < len(entries) else len(data) - position
            put(data, position, "QHBB", target, length, len(name), kind)
            data[position + 12:position + 12 + len(name)] = name
            position += length
        inline(number, data, 0x41ED, flags)

    systems = [("bad_blocks", 11, 1), ("global_inode_alloc", 12, 1), ("slot_map", 13, 1),
               ("heartbeat", 14, 1), ("global_bitmap", 15, 1), ("orphan_dir:0000", 18, 2),
               ("extent_alloc:0000", 19, 1), ("inode_alloc:0000", 20, 1), ("journal:0000", 21, 1),
               ("local_alloc:0000", 22, 1), ("truncate_log:0000", 23, 1)]
    for number in range(9, 24):
        inode(number, 0x81A4, 0x11)
    directory(10, 10, systems, 0x11)
    root_entries = [("hello", 513, 1), ("alias", 513, 1), ("sparse", 514, 1),
                    ("sub", 515, 2), ("link", 516, 7), ("fifo", 517, 5), ("unwritten", 518, 1)]
    if discontiguous:
        root_entries.append(("moved", 10241, 1))
        inline(10241, b"discontiguous inode allocation")
    directory(9, 9, root_entries, 0x11)
    directory(18, 10, [], 0x11)
    inline(513, b"hello OCFS2 evidence\n")
    directory(515, 9, [("nested", 513, 1)])
    data = inode(516, 0xA1FF, size=10)
    data[192:203] = b"sub/nested\0"
    inode(517, 0x11A4)
    data = inode(514, 0x81A4, size=cluster_size * 3 - 7, clusters=1)
    extents(data, [(1, 1, 9216, 0)])
    raw[9216 * block_size:9220 * block_size] = bytes((index * 17 + 5) & 255 for index in range(cluster_size))
    data = inode(518, 0x81A4, size=cluster_size * 2, clusters=2)
    extents(data, [(0, 2, 9220, 1)])
    raw[9220 * block_size:9228 * block_size] = b"\xA5" * (cluster_size * 2)
    data = inode(13, 0x81A4, 0x11, size=8, clusters=1)
    extents(data, [(0, 1, 256, 0)])
    data = inode(21, 0x8180, 0x111, size=8 * 1024 * 1024, clusters=2048)
    extents(data, [(0, 2048, 1024, 0)])
    journal = block(1024)
    struct.pack_into(">9I", journal, 0, 0xC03B3998, 4, 0, block_size, 8192, 1, 1, 0, 0)
    inode(22, 0x8180, 0xD1)
    inode(23, 0x8180, 0x811)

    # Two raw native-namespaced attributes, one local and one value tree.
    data = block(769)
    data[:8] = b"XATTR01\0"
    put(data, 8, "HHIQ", 0, 1, generation, 769)
    checks[769] = 24
    put(block(513), 118, "H", 3)
    put(block(513), 120, "Q", 769)
    header = data[48:]
    put(header, 0, "H", 2)
    for index, (name, offset, payload, local) in enumerate([(b"inline", 64, b"\0\xfffixture", True), (b"external", 112, bytes(range(100)), False)]):
        name_hash = uuid_hash
        for byte in name:
            name_hash = ((name_hash << 5) | (name_hash >> 27)) & 0xFFFFFFFF
            name_hash ^= byte
        put(header, 16 + index * 16, "IHBBQ", name_hash, offset, len(name), 0x81 if local else 1, len(payload))
        header[offset:offset + len(name)] = name
        value = offset + ((len(name) + 3) // 4) * 4
        if local:
            header[value:value + len(payload)] = payload
        else:
            put(header, value, "IIQHHHHQIHBBQ", 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 9228)
            raw[9228 * block_size:9228 * block_size + len(payload)] = payload

    metadata_groups = [(12, 8, 128, 32, 4, {0} | {number - 8 for number in inode_blocks if number < 136}),
                       (20, 512, 256, 64, 4, {0} | {number - 512 for number in inode_blocks if 512 <= number < 768} | ({129} if discontiguous else set())),
                       (19, 768, 256, 64, 4, {0} | {number - 768 for number in meta_blocks})]
    used_clusters = {0, 1, 64} | set(range(2, 34)) | set(range(128, 256)) | set(range(256, 2308))
    if discontiguous:
        used_clusters -= set(range(160, 192))
        used_clusters |= set(range(2560, 2592))
    metadata_groups.insert(0, (15, 4, 4096, 7680, 1, used_clusters))
    for owner, group, bits, cpg, bpc, used in metadata_groups:
        count = bits - len(used)
        data = inode(owner, 0x8180, 0x491, size=bits * (cluster_size if bpc == 1 else block_size), clusters=bits if bpc == 1 else bits // bpc)
        put(data, 184, "II", len(used), bits)
        put(data, 192, "HHHHQIIQ", cpg, bpc, 51, 1, 0, count, bits, group)
        desc = block(group)
        desc[:8] = b"GROUP01\0"
        bitmap_size = 960 if bpc == 1 or owner == 12 else 256
        put(desc, 8, "HHHHI", bitmap_size, bits, count, 0, generation)
        put(desc, 32, "QQ", owner, group)
        for bit in used:
            desc[64 + bit // 8] |= 1 << (bit % 8)
        checks[group] = 48
        if discontiguous and owner == 20:
            put(desc, 320, "HHHHQIHBBQIHBBQ", 0, 43, 2, 0, 0, 0, 32, 0, 0, 512, 32, 32, 0, 0, 10240)
    sb = block(2)
    sb[:8] = b"OCFSV2\0\0"
    put(sb, 20, "I", 4096)
    put(sb, 44, "I", 0x31)
    put(sb, 80, "Q", 2)
    put(sb, 96, "I", generation)
    put(sb, 192, "HH", 0, 90)
    features = 0x358 | (0x800 if ecc else 0) | (0x2000 if discontiguous else 0)
    put(sb, 220, "IIIQQIIHHIQ", 2, features, 1, 9, 10, 10, 12, 1, 0, uuid_hash, 4)
    label = b"LovelyCarve invented"
    sb[272:272 + len(label)] = label
    sb[336:352] = bytes(range(16))
    checks[2] = 128
    for number, links in [(9, 3), (10, 3), (18, 2), (513, 3), (515, 2)]:
        put(block(number), 42, "H", links)
    if ecc:
        for number, offset in checks.items():
            data = block(number)
            data[offset:offset + 8] = bytes(8)
            put(data, offset, "I", (~zlib.crc32(data)) & 0xFFFFFFFF)
    return bytes(raw)


if __name__ == "__main__":
    target = pathlib.Path(__file__).resolve().parents[1] / "vendor/ocfs2-reader/tests/fixtures"
    target.mkdir(exist_ok=True, parents=True)
    manifest = {"license": "CC0-1.0", "generator": "scripts/ocfs2-fixtures.py", "fixtures": []}
    for mode in ["plain", "ecc", "discontiguous-ecc"]:
        data = build(mode)
        packed = zlib.compress(data, 9)
        (target / (mode + ".zlib")).write_bytes(packed)
        manifest["fixtures"].append({"name": mode, "decoded_bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "compressed_sha256": hashlib.sha256(packed).hexdigest()})
    (target / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf8")
