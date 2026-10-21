"""Optical source bytes and filesystem bytes through the public CLI commands.

Fixtures are invented ISO 9660 / CD sectors built independently of the Rust
test helpers. No real evidence or external disc tools are required.
"""
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile


def iso_fixture():
    content = bytes((number * 17 + number // 101) % 256 for number in range(7001))
    image = bytearray(40 * 2048)

    def both32(target, offset, value):
        target[offset:offset + 8] = struct.pack("<I", value) + struct.pack(">I", value)

    def both16(target, offset, value):
        target[offset:offset + 4] = struct.pack("<H", value) + struct.pack(">H", value)

    def record(name, extent, length, directory):
        entry = bytearray(33 + len(name) + (len(name) % 2 == 0))
        entry[0] = len(entry)
        both32(entry, 2, extent)
        both32(entry, 10, length)
        entry[18:25] = bytes((126, 10, 21, 1, 2, 3, 0))
        entry[25] = 2 if directory else 0
        both16(entry, 28, 1)
        entry[32] = len(name)
        entry[33:33 + len(name)] = name
        return entry

    descriptor = bytearray(2048)
    descriptor[:7] = b"\x01CD001\x01"
    both32(descriptor, 80, 40)
    for offset, value in ((120, 1), (124, 1), (128, 2048)):
        both16(descriptor, offset, value)
    descriptor[156:190] = record(b"\x00", 20, 2048, True)
    descriptor[881] = 1
    image[16 * 2048:17 * 2048] = descriptor
    image[17 * 2048:17 * 2048 + 7] = b"\xffCD001\x01"
    entries = (record(b"\x00", 20, 2048, True) + record(b"\x01", 20, 2048, True)
               + record(b"PAYLOAD.BIN;1", 24, len(content), False))
    image[20 * 2048:20 * 2048 + len(entries)] = entries
    image[24 * 2048:24 * 2048 + len(content)] = content
    return bytes(image), content


def test_optical_discs(executable, base_environment):
    with tempfile.TemporaryDirectory(prefix="lovelycarve-optical-") as directory:
        root = Path(directory)
        iso, content = iso_fixture()
        audio = bytes([0x73]) * 2352 * 2
        (root / "音轨 one.bin").write_bytes(audio)
        for mode, width in ((1, 2352), (2, 2352), (2, 2336), (1, 2048)):
            data = bytearray()
            for number in range(40):
                sector = bytearray([0xA7]) * width
                if width == 2352:
                    absolute = number + 150
                    values = (absolute // 4500, absolute // 75 % 60, absolute % 75)
                    sector[:12] = b"\x00" + bytes([0xFF]) * 10 + b"\x00"
                    sector[12:15] = bytes(value // 10 * 16 + value % 10 for value in values)
                    sector[15] = mode
                start = 16 if mode == 1 and width == 2352 else 0
                if mode == 2:
                    subheader = 16 if width == 2352 else 0
                    sector[subheader:subheader + 8] = bytes((0, 0, 8, 0)) * 2
                    start = subheader + 8
                sector[start:start + 2048] = iso[number * 2048:(number + 1) * 2048]
                data.extend(sector)
            (root / "data.bin").write_bytes(data)
            cue = root / "disc.cue"
            cue.write_text('FILE "音轨 one.bin" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n'
                           f'FILE data.bin BINARY\nTRACK 02 MODE{mode}/{width}\nINDEX 01 00:00:00\n',
                           encoding="utf-8")
            raw_export = root / f"source-{mode}-{width}.raw"
            file_export = root / f"file-{mode}-{width}.bin"
            node = "cdfs:part:1:" + b"/PAYLOAD.BIN".hex()
            requests = [
                {"id": "open", "command": "images open", "args": {"path": str(cue)}},
                {"id": "list", "command": "files list", "args": {"parentId": "part:1"}},
                {"id": "read", "command": "files read", "args": {"nodeId": node, "offset": 2043, "length": 29}},
                {"id": "file-hash", "command": "files hash", "args": {"nodeId": node, "algos": ["sha256"]}},
                {"id": "source-hash", "command": "images hash", "args": {"algorithms": ["sha256"], "taskId": "optical-hash"}},
                {"id": "file-export", "command": "files export", "args": {"nodeId": node, "destination": str(file_export)}},
                {"id": "source-export", "command": "images export", "args": {"destination": str(raw_export), "taskId": "optical-export"}},
            ]
            result = subprocess.run([str(executable), "--quiet", "session"],
                                    input="\n".join(map(json.dumps, requests)).encode(),
                                    capture_output=True, cwd=root, env=base_environment, timeout=120)
            assert result.returncode == 0, (mode, width, result.stdout, result.stderr)
            rows = {row["id"]: row["result"] for row in map(json.loads, result.stdout.splitlines())}
            assert rows["open"]["opticalDisc"]["tracks"][1]["sectorBytes"] == width
            assert rows["open"]["partitions"][1]["kind"] == "cdfs"
            assert "PAYLOAD.BIN" in json.dumps(rows["list"])
            assert rows["read"] == {"encoding": "hex", "data": content[2043:2072].hex()}
            assert rows["file-hash"]["sha256"] == hashlib.sha256(content).hexdigest()
            assert rows["source-hash"]["sha256"] == hashlib.sha256(audio + data).hexdigest()
            assert file_export.read_bytes() == content
            assert raw_export.read_bytes() == audio + data
    print("CLI optical tests passed: CUE mixed tracks, Mode 1, XA 2336/2352, file/source reads, hashes and exports")
