#!/usr/bin/env python3
"""CC0-1.0 independent invented NILFS2 encoder, separate from native fixtures."""
import hashlib
import json
import pathlib
import struct
import zlib


def build(block, high=False):
    segment, segments, seed = 128, 16, 0x6D5A7531
    if high:
        segments = 65536
    first = (4096 + block - 1) // block
    log_start = (6 * 1024 ** 3) // block if high else first
    active_segment = log_start // segment
    device_size = segment * segments * block + 4096
    items = []
    virtual = {}

    def put(raw, at, code, *values):
        struct.pack_into("<" + code, raw, at, *values)

    def checksum(raw):
        return (~zlib.crc32(raw, (~seed) & 0xFFFFFFFF)) & 0xFFFFFFFF

    def add(owner, logical, raw, view=0, node=False):
        number = len(virtual) + 1
        item = {"owner": owner, "key": logical, "raw": raw, "virtual": number, "view": view, "node": node}
        items.append(item)
        virtual[number] = item
        return number

    def tree(mapping, owner, view=0):
        result = bytearray(56)
        if not mapping or max(mapping) < 6:
            for key, value in mapping.items():
                put(result, 8 + key * 8, "Q", value)
            return result
        keys = sorted(mapping)
        level = 1
        while len(keys) > 3:
            capacity = (block - 16) // 16
            parents = {}
            for offset in range(0, len(keys), capacity):
                subset = keys[offset:offset + capacity]
                raw = bytearray(block)
                put(raw, 0, "BBH", 0, level, len(subset))
                for index, key in enumerate(subset):
                    put(raw, 16 + index * 8, "Q", key)
                    put(raw, 16 + capacity * 8 + index * 8, "Q", mapping[key])
                parents[subset[0]] = add(owner, subset[0], raw, view, True)
            mapping, keys = parents, sorted(parents)
            level += 1
        put(result, 0, "BBH", 1, level, len(keys))
        for index, key in enumerate(keys):
            put(result, 8 + index * 8, "Q", key)
            put(result, 32 + index * 8, "Q", mapping[key])
        return result

    def inode(number, size, mapping, mode=0x81A4, links=1):
        raw = bytearray(128)
        put(raw, 0, "QQqqIIIIHHI", len(mapping) * (block // 512), size, 1700000001, -315619200,
            123, 456, 1000, 1000, mode, links, 0)
        raw[56:112] = mapping if isinstance(mapping, bytes | bytearray) else tree(mapping, number)
        put(raw, 120, "I", number & 0xFFFFFFFF)
        return raw

    def make_inode(number, size, mapping, mode=0x81A4, links=1):
        before = len(items)
        raw = inode(number, size, bytes(tree(mapping, number)), mode, links)
        put(raw, 0, "Q", (len(mapping) + len(items) - before) * (block // 512))
        return raw

    def directory(owner, parent, entries):
        raw = bytearray(block)
        records = [(".", owner, 2), ("..", parent, 2)] + entries
        at = 0
        for index, (name, target, kind) in enumerate(records):
            encoded = name.encode()
            length = ((12 + len(encoded) + 7) // 8) * 8 if index + 1 < len(records) else block - at
            put(raw, at, "QHBB", target, length if length < 65536 else 65535, len(encoded), kind)
            raw[at + 12:at + 12 + len(encoded)] = encoded
            at += length
        return raw

    cp_virtual = add(4, 0, bytearray(block))
    usage_virtual = add(5, 0, bytearray(block))
    usage_mapping = {0: usage_virtual}
    if high:
        key = (active_segment + 2) // (block // 16)
        usage_mapping[key] = add(5, key, bytearray(block))
    far = block * 8 + 11
    root_raw = directory(2, 2, [("hello", 11, 1), ("deleted", 0, 0), ("alias", 11, 1), ("sparse", 12, 1),
                               ("link", 13, 7), ("sub", 14, 2), ("fifo", 15, 5), ("versioned", 16, 1), ("big", 17, 1), ("far", far, 1)])
    root_mapping = {0: add(2, 0, root_raw)}
    if block == 65536:
        deleted = bytearray(block)
        put(deleted, 8, "H", 65535)
        root_mapping[1] = add(2, 1, deleted)
    content = b"hello NILFS2 evidence\n"
    hello = add(11, 0, content.ljust(block, b"\0"))
    sparse_first = add(12, 1, bytes((index * 23 + 7) & 255 for index in range(block)))
    sparse_last = add(12, 3, b"tail".ljust(block, b"A"))
    link = add(13, 0, b"sub/nested".ljust(block, b"\0"))
    sub = add(14, 0, directory(14, 2, [("nested", 11, 1)]))
    versions = [b"snapshot-one", b"snapshot-two", b"current"]
    version_blocks = [add(16, 0, value.ljust(block, b"\0"), view) for view, value in enumerate(versions, 1)]
    big_keys = list(range(6)) + [5 * 1024 ** 3 // block if high else 6]
    big = {index: add(17, index, bytes((offset * 17 + 5) & 255 for offset in range(block))) for index in big_keys}
    far_block = add(far, 0, b"second inode allocation group".ljust(block, b"\0"))
    common = {2: make_inode(2, len(root_mapping) * block, root_mapping, 0x41ED, 3),
              11: make_inode(11, len(content), {0: hello}, links=3),
              12: make_inode(12, block * 4 - 7, {1: sparse_first, 3: sparse_last}),
              13: make_inode(13, 10, {0: link}, 0xA1FF),
              14: make_inode(14, block, {0: sub}, 0x41ED, 2),
              15: make_inode(15, 0, {}, 0x11A4),
              17: make_inode(17, 5 * 1024 ** 3 + 37 if high else block * 7 - 13, big),
              far: make_inode(far, 29, {0: far_block})}
    entries_per_group = block * 8
    entries_per_block = block // 128
    blocks_per_group = (entries_per_group + entries_per_block - 1) // entries_per_block + 1
    group_descriptor = bytearray(block)
    bitmaps = []
    for group, used in [(0, set(range(18))), (1, {11})]:
        bitmap = bytearray(block)
        for bit in used:
            bitmap[bit // 8] |= 1 << (bit % 8)
        put(group_descriptor, group * 4, "I", entries_per_group - len(used))
        bitmaps.append(add(6, 1 + group * blocks_per_group, bitmap))
    descriptor = add(6, 0, group_descriptor)
    ifiles = []
    for view in [1, 2, 3]:
        inodes = dict(common)
        inodes[16] = make_inode(16, len(versions[view - 1]), {0: version_blocks[view - 1]})
        tables = {}
        for number, raw in inodes.items():
            group, local = divmod(number, entries_per_group)
            key = 2 + group * blocks_per_group + local // entries_per_block
            table = tables.setdefault(key, bytearray(block))
            offset = (local % entries_per_block) * 128
            table[offset:offset + 128] = raw
        mapping = {0: descriptor, 1: bitmaps[0], 1 + blocks_per_group: bitmaps[1]}
        mapping.update({key: add(6, key, raw, view) for key, raw in sorted(tables.items())})
        before = len(items)
        raw = inode(6, 0, bytes(tree(mapping, 6, view)))
        put(raw, 0, "Q", (len(mapping) + len(items) - before) * (block // 512))
        ifiles.append(raw)

    dat_entries_per_block = block // 32
    dat_blocks = 2 + max(virtual) // dat_entries_per_block + 1
    assert dat_blocks <= 6
    for key in range(dat_blocks):
        items.append({"owner": 3, "key": key, "raw": bytearray(block), "virtual": None, "view": 0, "node": False})
    items.sort(key=lambda item: (item["owner"], item["node"], item["virtual"] or item["key"]))
    groups = []
    for owner in sorted(set(item["owner"] for item in items)):
        group = [item for item in items if item["owner"] == owner]
        groups.append((owner, group))
    summary = bytearray(64)

    def append(raw):
        remaining = block - len(summary) % block
        if len(raw) > remaining:
            summary.extend(bytes(remaining))
        summary.extend(raw)

    for owner, group in groups:
        append(struct.pack("<QQII", owner, 3, len(group), sum(not item["node"] for item in group)))
        for item in group:
            if owner == 3:
                append(struct.pack("<Q", item["key"]))
            elif item["node"]:
                append(struct.pack("<Q", item["virtual"]))
            else:
                append(struct.pack("<QQ", item["virtual"], item["key"]))
    summary_bytes = len(summary)
    summary_blocks = (summary_bytes + block - 1) // block
    summary.extend(bytes(summary_blocks * block - len(summary)))
    for index, item in enumerate(items):
        item["physical"] = log_start + summary_blocks + index
    root_block = log_start + summary_blocks + len(items)
    assert root_block < (active_segment + 1) * segment
    dat = {item["key"]: item for item in items if item["owner"] == 3}
    put(dat[0]["raw"], 0, "I", entries_per_group - len(virtual) - 1)
    dat[1]["raw"][0] = 1
    for number, item in virtual.items():
        dat[1]["raw"][number // 8] |= 1 << (number % 8)
        key, offset = 2 + number // dat_entries_per_block, (number % dat_entries_per_block) * 32
        start = item["view"] or 1
        end = start + 1 if item["view"] in [1, 2] else (1 << 64) - 1
        put(dat[key]["raw"], offset, "QQQQ", item["physical"], start, end, 0)
    cp = virtual[cp_virtual]["raw"]
    put(cp, 0, "QQQQ", 3, 2, 1, 2)
    feature = block != 65536
    for number in [1, 2, 3]:
        offset = number * 192
        put(cp, offset, "IIQQQQQQQ", 1 if number < 3 else 0, 0, 2 if number == 1 else 0,
            1 if number == 2 else 0, number, 1700000000 + number, 1, 9, 42 if feature else 0)
        cp[offset + 64:offset + 192] = ifiles[number - 1]
    usage = virtual[usage_virtual]["raw"]
    put(usage, 0, "QQQ", segments - 1, 1, active_segment)
    usage_index = active_segment + 2
    key, offset = divmod(usage_index, block // 16)
    put(virtual[usage_mapping[key]]["raw"], offset * 16, "QII", 1700000003, root_block + 1 - active_segment * segment, 2)
    root = bytearray(block)
    put(root, 4, "HHQ", 400, 0, 1700000003)
    dat_inode = inode(3, 0, bytes(tree({key: item["physical"] for key, item in dat.items()}, 3)))
    put(dat_inode, 0, "Q", len(dat) * (block // 512))
    root[16:144] = dat_inode
    root[144:272] = make_inode(4, 0, {0: cp_virtual})
    root[272:400] = make_inode(5, 0, usage_mapping)
    put(root, 0, "I", checksum(root[4:400]))
    count = summary_blocks + len(items) + 1
    put(summary, 8, "IHHQQQIIIIQ", 0x1EAFFA11, 64, 7, 7, 1700000003, (active_segment + 1) * segment, count, len(groups), summary_bytes, 0, 3)
    put(summary, 4, "I", checksum(summary[8:summary_bytes]))
    log = summary + b"".join(item["raw"] for item in items) + root
    put(log, 0, "I", checksum(log[4:]))
    sb = bytearray(1024)
    put(sb, 0, "IHHHHIII", 2, 0, 0x3434, 280, 0, seed, 0, block.bit_length() - 11)
    put(sb, 24, "QQQIIQQQQQQQ", segments, device_size, first, segment, 5, 3, log_start, 7, (segments - 1) * segment, 1700000000, 1700000000, 1700000003)
    put(sb, 116, "H", 1)
    put(sb, 140, "IHHHH", 11, 128, 32, 192, 16)
    sb[152:168] = bytes(range(16))
    label = b"LovelyCarve invented"
    sb[168:168 + len(label)] = label
    put(sb, 264, "Q", int(feature))
    put(sb, 16, "I", checksum(sb[:280]))
    backup = (device_size // 4096 - 1) * 4096
    if high:
        return device_size, [(1024, bytes(sb)), (backup, bytes(sb)), (log_start * block, bytes(log))]
    raw = bytearray(device_size)
    raw[first * block:first * block + len(log)] = log
    raw[1024:2048] = sb
    raw[backup:backup + 1024] = sb
    return bytes(raw)


if __name__ == "__main__":
    output = pathlib.Path(__file__).resolve().parents[1] / "src-tauri/vendor/nilfs2-reader/tests/fixtures"
    output.mkdir(exist_ok=True, parents=True)
    manifest = {"license": "CC0-1.0", "generator": "scripts/nilfs2-fixtures.py", "fixtures": []}
    for block in [1024, 4096, 65536]:
        raw = build(block)
        packed = zlib.compress(raw, 9)
        name = f"invented-{block}"
        (output / (name + ".zlib")).write_bytes(packed)
        manifest["fixtures"].append({"name": name, "decoded_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest(),
                                     "compressed_sha256": hashlib.sha256(packed).hexdigest()})
    length, chunks = build(1024, high=True)
    high = {"length": length, "chunks": []}
    for index, (offset, raw) in enumerate(chunks):
        name = f"high-chunk-{index}.zlib"
        packed = zlib.compress(raw, 9)
        (output / name).write_bytes(packed)
        high["chunks"].append({"name": name, "offset": offset, "decoded_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()})
    manifest["virtual_source"] = high
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf8")
