"""Independent PyCryptodome/cryptography vectors; no Rust encoder is used.

Compact projections contain a boot sector, FVE block and two encrypted sectors.
Tests expand these into an in-memory volume. Synthetic format interoperability,
not a claim that Windows issued these images. Run with Python 3 + pycryptodome
and cryptography. Elephant is the published A/B algorithm, modulo 2**32.
"""
from pathlib import Path
import hashlib
import struct
from Crypto.Cipher import AES
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes


def entry(kind, value, data):
    return struct.pack("<4H", len(data) + 8, kind, value, 1) + data


def wrap(key, nonce, data):
    cipher = AES.new(key, AES.MODE_CCM, nonce=nonce, mac_len=16)
    encrypted = cipher.encrypt(data)
    return nonce + cipher.digest() + encrypted


def rotate(value, bits):
    return ((value << bits) | (value >> ((32 - bits) % 32))) & 0xffffffff


def encrypt(method, size, offset, plain, fvek, tweak):
    if method >= 0x8004:
        cipher = Cipher(algorithms.AES(fvek), modes.XTS(struct.pack("<Q", offset // size) + bytes(8))).encryptor()
        return cipher.update(plain) + cipher.finalize()
    iv_data = struct.pack("<Q", offset) + bytes(8)
    if method <= 0x8001:
        key = AES.new(tweak, AES.MODE_ECB)
        sector_key = key.encrypt(iv_data) + key.encrypt(iv_data[:15] + b"\x80")
        mixed = bytes(value ^ sector_key[index % 32] for index, value in enumerate(plain))
        words = list(struct.unpack(f"<{size // 4}I", mixed))
        count = len(words)
        for cycles, left, right, rotations in [(5, -2, -5, [9, 0, 13, 0]), (3, 2, 5, [0, 10, 0, 25])]:
            for _ in range(cycles):
                for index in reversed(range(count)):
                    words[index] = (words[index] - (words[(index + left) % count] ^ rotate(words[(index + right) % count], rotations[index % 4]))) & 0xffffffff
        plain = struct.pack(f"<{count}I", *words)
    iv = AES.new(fvek, AES.MODE_ECB).encrypt(iv_data)
    return AES.new(fvek, AES.MODE_CBC, iv=iv).encrypt(plain)


def main():
    directory = Path(__file__).parent
    manifest = []
    for method in range(0x8000, 0x8006):
        for size in [512, 4096]:
            key_size = 64 if method == 0x8005 else 32 if method in [0x8001, 0x8003, 0x8004] else 16
            fvek = bytes(range(1, key_size + 1))
            tweak = bytes(range(65, 65 + (16 if method == 0x8000 else 32))) if method <= 0x8001 else b""
            vmk, clear = bytes([0x46]) * 32, bytes([0x55]) * 32
            key_container = entry(0, 1, struct.pack("<I", method) + (fvek.ljust(32, b"\0") + tweak if tweak else fvek))
            entries = entry(15, 15, struct.pack("<QQ", 0x10000, size))
            for protector in [0, 0x0200]:
                properties = entry(0, 1, bytes(4) + clear) if protector == 0 else b""
                properties += entry(0, 5, wrap(clear, bytes([0x57]) * 12, entry(0, 1, bytes(4) + vmk)))
                entries += entry(2, 8, bytes(26) + struct.pack("<H", protector) + properties)
            entries += entry(3, 5, wrap(vmk, bytes([0x68]) * 12, key_container))
            boot = bytearray(512)
            boot[:3], boot[3:11] = b"\xeb\x58\x90", b"-FVE-FS-"
            struct.pack_into("<H", boot, 11, size)
            boot[160:176] = bytes.fromhex("3bd66749292ed84a8399f6a339e3d001")
            struct.pack_into("<3Q", boot, 176, 0x20000, 0x30000, 0x40000)
            block = bytearray(112 + len(entries))
            block[:8] = b"-FVE-FS-"
            struct.pack_into("<H", block, 10, 2)
            struct.pack_into("<Q", block, 16, 0x60000)
            struct.pack_into("<I", block, 28, 1)
            struct.pack_into("<4Q", block, 32, 0x20000, 0x30000, 0x40000, 0x10000)
            struct.pack_into("<I", block, 64, 48 + len(entries))
            struct.pack_into("<H", block, 100, method)
            block[112:] = entries
            plain = bytes((index * 7 + 19) % 256 for index in range(size))
            # A real NTFS signature lets the application reject incorrect raw keys.
            plain = plain[:3] + b"NTFS    " + plain[11:]
            sectors = b"".join(encrypt(method, size, offset, plain, fvek, tweak) for offset in [0x10000, 0x12000])
            payload = struct.pack("<HHI", method, size, len(block)) + boot + block + sectors
            name = f"parity-{method:04x}-{size}.bin"
            directory.joinpath(name).write_bytes(payload)
            manifest.append(f"{hashlib.sha256(payload).hexdigest()}  {name}")
    bek = bytes(48) + entry(0, 9, bytes(24) + entry(0, 1, bytes(4) + bytes([0x55]) * 32))
    directory.joinpath("parity-startup.bek").write_bytes(bek)
    manifest.append(f"{hashlib.sha256(bek).hexdigest()}  parity-startup.bek")
    directory.joinpath("parity-sha256.txt").write_text("\n".join(manifest) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
