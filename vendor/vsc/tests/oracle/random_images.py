"""Generate random multi-store VSS volumes and dump every store with pyvshadow.

Usage: random_images.py <out-dir> <count> <seed> <dfvfs-vss.raw>

The NTFS boot sector and the VSS identifier are copied from dfVFS vss.raw.

Each image has 1-4 stores whose block lists mix plain, overlay, forwarder and
not-used descriptors (including forwarder chains, self-forwarders and reverse
swaps), random current/previous bitmaps, and a shuffled catalog order. For image
k this writes rand_k.raw, rand_k.store<i>.oracle.bin per store, and
rand_k.status ("ok <stores>" or "error <message>").
"""
import os
import random
import struct
import sys

import pyvshadow

BLOCK = 16384
LIVE = range(2, 64)        # live volume blocks used as original/forward targets
SAVED = range(64, 128)     # blocks holding saved store data
META_BASE = 128            # 6 metadata blocks per store from here
STORES_MAX = 4
TOTAL_BLOCKS = META_BASE + 6 * STORES_MAX
FLAG_FORWARDER, FLAG_OVERLAY, FLAG_NOT_USED = 0x1, 0x2, 0x4


def block_header(identifier, record_type, offset, next_offset, info_size=0):
    header = bytearray(128)
    header[0:16] = identifier
    struct.pack_into("<IIQQQQ", header, 16, 1, record_type, 0, offset, next_offset, info_size)
    return header


def put(image, offset, data):
    image[offset:offset + len(data)] = data


def fill_patterns(image):
    for number in list(LIVE) + list(SAVED):
        for sub in range(32):
            start = number * BLOCK + sub * 512
            pattern = struct.pack("<HH", number, sub) * 128
            put(image, start, pattern)


def random_descriptors(rng):
    descriptors = []
    forward_targets = []
    for _ in range(rng.randint(0, 40)):
        roll = rng.random()
        original = rng.choice(LIVE)
        if forward_targets and rng.random() < 0.15:
            # Re-use an earlier forwarder target to trigger a reverse swap.
            original = rng.choice(forward_targets)
        if roll < 0.40:
            flags, relative, store, bitmap = 0, rng.randint(0, 64) * BLOCK, rng.choice(SAVED) * BLOCK, 0
        elif roll < 0.65:
            flags, relative, store = FLAG_OVERLAY, 1, rng.choice(SAVED) * BLOCK
            bitmap = rng.getrandbits(32) or 1
        elif roll < 0.90:
            target = original if rng.random() < 0.1 else rng.choice(LIVE)
            forward_targets.append(target)
            flags, relative, store, bitmap = FLAG_FORWARDER, target * BLOCK, 0, 0
        else:
            flags = FLAG_NOT_USED | rng.choice([0, FLAG_OVERLAY, FLAG_FORWARDER])
            relative = rng.choice(LIVE) * BLOCK
            store = 0 if flags & FLAG_FORWARDER else rng.choice(SAVED) * BLOCK
            bitmap = rng.getrandbits(32)
        descriptors.append((original * BLOCK, relative, store, flags, bitmap))
    return descriptors


def random_bitmap(rng):
    bits = bytearray(BLOCK - 128)
    for number in LIVE:
        if rng.random() < 0.3:
            bits[number // 8] |= 1 << (number % 8)
    return bits


def build(rng, identifier, boot_sector):
    image = bytearray(TOTAL_BLOCKS * BLOCK)
    fill_patterns(image)
    # A real NTFS boot sector resized to this image, plus its backup copy in
    # the last sector (libvshadow validates the NTFS volume header).
    boot = bytearray(boot_sector)
    struct.pack_into("<Q", boot, 0x28, len(image) // 512 - 1)
    put(image, 0, boot)
    put(image, len(image) - 512, boot)
    volume_header = bytearray(128)
    volume_header[0:16] = identifier
    struct.pack_into("<IIQ", volume_header, 16, 1, 1, 0x1E00)
    struct.pack_into("<Q", volume_header, 48, 1 * BLOCK)
    put(image, 0x1E00, volume_header)

    count = rng.randint(1, STORES_MAX)
    catalog = bytearray(BLOCK)
    catalog[0:128] = block_header(identifier, 2, 1 * BLOCK, 0)
    order = list(range(count))
    rng.shuffle(order)
    entry = 128
    for position, index in enumerate(order):
        meta = META_BASE + 6 * index
        header_at, list_at, list2_at, bitmap_at, previous_at = (meta + i for i in range(5))
        store_id = bytes([index + 1] * 16)
        creation_time = 132_600_000_000_000_000 + index * 10_000_000

        two_type = bytearray(128)
        struct.pack_into("<QQ", two_type, 0, 2, len(image))
        two_type[16:32] = store_id
        struct.pack_into("<QQQ", two_type, 32, position + 1, 0x40, creation_time)
        catalog[entry:entry + 128] = two_type
        entry += 128

        put(image, header_at * BLOCK, block_header(identifier, 4, header_at * BLOCK, 0, 128))
        descriptors = random_descriptors(rng)
        split = 508 if len(descriptors) > 20 and rng.random() < 0.5 else None
        first = descriptors if split is None else descriptors[:20]
        second = None if split is None else descriptors[20:]
        list_block = bytearray(BLOCK)
        list_block[0:128] = block_header(identifier, 3, list_at * BLOCK, list2_at * BLOCK if second else 0)
        for slot, record in enumerate(first):
            # Leave the occasional empty slot between records.
            struct.pack_into("<QQQII", list_block, 128 + 32 * (slot * 2 if rng.random() < 0.1 else slot), *record)
        put(image, list_at * BLOCK, list_block)
        if second:
            list_block = bytearray(BLOCK)
            list_block[0:128] = block_header(identifier, 3, list2_at * BLOCK, 0)
            for slot, record in enumerate(second):
                struct.pack_into("<QQQII", list_block, 128 + 32 * slot, *record)
            put(image, list2_at * BLOCK, list_block)

        put(image, bitmap_at * BLOCK, block_header(identifier, 6, bitmap_at * BLOCK, 0) + random_bitmap(rng))
        has_previous = rng.random() < 0.5
        if has_previous:
            put(image, previous_at * BLOCK, block_header(identifier, 6, previous_at * BLOCK, 0) + random_bitmap(rng))

        three_type = bytearray(128)
        struct.pack_into("<QQ", three_type, 0, 3, list_at * BLOCK)
        three_type[16:32] = store_id
        struct.pack_into("<QQQ", three_type, 32, header_at * BLOCK, 0, bitmap_at * BLOCK)
        struct.pack_into("<Q", three_type, 72, previous_at * BLOCK if has_previous else 0)
        catalog[entry:entry + 128] = three_type
        entry += 128
    put(image, 1 * BLOCK, catalog)
    return image


def main():
    out_dir, count, seed, source = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    with open(source, "rb") as handle:
        boot_sector = handle.read(512)
        handle.seek(0x1E00)
        identifier = handle.read(16)
    os.makedirs(out_dir, exist_ok=True)
    rng = random.Random(seed)
    errors = 0
    for k in range(count):
        image = build(rng, identifier, boot_sector)
        path = os.path.join(out_dir, f"rand_{k}.raw")
        with open(path, "wb") as handle:
            handle.write(image)
        status = ""
        try:
            volume = pyvshadow.volume()
            volume.open(path)
            for index in range(volume.number_of_stores):
                store = volume.get_store(index)
                data = store.read_buffer_at_offset(store.volume_size, 0)
                with open(os.path.join(out_dir, f"rand_{k}.store{index}.oracle.bin"), "wb") as handle:
                    handle.write(data)
            status = f"ok {volume.number_of_stores}"
            volume.close()
        except Exception as error:  # noqa: BLE001 - record libvshadow's verdict
            status = f"error {error}".replace("\n", " ")
            errors += 1
        with open(os.path.join(out_dir, f"rand_{k}.status"), "w", encoding="utf-8") as handle:
            handle.write(status)
    print(f"generated {count} images, libvshadow errors: {errors}")


if __name__ == "__main__":
    main()
