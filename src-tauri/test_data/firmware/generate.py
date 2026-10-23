"""Original CC0-1.0 synthetic Buffalo containers, generated independently of Rust.
Only Python stdlib is used. No downloaded/real firmware data is included.
"""
from pathlib import Path
import struct
import zlib

here = Path(__file__).resolve().parent
filesystem = here.parents[1] / "vendor/backhand/test_data/ddwrt-v3.squashfs"


def rc4(data, seed):
    key = bytes([seed]) + b"Buffalo"
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    i = j = 0
    result = bytearray()
    for byte in data:
        i = (i + 1) % 256
        j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        result.append(byte ^ state[(state[i] + state[j]) % 256])
    return result


def envelope(data):
    product = rc4(b"SyntheticRouter\0", 148)
    version = rc4(b"1.0\0", product[0])
    header = b"start\0\x94" + struct.pack(">I", len(product)) + product
    header += struct.pack(">I", len(version)) + version + struct.pack(">I", len(data))
    checksum = len(data)
    for byte in data:
        checksum ^= (byte if byte < 128 else byte - 256) & 0xffffffff
        for _ in range(8):
            checksum = (checksum >> 1) ^ (0xedb88320 if checksum & 1 else 0)
    result = header + rc4(data, version[0]) + struct.pack(">I", checksum)
    return result + bytes(-len(result) % 4)


def firmware(payload):
    trx = bytearray(b"HDR0" + bytes(24)) + payload
    struct.pack_into("<I", trx, 4, len(trx))
    struct.pack_into("<I", trx, 12, 0x10000)
    struct.pack_into("<I", trx, 16, 28)
    struct.pack_into("<I", trx, 8, zlib.crc32(trx[12:]) ^ 0xffffffff)
    plaintext = b"buffalo.enc\nfilelen=" + str(len(trx)).encode() + b"\n" + trx
    enc = envelope(plaintext)
    bgn = bytearray(b"bgn\0" + bytes(16)) + envelope(b"SyntheticRouter\0" + bytes(136)) + enc
    struct.pack_into(">I", bgn, 12, len(bgn))
    return enc, bgn


if __name__ == "__main__":
    enc, bgn = firmware(filesystem.read_bytes())
    (here / "synthetic.enc").write_bytes(enc)
    (here / "synthetic-bgn.enc").write_bytes(bgn)
    ubifs = here.parents[1] / "vendor/ubidump/test_data/synthetic-deflate.ubifs"
    if ubifs.exists():
        (here / "synthetic-ubifs.enc").write_bytes(firmware(ubifs.read_bytes())[0])
    jffs2 = here.parents[1] / "vendor/jffs2/test_data/synthetic-be-zlib.jffs2"
    if jffs2.exists():
        (here / "synthetic-jffs2.enc").write_bytes(firmware(jffs2.read_bytes())[0])
    legacy = here.parents[1] / "vendor/backhand/test_data/legacy/v2.1-be-zlib.squashfs"
    if legacy.exists():
        (here / "synthetic-legacy-squashfs.enc").write_bytes(firmware(legacy.read_bytes())[0])
