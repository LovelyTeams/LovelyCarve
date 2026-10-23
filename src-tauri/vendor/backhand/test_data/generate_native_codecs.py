"""Independent native-codec fixtures. Original CC0-1.0 data; Python stdlib XZ."""
import hashlib
import json
import lzma
import struct
import zlib
import zstandard
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DATA = bytes(range(256)) * 64
outputs = {}


def save(name, data):
    path = ROOT / ("native-" + name + ".xz")
    path.write_bytes(data)
    outputs[path.name] = hashlib.sha256(data).hexdigest()


for name, prefilter in [
    ("plain", None),
    ("x86", lzma.FILTER_X86),
    ("powerpc", lzma.FILTER_POWERPC),
    ("ia64", lzma.FILTER_IA64),
    ("arm", lzma.FILTER_ARM),
    ("armthumb", lzma.FILTER_ARMTHUMB),
    ("sparc", lzma.FILTER_SPARC),
]:
    filters = [] if prefilter is None else [{"id": prefilter}]
    filters += [{"id": lzma.FILTER_LZMA2, "dict_size": 65536}]
    save(name, lzma.compress(DATA, filters=filters, check=lzma.CHECK_CRC32))
for name, check in [("crc64", lzma.CHECK_CRC64), ("sha256", lzma.CHECK_SHA256), ("no-check", lzma.CHECK_NONE)]:
    save(name, lzma.compress(DATA, check=check))
save("large-output", lzma.compress(b"A" * (4 * 1024 * 1024 + 1)))
save("large-dictionary", lzma.compress(DATA, filters=[{"id": lzma.FILTER_LZMA2, "dict_size": 64 * 1024 * 1024}]))

save("delta-x86", lzma.compress(DATA, filters=[{"id": lzma.FILTER_DELTA, "dist": 2}, {"id": lzma.FILTER_X86}, {"id": lzma.FILTER_LZMA2, "dict_size": 65536}], check=lzma.CHECK_CRC32))
save("empty", lzma.compress(b""))

# Footer CRC is valid, but its declared index location is false.
footer = bytearray((ROOT / "native-plain.xz").read_bytes())
footer[-8] ^= 1
struct.pack_into("<I", footer, len(footer) - 12, zlib.crc32(footer[-8:-2]))
save("bad-footer", footer)

# Correct stream and payload checksum, but a CRC-valid lying index.
bad = bytearray((ROOT / "native-plain.xz").read_bytes())
index_size = (struct.unpack_from("<I", bad, len(bad) - 8)[0] + 1) * 4
index = len(bad) - 12 - index_size
position = index + 2
while bad[position] & 128:
    position += 1
position += 1  # first byte of uncompressed size
bad[position] ^= 1
struct.pack_into("<I", bad, len(bad) - 16, zlib.crc32(bad[index:len(bad)-16]))
save("bad-index", bad)


def integer(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    return result + bytes([value])


def stream(blocks, records):
    index = bytearray(b"\0") + integer(len(records))
    for unpadded, uncompressed in records:
        index += integer(unpadded) + integer(uncompressed)
    index += bytes((-len(index)) % 4)
    index += struct.pack("<I", zlib.crc32(index))
    flags = struct.pack("<I", len(index) // 4 - 1) + b"\0\1"
    return plain[:12] + b"".join(blocks) + index + struct.pack("<I", zlib.crc32(flags)) + flags + b"YZ"


plain = (ROOT / "native-plain.xz").read_bytes()
index_size = (struct.unpack_from("<I", plain, len(plain) - 8)[0] + 1) * 4
block = plain[12:len(plain) - 12 - index_size]
header_size = (block[0] + 1) * 4
# The independent encoder's single index record supplies unpadded block size.
index_data = plain[len(plain) - 12 - index_size:-12]
assert index_data[:2] == b"\0\1"
unpadded = 0
for shift, value in enumerate(index_data[2:]):
    unpadded |= (value & 127) << (7 * shift)
    if value < 128:
        break
compressed = unpadded - header_size - 4
multi = stream([block, block], [(unpadded, len(DATA))] * 2)
assert lzma.decompress(multi) == DATA * 2
save("multi-block", multi)

for name, declared in [("with-sizes", compressed), ("bad-block-size", compressed + 1)]:
    header = bytearray(b"\0\xc0") + integer(declared) + integer(len(DATA)) + block[2:5]
    header += bytes((-(len(header) + 4)) % 4)
    header[0] = (len(header) + 4) // 4 - 1
    header += struct.pack("<I", zlib.crc32(header))
    encoded = stream([header + block[header_size:]], [(len(header) + compressed + 4, len(DATA))])
    if name == "with-sizes":
        assert lzma.decompress(encoded) == DATA
    save(name, encoded)

# Independent oracle: Python zstandard 0.25.0 (libzstd).
# Only fixture generation uses it; no Python/native decoder ships with the reader.
for name, data in [("checksum", DATA), ("large-output", b"A" * (4 * 1024 * 1024 + 1))]:
    encoded = zstandard.ZstdCompressor(write_checksum=True).compress(data)
    path = ROOT / ("native-" + name + ".zst")
    path.write_bytes(encoded)
    outputs[path.name] = hashlib.sha256(encoded).hexdigest()
# Streaming compression records an explicit 16 MiB window, above our bound.
compressor = zstandard.ZstdCompressor(compression_params=zstandard.ZstdCompressionParameters(window_log=24))
context = compressor.compressobj()
encoded = context.compress(DATA) + context.flush()
path = ROOT / "native-large-window.zst"
path.write_bytes(encoded)
outputs[path.name] = hashlib.sha256(encoded).hexdigest()
(ROOT / "native-codecs.json").write_text(json.dumps({"generator": "Python stdlib lzma and zstandard 0.25.0", "license": "CC0-1.0", "payloadSha256": hashlib.sha256(DATA).hexdigest(), "files": outputs}, indent=2) + "\n")
