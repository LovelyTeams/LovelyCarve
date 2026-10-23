"""CC0-1.0 Sercomm NAND/ZIP fixtures, independently encoded with Python stdlib.

Format facts: OpenWrt v24.10.1 netgear_sercomm_nand and mksercommfw.c.
UBI headers/volume table follow Linux UBI media fields; no device data is used.
"""
from pathlib import Path
import hashlib
import io
import json
import struct
import zipfile
import zlib

ROOT = Path(__file__).resolve().parent
PEB, DATA = 131072, 4096


def jamseal(buffer):
    struct.pack_into(">I", buffer, len(buffer) - 4, zlib.crc32(buffer[:-4]) ^ 0xffffffff)


def ubi(payload):
    leb = PEB - DATA
    count = max(1, (len(payload) + leb - 1) // leb)
    table = bytearray(128 * 172)
    for index in range(128):
        record = bytearray(172)
        if index == 2:
            struct.pack_into(">III", record, 0, count, 1, 0)
            record[12] = 1
            struct.pack_into(">H", record, 14, 4)
            record[16:20] = b"root"
        jamseal(record)
        table[index * 172:(index + 1) * 172] = record
    result = bytearray(b"\xff" * ((count + 2) * PEB))
    for physical in range(count + 2):
        ec = bytearray(64)
        ec[:5] = b"UBI#\x01"
        struct.pack_into(">II", ec, 16, 2048, DATA)
        jamseal(ec)
        vid = bytearray(64)
        vid[:6] = b"UBI!\x01\x01"
        volume = 0x7fffefff if physical < 2 else 2
        number = physical if physical < 2 else count + 1 - physical
        struct.pack_into(">II", vid, 8, volume, number)
        jamseal(vid)
        base = physical * PEB
        result[base:base + 64] = ec
        result[base + 2048:base + 2112] = vid
        data = table if physical < 2 else payload[number * leb:(number + 1) * leb]
        result[base + DATA:base + DATA + len(data)] = data
    return result


def image(rootfs, compression):
    seal = bytearray(512)
    seal[:11] = b"sErCoMm\x00\x01\x00\x00"
    seal[11:14], seal[45:49] = b"AYA", b"A001"
    struct.pack_into(">I", seal, 55, 0x86)
    seal[63:70] = b"sErCoMm"
    kernel = b"independent synthetic MIPS kernel\n" * 8
    header = bytearray(struct.pack(">7I4B32s", 0x27051956, 0, 1600000000,
                                  len(kernel), 0, 0, zlib.crc32(kernel),
                                  5, 5, 2, 0, b"Synthetic kernel"))
    struct.pack_into(">I", header, 4, zlib.crc32(header))
    nand = bytearray(2 * 1024 * 1024) + header + kernel
    nand += bytes(6 * 1024 * 1024 - len(nand))
    nand += rootfs + seal[:70] + bytes(58)
    output = io.BytesIO()
    info = zipfile.ZipInfo("Synthetic.bin", (2020, 1, 1, 0, 0, 0))
    info.compress_type = compression
    info.create_system = 3
    info.external_attr = 0o100644 << 16
    with zipfile.ZipFile(output, "w") as archive:
        archive.writestr(info, nand)
    result = seal + output.getvalue()
    result[511] = (-sum(result)) & 255
    return bytes(result)


if __name__ == "__main__":
    filesystem = (ROOT.parents[1] / "vendor/backhand/test_data/ddwrt-v3.squashfs").read_bytes()
    rootfs = ubi(filesystem)
    deflated = image(rootfs, zipfile.ZIP_DEFLATED)
    stored = image(rootfs, zipfile.ZIP_STORED)
    (ROOT / "synthetic-sercomm-deflate.img").write_bytes(deflated)
    (ROOT / "synthetic-sercomm-stored.img.zlib").write_bytes(zlib.compress(stored, 9))
    records = {"rootfsSize": len(rootfs), "rootfsSha256": hashlib.sha256(rootfs).hexdigest(),
               "storedSourceSize": len(stored), "storedSourceSha256": hashlib.sha256(stored).hexdigest(),
               "fixtures": {}}
    for name in ["synthetic-sercomm-deflate.img", "synthetic-sercomm-stored.img.zlib"]:
        content = (ROOT / name).read_bytes()
        records["fixtures"][name] = {"size": len(content), "sha256": hashlib.sha256(content).hexdigest()}
    (ROOT / "sercomm-fixtures.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(records, indent=2))
