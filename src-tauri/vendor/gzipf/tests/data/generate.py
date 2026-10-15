"""Reproduce invented gzip fixtures using Python's independent C zlib backend.

No libgzipf source, fixture or real evidence is included. Run from any directory.
SPDX-License-Identifier: AGPL-3.0-only
"""
import hashlib
from pathlib import Path
import struct
import zlib


ROOT = Path(__file__).resolve().parent
PAYLOAD = (b"LovelyCarve independent gzip fixture.\n" * 4096
           + bytes(range(256)) * 16)


def member(payload, level=6, strategy=zlib.Z_DEFAULT_STRATEGY, optional=False):
    header = bytearray(b"\x1f\x8b\x08" + bytes([0x1f if optional else 0])
                       + struct.pack("<I", 1_700_000_000 if optional else 0)
                       + b"\x00\x03")
    if optional:
        extra = b"LC\x04\x00\x01\x02\x03\x04"
        header += struct.pack("<H", len(extra)) + extra
        header += b"caf\xe9.log\x00"
        header += b"na\xefve fixture\x00"
        header += struct.pack("<H", zlib.crc32(header) & 0xffff)
    compressor = zlib.compressobj(level, zlib.DEFLATED, -15, 8, strategy)
    body = compressor.compress(payload) + compressor.flush()
    return bytes(header) + body + struct.pack("<II", zlib.crc32(payload),
                                             len(payload) & 0xffffffff)


files = {
    "stored.gz": member(PAYLOAD, 0),
    "fixed.gz": member(PAYLOAD, 6, zlib.Z_FIXED),
    "dynamic.gz": member(PAYLOAD),
    "optional.gz": member(PAYLOAD, optional=True),
    "concatenated.gz": (member(b"") + member(PAYLOAD[:80000], optional=True)
                        + member(b"") + member(PAYLOAD[80000:]) + member(b"")),
}
hashes = []
for name, data in files.items():
    assert zlib.decompress(data, 31) is not None
    (ROOT / name).write_bytes(data)
    hashes.append(f"{hashlib.sha256(data).hexdigest()}  {name}")
(ROOT / "SHA256SUMS").write_text("\n".join(hashes) + "\n", encoding="ascii")
print(f"Generated {len(files)} fixtures; Python zlib {zlib.ZLIB_RUNTIME_VERSION}")
