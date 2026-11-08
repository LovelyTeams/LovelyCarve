"""Regenerate a synthetic recovery-password CBC fixture using PyCryptodome.

Only used to regenerate test bytes, never at runtime. Unlike the Rust reader,
this independent crypto implementation produces the AES-CCM/CBC ciphertext.
This tests interoperability of crypto and metadata wiring, not Windows issuance.
"""
import hashlib
from pathlib import Path
import struct
from Crypto.Cipher import AES


def entry(kind, value, data):
    return struct.pack("<4H", len(data) + 8, kind, value, 1) + data


def wrap(key, nonce, data):
    cipher = AES.new(key, AES.MODE_CCM, nonce=nonce, mac_len=16)
    encrypted = cipher.encrypt(data)
    return nonce + cipher.digest() + encrypted


def main():
    salt, vmk, fvek = bytes([0x37]) * 16, bytes([0x46]) * 32, bytes([0x13]) * 16
    initial = hashlib.sha256(struct.pack("<8H", *([111111 // 11] * 8))).digest()
    last = bytes(32)
    for counter in range(0x100000):
        last = hashlib.sha256(last + initial + salt + struct.pack("<Q", counter)).digest()
    vmk_data = bytes(26) + struct.pack("<H", 0x0800)
    vmk_data += entry(0, 3, bytes(4) + salt)
    vmk_data += entry(0, 5, wrap(last, bytes([0x57]) * 12, bytes(12) + vmk))
    entries = entry(15, 15, struct.pack("<QQ", 0x4000, 512))
    entries += entry(2, 8, vmk_data)
    entries += entry(3, 5, wrap(vmk, bytes([0x68]) * 12, bytes(12) + fvek + bytes(16)))
    image = bytearray(0x8000)
    image[:3], image[3:11] = b"\xeb\x58\x90", b"MSWIN4.1"
    struct.pack_into("<H", image, 11, 512)
    image[424:440] = bytes.fromhex("3bd66749292ed84a8399f6a339e3d001")
    struct.pack_into("<Q", image, 440, 0x1000)
    image[0x1000:0x1008] = b"-FVE-FS-"
    struct.pack_into("<H", image, 0x100a, 2)
    struct.pack_into("<Q", image, 0x1010, 0x7000)
    struct.pack_into("<I", image, 0x101c, 1)
    struct.pack_into("<Q", image, 0x1020, 0x1000)
    struct.pack_into("<Q", image, 0x1038, 0x4000)
    struct.pack_into("<I", image, 0x1040, 48 + len(entries))
    struct.pack_into("<H", image, 0x1064, 0x8002)
    image[0x1070:0x1070 + len(entries)] = entries
    plaintext = bytes((index & 255) ^ 0x3c for index in range(512))
    iv = AES.new(fvek, AES.MODE_ECB).encrypt(struct.pack("<Q", 0x4000) + bytes(8))
    image[0x4000:0x4200] = AES.new(fvek, AES.MODE_CBC, iv=iv).encrypt(plaintext)
    Path(__file__).with_name("recovery-cbc.bin").write_bytes(image)
    print("plaintext SHA-256:", hashlib.sha256(plaintext).hexdigest())


if __name__ == "__main__":
    main()
