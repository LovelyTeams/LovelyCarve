"""CC0-1.0 independent SquashFS 1.0/2.0/2.1 wire encoder.

Python stdlib produces bit fields, metadata and zlib/EOS LZMA. Optional 7-Zip
or NanaZip produces markerless LZMA in a plain single-stream 7z container;
Python/liblzma independently verifies its output and the lack of an EOS marker.
No backhand or SquashFS writer dependency. See legacy/README.md.
"""
from pathlib import Path
import argparse
import hashlib
import lzma
import struct
import subprocess
import tempfile
import zlib

ROOT = Path(__file__).resolve().parent / "legacy"
BASE = bytes((index * 17 + 3) & 255 for index in range(4096))
TAIL = (b"independent tail\n" * 60)[:809]
PAYLOAD = BASE + bytes(4096) + TAIL


def without_marker(data, executable):
    folder = Path(tempfile.mkdtemp(prefix="lovelycarve-legacy-lzma-"))
    (folder / "payload.bin").write_bytes(data)
    result = subprocess.run([executable, "a", "-t7z", "-m0=LZMA:d=4k:lc=3:lp=0:pb=2",
                             "-mx=3", "-mhc=off", "-ms=off", str(folder / "block.7z"),
                             str(folder / "payload.bin")], capture_output=True)
    assert result.returncode == 0, result.stdout + result.stderr
    archive = (folder / "block.7z").read_bytes()
    assert archive[:6] == b"7z\xbc\xaf\x27\x1c"
    assert zlib.crc32(archive[12:32]) == struct.unpack_from("<I", archive, 8)[0]
    length, header_size, checksum = struct.unpack_from("<QQI", archive, 12)
    header = archive[32 + length:32 + length + header_size]
    assert len(archive) == 32 + length + header_size and zlib.crc32(header) == checksum
    encoded = archive[32:32 + length]
    decoder = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=[
        dict(id=lzma.FILTER_LZMA1, dict_size=4096, lc=3, lp=0, pb=2)])
    assert decoder.decompress(encoded) == data and not decoder.eof and decoder.needs_input
    return b"\x5d\0\x10\0\0" + encoded


class Writer:
    def __init__(self, version, minor, big, stored, check, codec="zlib", sevenzip=None, block_size=4096):
        self.version, self.minor, self.big = version, minor, big
        self.order = ">" if big else "<"
        self.stored, self.check, self.codec = stored, check, codec
        self.sevenzip, self.block_size = sevenzip, block_size
        self.header_size = 51 if version == 1 else 63
        self.blocks = bytearray()

    def pack(self, fmt, *values):
        return struct.pack(self.order + fmt, *values)

    def fields(self, values):
        # C-compatible bit placement: fields advance MSB first for BE, LSB for LE.
        output = bytearray((sum(count for _, count in values) + 7) // 8)
        offset = 0
        for value, count in values:
            assert 0 <= value < (1 << count)
            for index in range(count):
                bit = offset + index
                shift = count - 1 - index if self.big else index
                if (value >> shift) & 1:
                    output[bit // 8] |= 1 << (7 - bit % 8 if self.big else bit % 8)
            offset += count
        return bytes(output)

    def base(self, kind, mode, uid=35):
        if self.version == 1:
            if kind in (6, 7):
                return self.fields([(0, 4), (mode, 12), (uid % 16, 4), (15, 4), (kind, 4), (uid // 16, 4)])
            return self.fields([(kind + (uid // 16) * 5, 4), (mode, 12), (uid % 16, 4), (15, 4)])
        return self.fields([(kind, 4), (mode, 12), (uid, 8), (255, 8)])

    def compress(self, data):
        if self.codec == "known":
            return without_marker(data, self.sevenzip)
        if self.codec == "props":
            return b"\x5d\0\x10\0\0" + lzma.compress(data, format=lzma.FORMAT_RAW, filters=[
                dict(id=lzma.FILTER_LZMA1, dict_size=4096, lc=3, lp=0, pb=2)])
        return zlib.compress(data, 3)

    def block(self, data, stored):
        start = self.header_size + len(self.blocks)
        encoded = data if stored else self.compress(data)
        self.blocks.extend(encoded)
        raw = len(encoded) | (0x1000000 if stored else 0)
        return start, raw

    def metadata(self, data):
        output, offsets = bytearray(), []
        for offset in range(0, len(data), 8192):
            offsets.append(len(output))
            plain = data[offset:offset + 8192]
            encoded = plain if self.stored else zlib.compress(plain, 3)
            output.extend(self.pack("H", len(encoded) | (0x8000 if self.stored else 0)))
            if self.check:
                output.append(0xff)
            output.extend(encoded)
        return bytes(output), offsets

    def reference(self, offsets, logical):
        return (offsets[logical // 8192] << 16) | (logical % 8192)

    def directory(self, entries, inode_offsets):
        # v1 and v2.0 deliberately contain unsorted names.
        if self.minor == 1:
            entries = sorted(entries)
        output = bytearray()
        for name, logical, kind in entries:
            reference = self.reference(inode_offsets, logical)
            name = name.encode()
            output.extend(self.fields([(0, 8), (reference >> 16, 24)]))
            output.extend(self.fields([(reference & 0xffff, 13), (kind, 3), (len(name) - 1, 8)]))
            output.extend(name)
        return bytes(output)

    def image(self, payload):
        no_fragment = self.version == 1 or self.codec == "known"
        chunks = [payload[offset:offset + self.block_size] for offset in range(0, len(payload), self.block_size)]
        regular = chunks if no_fragment else chunks[:-1]
        data_sizes, first = [], None
        for index, chunk in enumerate(regular):
            start, raw = self.block(chunk, stored=(index == 0))
            if first is None:
                first = start
            data_sizes.append(raw)
        fragment = None if no_fragment else self.block(b"prefix!" + chunks[-1], stored=False)
        # The first file inode crosses an 8 KiB metadata boundary after this link.
        inodes = bytearray(self.base(3, 0o777, 2) + self.pack("H", 8180) + b"p" * 8180)
        payload_at = len(inodes)
        inodes.extend(self.base(2, 0o644) + self.pack("II", 1700000002, first or 0))
        if self.version == 2:
            inodes.extend(self.pack("II", 0xffffffff if no_fragment else 0, 0 if no_fragment else 7))
        inodes.extend(self.pack("I", len(payload)))
        for raw in data_sizes:
            if self.version == 1:
                length = raw & 0xffffff
                inodes.extend(self.pack("H", (length & 0x7fff) | (0x8000 if raw & 0x1000000 else 0)))
            else:
                inodes.extend(self.pack("I", raw))
        empty_at = len(inodes)
        inodes.extend(self.base(2, 0o600) + self.pack("II", (-123) & 0xffffffff, 0))
        if self.version == 2:
            inodes.extend(self.pack("II", 0xffffffff, 0))
        inodes.extend(self.pack("I", 0))
        link_at = len(inodes)
        inodes.extend(self.base(3, 0o777) + self.pack("H", 11) + b"payload.bin")
        specials = []
        for name, kind in [("block", 4), ("char", 5), ("fifo", 6), ("socket", 7)]:
            specials.append((name, len(inodes), kind))
            inodes.extend(self.base(kind, 0o600))
            if kind in (4, 5):
                inodes.extend(self.pack("H", 0x102))
        child_at = len(inodes)
        # Child directory is at byte zero of the directory stream; its size is fixed.
        child_size = 4 + 3 + len("tail.txt")
        inodes.extend(self.base(1, 0o755) + self.fields([(child_size, 19), (0, 13)])
                      + self.pack("I", 1700000004) + self.fields([(0, 24)]))
        root_at = len(inodes)
        root_entries = [("payload.bin", payload_at, 2), ("alias.bin", payload_at, 2),
                        ("empty.txt", empty_at, 2), ("link", link_at, 3),
                        ("子目录", child_at, 1), ("huge-link", 0, 3)] + specials
        root_size = sum(7 + len(name.encode()) for name, _, _ in root_entries)
        if self.minor == 1:
            inodes.extend(self.base(8, 0o755, 0) + self.fields([(root_size, 27), (child_size, 13)])
                          + self.pack("I", 1700000005) + self.fields([(0, 24)]) + self.pack("H", 1))
            name = sorted(name for name, _, _ in root_entries)[0].encode()
            inodes.extend(self.fields([(0, 27), (0, 29), (len(name) - 1, 8)]) + name)
        else:
            inodes.extend(self.base(1, 0o755, 0) + self.fields([(root_size, 19), (child_size, 13)])
                          + self.pack("I", 1700000005) + self.fields([(0, 24)]))
        inode_data, offsets = self.metadata(inodes)
        directory = self.directory([("tail.txt", payload_at, 2)], offsets) + self.directory(root_entries, offsets)
        directory_data, _ = self.metadata(directory)
        inode_start = self.header_size + len(self.blocks)
        directory_start = inode_start + len(inode_data)
        body = bytes(self.blocks) + inode_data + directory_data
        fragment_start = self.header_size + len(body)
        if fragment:
            raw = self.pack("II", *fragment)
            encoded, _ = self.metadata(raw)
            body += encoded
            fragment_start = self.header_size + len(body)
            body += self.pack("I", self.header_size + len(self.blocks) + len(inode_data) + len(directory_data))
        uid_start = self.header_size + len(body)
        body += self.pack("48I", *(1000 + index for index in range(48)))
        length = self.header_size + len(body)
        header = bytearray(self.pack("7I4H3B", 0x73717368, 10, length, uid_start, 0, inode_start,
                                     directory_start, self.version, self.minor,
                                     self.block_size if self.version == 1 else 0,
                                     self.block_size.bit_length() - 1, 1 | (4 if self.check else 0), 48, 0))
        header.extend(self.pack("IQ", 1600000000, self.reference(offsets, root_at)))
        if self.version == 2:
            header.extend(self.pack("III", self.block_size, int(fragment is not None), fragment_start))
        assert len(header) == self.header_size
        return bytes(header) + body


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--sevenzip", help="7z-compatible executable, for markerless LZMA fixtures")
    args = parser.parse_args()
    ROOT.mkdir(exist_ok=True)
    (ROOT / "payload.bin").write_bytes(PAYLOAD)
    (ROOT / "eos-length.block").write_bytes(Writer(2, 0, False, False, False, codec="props").compress(TAIL))
    for version, minor in [(1, 0), (2, 0), (2, 1)]:
        for big, label in [(False, "le"), (True, "be")]:
            for stored in [False, True]:
                mode = "stored" if stored else "zlib"
                writer = Writer(version, minor, big, stored, check=stored)
                (ROOT / f"v{version}.{minor}-{label}-{mode}.squashfs").write_bytes(writer.image(PAYLOAD))
    (ROOT / "v2.0-le-lzma-props.squashfs").write_bytes(Writer(2, 0, False, False, False, codec="props").image(PAYLOAD))
    for version, size, label in [(1, 32768, "32k"), (2, 65536, "64k")]:
        payload = (BASE * (size // len(BASE))) + bytes(size) + TAIL
        (ROOT / f"payload-{label}.bin").write_bytes(payload)
        (ROOT / f"v{version}.0-le-{label}.squashfs").write_bytes(Writer(version, 0, False, False, False, block_size=size).image(payload))
    if args.sevenzip:
        (ROOT / "known-length.block").write_bytes(without_marker(TAIL, args.sevenzip))
        (ROOT / "v2.0-le-lzma-known.squashfs").write_bytes(Writer(2, 0, False, False, False, codec="known", sevenzip=args.sevenzip).image(PAYLOAD))
    for path in sorted(ROOT.iterdir()):
        if path.suffix in (".squashfs", ".bin", ".block"):
            print(path.name, len(path.read_bytes()), hashlib.sha256(path.read_bytes()).hexdigest())
