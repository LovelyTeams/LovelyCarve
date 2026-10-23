# SPDX-License-Identifier: CC0-1.0
"""Independent MINIFS v3 encoder/oracle using Python's liblzma.

No vendor firmware is committed. Run with --firmware and --output to produce a
raw MINIFS image and a TSV of logical paths, sizes and SHA-256 hashes.
"""
import argparse
import hashlib
import lzma
from pathlib import Path
import struct


def fixture():
    names = b"web/js\0app.js\0empty\0\0hello.txt\0unicode\0\xe6\xb5\x8b\xe8\xaf\x95.txt\0"
    payload = bytes(range(256)) * 289 + b"MINIFS test\n"
    bodies = [payload + b"hello\n", "\u4f60\u597d MINIFS\n".encode()]
    files = [(0, 7, 0, 0, len(payload)), (0, 14, 0, len(payload), 0),
             (20, 21, 0, len(payload), 6), (31, 39, 1, 0, len(bodies[1]))]
    packed = [lzma.compress(body, format=lzma.FORMAT_ALONE) for body in bodies]
    header = b"MINIFS" + bytes(10) + struct.pack(">4I", 3, len(files), 0, len(names))
    records = b"".join(struct.pack(">5I", *file) for file in files)
    chunks = b""
    offset = 0
    for raw, encoded in zip(bodies, packed):
        chunks += struct.pack(">3I", offset, len(encoded), len(raw))
        offset += len(encoded)
    output = Path("src-tauri/vendor/minifs/tests/fixtures")
    output.mkdir(parents=True, exist_ok=True)
    (output / "synthetic.minifs").write_bytes(header + names + records + chunks + b"".join(packed))


def oracle(firmware, output):
    source = Path(firmware).read_bytes()
    base = source.index(b"MINIFS" + bytes(10))
    version, count, _, names_size = struct.unpack_from(">4I", source, base + 16)
    assert version == 3 and 0 < count <= 100000
    names = source[base + 32:base + 32 + names_size]
    table = base + 32 + names_size
    files = [struct.unpack_from(">5I", source, table + index * 20) for index in range(count)]
    chunk_count = max(file[2] for file in files) + 1
    chunk_table = table + count * 20
    data = chunk_table + chunk_count * 12
    chunks = []
    end = data
    for index in range(chunk_count):
        offset, packed, unpacked = struct.unpack_from(">3I", source, chunk_table + index * 12)
        decoder = lzma.LZMADecompressor(format=lzma.FORMAT_ALONE)
        decoded = decoder.decompress(source[data + offset:data + offset + packed])
        assert decoder.eof and not decoder.unused_data and len(decoded) == unpacked
        chunks.append(decoded)
        end = max(end, data + offset + packed)
    manifest = []
    for directory, filename, chunk, offset, length in files:
        directory = names[directory:names.index(0, directory)].decode()
        filename = names[filename:names.index(0, filename)].decode()
        path = "/" + "/".join(filter(None, [directory, filename]))
        body = chunks[chunk][offset:offset + length]
        assert len(body) == length
        manifest.append(f"{path}\t{length}\t{hashlib.sha256(body).hexdigest()}")
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.with_suffix(".minifs").write_bytes(source[base:end])
    output.with_suffix(".tsv").write_text("\n".join(manifest) + "\n", encoding="utf-8")
    print(f"source_sha256={hashlib.sha256(source).hexdigest()} base={base:#x} "
          f"length={end-base} files={count} chunks={chunk_count} "
          f"minifs_sha256={hashlib.sha256(source[base:end]).hexdigest()}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware")
    parser.add_argument("--output", default="fixtures/minifs/oracle")
    args = parser.parse_args()
    if args.firmware:
        oracle(args.firmware, args.output)
    else:
        fixture()
