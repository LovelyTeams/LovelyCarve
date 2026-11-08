"""Invented QCOW fixtures: independent Python zlib/OpenSSL encoders.

Requires cryptography. No upstream or real evidence data. AGPL-3.0-only.
Run from any directory; outputs are deterministic, beside this script.
"""
from pathlib import Path
import hashlib
import json
import struct
import zlib
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

ROOT = Path(__file__).resolve().parent
CLUSTER = 4096
KEY = b"qcow-test" + bytes(7)
PLAIN = bytes((index * 7 + index // 23) % 251 for index in range(CLUSTER))


def put(image, offset, fmt, value):
    struct.pack_into(">" + fmt, image, offset, value)


def header(version, encrypted=False):
    image = bytearray(CLUSTER * 4)
    put(image, 0, "I", 0x514649FB)
    put(image, 4, "I", version)
    put(image, 24, "Q", CLUSTER * 2)
    put(image, 40, "Q", CLUSTER)
    if version == 1:
        image[32:34] = bytes([12, 9])
        put(image, 36, "I", int(encrypted))
    else:
        put(image, 20, "I", 12)
        put(image, 32, "I", int(encrypted))
        put(image, 36, "I", 1)
    if version == 3:
        put(image, 96, "I", 4)
        put(image, 100, "I", 112)
    put(image, CLUSTER, "Q", CLUSTER * 2)
    return image


def compressed(version, plain=PLAIN):
    image = header(version)
    # QEMU's raw DEFLATE uses window_bits=12; decoding uses a different Rust engine.
    encoder = zlib.compressobj(9, zlib.DEFLATED, -12)
    payload = encoder.compress(plain) + encoder.flush()
    position = CLUSTER * 3 + 497  # Unaligned; spans multiple 512-byte sectors.
    shift = 63 - 12 if version == 1 else 70 - 12
    size = len(payload) if version == 1 else (len(payload) + (position % 512) + 511) // 512 - 1
    flag = 1 << (63 if version == 1 else 62)
    put(image, CLUSTER * 2, "Q", flag | (size << shift) | position)
    image[position:position + len(payload)] = payload
    # Physical EOF immediately after DEFLATE: QCOW2 need not retain sector padding.
    return image[:position + len(payload)]


def encrypted(version):
    image = header(version, True)
    put(image, CLUSTER * 2 + 8, "Q", CLUSTER * 3)
    for sector in range(CLUSTER // 512):
        iv = (CLUSTER // 512 + sector).to_bytes(8, "little") + bytes(8)
        encryptor = Cipher(algorithms.AES(KEY), modes.CBC(iv)).encryptor()
        payload = encryptor.update(PLAIN[sector * 512:(sector + 1) * 512]) + encryptor.finalize()
        image[CLUSTER * 3 + sector * 512:CLUSTER * 3 + (sector + 1) * 512] = payload
    return image


files = {"plain.bin": PLAIN}
for version in (1, 2, 3):
    files[f"v{version}-deflate.qcow"] = compressed(version)
    files[f"v{version}-aes.qcow"] = encrypted(version)
files["v2-deflate-short.qcow"] = compressed(2, PLAIN[:-1])
files["v2-deflate-long.qcow"] = compressed(2, PLAIN + b"x")
for name, data in files.items():
    (ROOT / name).write_bytes(data)
manifest = {name: {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in files.items()}
(ROOT / "SHA256.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
