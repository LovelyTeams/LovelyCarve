"""Readers through the public CLI commands: each fixture is opened, listed,
read by range, hashed and exported, by node ID and by path.

Fixtures are the readers' committed synthetic or reference images; the
optional native images come from the environment variables named below.
"""
from datetime import datetime, timedelta
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import zlib

from cli_optical_tests import iso_fixture
from cli_support import REPOSITORY, node, path_id, request

ALIAS = "ev"
FIRMWARE = REPOSITORY / "src-tauri/test_data/firmware"
VENDOR = REPOSITORY / "src-tauri/vendor"


def add(path, *options):
    return request("open", "evidence", "add", path, "--as", ALIAS, *options)


def at(node_id):
    return node(ALIAS, node_id)


def ls(identifier, address, *options):
    return request(identifier, "ls", address, *options)


def cat(identifier, address, offset=0, length=None, stream=None):
    words = ["cat", address, "--offset", offset]
    if length is not None:
        words += ["--length", length]
    if stream is not None:
        words += ["--stream", stream]
    return request(identifier, *words)


def digest(identifier, address):
    return request(identifier, "hash", address, "--algo", "sha256")


def export(identifier, address, destination, stream=None):
    return request(identifier, "export", address, "-o", destination, *(["--stream", stream] if stream else []))


def opened(rows):
    return rows["open"]["items"][0]


def sha256(result):
    return result["items"][0]["sha256"]


def names(result):
    return {item["name"] for item in result["items"]}


def hex_of(data):
    return {"encoding": "hex", "data": data.hex()}


def test_vmfs(cli, root):
    fixture_path = os.environ.get("LOVELYCARVE_VMFS_ORACLE")
    if not fixture_path:
        print("VMFS CLI oracle skipped: set LOVELYCARVE_VMFS_ORACLE")
        return
    fixtures = Path(fixture_path)
    case = root / "vmfs-case"
    scope = ("-C", case)
    cli.run("case", "new", case, "--name", "VMFS test")
    cli.run(*scope, "case", "open")
    try:
        for version, descriptor_prefix in [
            (5, "c800000040000000000001000008000000009101183601000700000000000000"),
            (6, "0001000008000000000001000020000000000101004000000800000000000000"),
        ]:
            output = root / f"vmfs{version}-small.txt"
            rows = cli.results([
                add(fixtures / f"vmfs{version}.bin"),
                ls("directory", "/ev/part:0/directory"),
                cat("read", at(path_id("vmfs:part:0:", "/directory/file100"))),
                cat("large", "/ev/part:0/large-flat.vmdk", 117 << 30, 512),
                cat("hole", "/ev/part:0/large-flat.vmdk", 0, 513),
                request("stat", "stat", "/ev/part:0/large-flat.vmdk"),
                digest("hash", "/ev/part:0/small"),
                export("export", "/ev/part:0/small", output),
            ])
            assert opened(rows)["partitions"][0]["kind"] == "vmfs", rows["open"]
            assert names(rows["directory"]) == {f"file{index}" for index in range(1, 101)}
            assert rows["read"] == hex_of(b"File 100\n")
            assert rows["large"] == hex_of(b"\x42" * 512)
            assert rows["hole"] == hex_of(bytes(513))
            assert rows["stat"]["size"] == 42 << 40, rows["stat"]
            assert sha256(rows["hash"]) == hashlib.sha256(b"tiny\n").hexdigest()
            assert output.read_bytes() == b"tiny\n"

            for suffix in ["d", "l", "ld"]:
                view = cli.results([
                    add(fixtures / f"vmfs{version}{suffix}.bin"),
                    ls("root", "/ev/part:0"),
                ])
                assert opened(view)["partitions"][0]["kind"] == "vmfs"
                assert {".fdc.sf", ".pbc.sf", ".sbc.sf"} <= names(view["root"])

            # Load the members out of extent order. An incomplete datastore is
            # recognized but exposes no synthetic filesystem until complete.
            member_paths = [fixtures / "span" / f"vmfs{version}-span-{index}.bin" for index in [1, 0]]
            aliases = [f"v{version}second", f"v{version}first"]
            cli.run(*scope, "evidence", "add", member_paths[0], "--as", aliases[0])
            lone = cli.json(*scope, "raid", "detect", "--member", aliases[0])
            assert lone["assembled"] == [] and any("VMFS" in message for message in lone["diagnostics"]), lone
            cli.run(*scope, "evidence", "add", member_paths[1], "--as", aliases[1])
            stack = cli.json(*scope, "raid", "detect", "--member", aliases[0], "--member", aliases[1])
            assert len(stack["assembled"]) == 1 and stack["arrays"][0]["raidDisks"] == 2, stack
            path = stack["assembled"][0]["path"]
            for partition in [0, 1]:
                assert cli.run(*scope, "cat", f"{path}/part:{partition}/.fdc.sf", "--length", "32", binary=True).stdout.hex() == descriptor_prefix
    finally:
        cli.run(*scope, "case", "close")
    print("VMFS CLI oracle passed: native paths, sparse reads, hash, export and two-image assembly")


def test_hpfs(cli, root):
    fixture = VENDOR / "hpfs-reader/tests/fixtures/hpfsutils.zlib"
    volume = zlib.decompress(fixture.read_bytes())
    bare = root / "hpfs.bin"
    bare.write_bytes(volume)
    disk = root / "hpfs-mbr.img"
    base = 2048
    table = bytearray(512)
    table[450] = 7
    struct.pack_into("<II", table, 454, base, len(volume) // 512)
    table[510:] = b"\x55\xaa"
    with disk.open("xb") as output:
        output.write(table)
        output.seek(base * 512)
        output.write(volume)
    payload = bytes(range(256)) * 4129 + b"tail"
    for index, source in enumerate([bare, disk]):
        exported = root / f"hpfs-{index}-payload.bin"
        rows = cli.results([
            add(source),
            ls("tree", "/ev/part:0/TREE", "--limit", 1000),
            cat("read", at(path_id("hpfs:part:0:", "/Read Me.txt"))),
            cat("casefold", "/ev/part:0/TREE/item 0512.txt"),
            cat("range", "/ev/part:0/payload.bin", 509, 1027),
            request("stat", "stat", "/ev/part:0/Read Me.txt"),
            digest("hash", "/ev/part:0/payload.bin"),
            export("export", "/ev/part:0/payload.bin", exported),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "hpfs"
        assert names(rows["tree"]) == {f"Item {n:04}.txt" for n in range(513)}
        assert rows["read"] == hex_of(b"independent HPFS fixture\n")
        assert rows["casefold"] == hex_of(b"0512 HPFS entry\n")
        assert rows["range"] == hex_of(payload[509:1536])
        assert not rows["stat"]["timestamps"]["created"].endswith("Z")
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert exported.read_bytes() == payload
    print("HPFS CLI tests passed: bare and MBR volumes, split directories, ranges, times, hash and export")


def test_vxfs(cli, root):
    fixtures = VENDOR / "vxfs-reader/tests/fixtures"
    for name, block_size in [("le2", 1024), ("le3", 2048), ("be4", 8192)]:
        volume = zlib.decompress((fixtures / f"{name}.zlib").read_bytes())
        source = root / f"vxfs-{name}.img"
        if name == "be4":
            table = bytearray(512)
            table[450] = 0x83
            struct.pack_into("<II", table, 454, 8, len(volume) // 512)
            table[510:] = b"\x55\xaa"
            source.write_bytes(table + bytes(7 * 512) + volume)
        else:
            source.write_bytes(volume)
        exported = root / f"{name}-linear.bin"
        expected = b"D" * block_size + bytes(block_size) + b"I" * (block_size - 19)
        rows = cli.results([
            add(source),
            ls("root", "/ev/part:0"),
            ls("dir", "/ev/part:0/dir"),
            cat("tiny", at(path_id("vxfs:part:0:", "/tiny"))),
            cat("link", "/ev/part:0/link"),
            cat("far", "/ev/part:0/dir/large", (5 << 30) + block_size - 2, 10),
            cat("linear", "/ev/part:0/linear", 2 * block_size - 3, 10),
            request("stat", "stat", "/ev/part:0/tiny"),
            request("alias", "stat", "/ev/part:0/dir/alias"),
            digest("hash", "/ev/part:0/linear"),
            export("export", "/ev/part:0/linear", exported),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "vxfs"
        assert names(rows["root"]) == {"tiny", "dir", "link", "linear", "empty"}
        assert names(rows["dir"]) == {"alias", "large"}
        assert rows["tiny"] == rows["link"] == hex_of(b"tiny\n")
        assert rows["far"] == hex_of(bytes(2) + bytes(range(8)))
        assert rows["linear"] == hex_of(bytes(3) + b"I" * 7)
        assert rows["stat"]["inode"] == rows["alias"]["inode"] == 3
        changed = datetime.fromisoformat(rows["stat"]["timestamps"]["mftChanged"].replace("Z", "+00:00"))
        assert changed.microsecond == 9876 and changed.utcoffset() == timedelta(0), changed
        assert sha256(rows["hash"]) == hashlib.sha256(expected).hexdigest()
        assert exported.read_bytes() == expected
    print("VxFS CLI tests passed: layouts 2–4, MBR, hard links, ranges beyond 5 GiB, hash and export")


def test_aix_jfs(cli, root):
    fixtures = VENDOR / "aix-jfs-reader/tests/fixtures"
    for name in ["fsv3", "fsv3p-512", "fsv3p-1024"]:
        volume = bytearray(zlib.decompress((fixtures / f"{name}.zlib").read_bytes()))
        image = root / f"aix-{name}.img"
        if name == "fsv3p-512":
            # Force backup-superblock recovery inside a nonzero partition.
            volume[4096 + 20:4096 + 22] = b"\0\0"
            mbr = bytearray(512)
            mbr[450] = 0x83
            struct.pack_into("<II", mbr, 454, 8, len(volume) // 512)
            mbr[510:] = b"\x55\xaa"
            image.write_bytes(mbr + bytes(7 * 512) + volume)
        else:
            image.write_bytes(volume)
        exported = root / f"aix-{name}-single.bin"
        payload = b"A" * 4096 + bytes(7 * 4096) + b"Z" * 4096
        rows = cli.results([
            add(image),
            ls("root", "/ev/part:0"),
            ls("dir", "/ev/part:0/dir"),
            cat("tiny", at(path_id("aixjfs:part:0:", "/tiny"))),
            cat("link", "/ev/part:0/link"),
            cat("group", "/ev/part:0/dir/follow"),
            cat("far", "/ev/part:0/double", 1025 * 4096 - 2, 10),
            request("stat", "stat", "/ev/part:0/dir/alias"),
            digest("hash", "/ev/part:0/single"),
            export("export", "/ev/part:0/single", exported),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "aixjfs"
        assert names(rows["root"]) == {"tiny", "dir", "single", "double", "link", "longlink"}
        assert names(rows["dir"]) == {"alias", "follow"}
        assert rows["tiny"] == rows["link"] == hex_of(b"aixjfs\n")
        assert rows["group"] == hex_of(b"allocation group")
        assert rows["far"] == hex_of(bytes(2) + bytes(range(8)))
        assert rows["stat"]["inode"] == 5
        stamp = datetime.fromisoformat(rows["stat"]["timestamps"]["modified"].replace("Z", "+00:00"))
        assert stamp.utcoffset() == timedelta(0) and stamp.microsecond == 85
        assert stamp.replace(microsecond=0).timestamp() == -2
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert exported.read_bytes() == payload
    print("AIX JFS1 CLI tests passed: fsv3 and fsv3p, backup superblock, fragments, indirection and export")


def test_unixware_bfs(cli, root):
    fixtures = VENDOR / "unixware-bfs-reader/tests/fixtures"
    empty = root / "bfs-empty.img"
    empty.write_bytes(zlib.decompress((fixtures / "util-linux-empty.zlib").read_bytes()))
    rows = cli.results([add(empty), ls("root", "/ev/part:0")])
    assert opened(rows)["partitions"][0]["kind"] == "unixwarebfs"
    assert names(rows["root"]) == set()

    volume = zlib.decompress((fixtures / "populated.zlib").read_bytes())
    bare = root / "bfs-bare.img"
    bare.write_bytes(volume)
    wrapped = root / "bfs-vtoc.img"
    sector = 512
    owner_start = 64
    stand_start = owner_start + 64
    volume_sectors = len(volume) // sector
    swap_start = stand_start + volume_sectors
    foreign_start = swap_start + 64
    owner_length = foreign_start - owner_start
    disk = bytearray((foreign_start + 64) * sector)
    disk[450] = 0x63
    disk[466] = 0x82
    struct.pack_into("<II", disk, 454, owner_start, owner_length)
    struct.pack_into("<II", disk, 470, foreign_start, 64)
    disk[510:512] = b"\x55\xaa"
    label = (owner_start + 29) * sector
    struct.pack_into("<I", disk, label + 4, 0xCA5E600D)
    struct.pack_into("<II", disk, label + 36, sector, owner_start)
    struct.pack_into("<I", disk, label + 156, 0x600DDEEE)
    struct.pack_into("<H", disk, label + 172, 16)
    slices = label + 216
    struct.pack_into("<HHII", disk, slices, 5, 0x200, owner_start, owner_length)
    struct.pack_into("<HHII", disk, slices + 7 * 12, 9, 0x201, stand_start, volume_sectors)
    struct.pack_into("<HHII", disk, slices + 8 * 12, 3, 0x200, swap_start, 32)
    struct.pack_into("<HHII", disk, slices + 9 * 12, 4, 0x200, foreign_start - 1, 2)
    struct.pack_into("<HHII", disk, slices + 10 * 12, 4, 0, stand_start + 1, 1)
    disk[stand_start * sector:swap_start * sector] = volume
    wrapped.write_bytes(disk)

    payload = bytes(range(256)) * 5 + b"unixware\n"
    for source in (bare, wrapped):
        output = root / f"{source.stem}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", "/ev/part:0"),
            cat("alias", at(path_id("unixwarebfs:part:0:", "/unix.old"))),
            cat("long", "/ev/part:0/1234567890.dat"),
            cat("range", "/ev/part:0/unix", 509, 522),
            cat("tail", "/ev/part:0/unix", len(payload) - 4, 50),
            cat("empty", "/ev/part:0/empty"),
            request("stat", "stat", "/ev/part:0/unix"),
            request("alias_stat", "stat", "/ev/part:0/unix.old"),
            digest("hash", "/ev/part:0/unix"),
            export("export", "/ev/part:0/unix", output),
        ])
        parts = {p["id"]: p for p in opened(rows)["partitions"]}
        assert parts[0]["kind"] == "unixwarebfs"
        if source == wrapped:
            assert set(parts) == {0, 1, 2}
            assert parts[0]["startLba"] == stand_start and parts[0]["lengthLba"] == volume_sectors
            assert parts[0]["label"] == "/stand (UnixWare slice 7)"
            assert parts[1]["kind"] == "swap" and parts[1]["startLba"] == foreign_start
            assert parts[2]["kind"] == "swap" and parts[2]["startLba"] == swap_start
        assert names(rows["root"]) == {"unix", "unix.old", "empty", "1234567890.dat"}
        assert rows["alias"] == hex_of(payload)
        assert rows["long"] == hex_of(b"UnixWare BFS fixture\n")
        assert rows["range"] == hex_of(payload[509:1031])
        assert rows["tail"] == hex_of(payload[-4:])
        assert rows["empty"] == hex_of(b"")
        assert rows["stat"]["inode"] == rows["alias_stat"]["inode"] == 3
        modified = datetime.fromisoformat(rows["stat"]["timestamps"]["modified"].replace("Z", "+00:00"))
        assert modified.utcoffset() == timedelta(0) and modified.timestamp() == 1706010001
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload

    # A conflicting, otherwise valid slice must not replace the original MBR span.
    overlapping = root / "bfs-vtoc-overlap.img"
    struct.pack_into("<HHII", disk, slices + 10 * 12, 4, 0x200, stand_start + 1, 1)
    overlapping.write_bytes(disk)
    parts = {p["id"]: p for p in opened(cli.results([add(overlapping)]))["partitions"]}
    assert set(parts) == {0, 1}
    assert parts[0]["startLba"] == owner_start and parts[0]["lengthLba"] == owner_length
    assert parts[1]["startLba"] == foreign_start
    print("UnixWare BFS CLI tests passed: native empty volume, VTOC, hard links, ranges, hash and export")


def test_xdvdfs(cli, root):
    fixture = VENDOR / "xdvdfs-reader/tests/fixtures/extract-xiso.zlib"
    volume = zlib.decompress(fixture.read_bytes())
    video, video_payload = iso_fixture(b"ISO 9660 video partition data\n")
    payload = bytes(range(256)) * 19 + b"ending"
    for name, base in [("xiso", 0), ("xgd1", 0x18300000), ("xgd2", 0x0FD90000), ("xgd3", 0x02080000)]:
        source = root / f"{name}.iso"
        has_video = name == "xgd3"
        with source.open("xb") as disk:
            if has_video:
                disk.write(video)
            disk.seek(base)
            disk.write(volume)
        game = int(has_video)
        path = f"/ev/part:{game}"
        output = root / f"{name}-frame.bin"
        commands = [
            add(source),
            ls("root", path),
            ls("textures", f"{path}/textures", "--limit", 500),
            cat("range", f"{path}/TEXTURES/frame.bin", 2043, 29),
            cat("nested", at(path_id(f"xdvdfs:part:{game}:", "/Textures/Nested/Text.txt"))),
            cat("empty", f"{path}/empty"),
            request("stat", "stat", f"{path}/textures/Frame.bin"),
            digest("hash", f"{path}/Textures/Frame.bin"),
            export("export", f"{path}/Textures/Frame.bin", output),
        ]
        if has_video:
            commands += [cat("video", "/ev/part:0/PAYLOAD.BIN")]
        rows = cli.results(commands)
        parts = {p["id"]: p for p in opened(rows)["partitions"]}
        assert parts[game]["kind"] == "xdvdfs" and parts[game]["startLba"] * opened(rows)["sectorSize"] == base
        assert names(rows["root"]) == {"Textures", "EMPTY", "readme.txt"}
        assert names(rows["textures"]) == {"Frame.bin", "Nested"} | {f"Data-{index:03}.dat" for index in range(120)}
        assert rows["range"] == hex_of(payload[2043:2072])
        assert rows["nested"] == hex_of(b"Independent Xbox DVD image.\n")
        assert rows["empty"] == hex_of(b"") and rows["stat"]["size"] == len(payload)
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload
        if has_video:
            assert parts[0]["kind"] == "cdfs" and rows["video"] == hex_of(video_payload)
        source.unlink()
    print("XDVDFS CLI tests passed: native XISO, XGD1/2/3 bases, concurrent ISO 9660 view, ranges, hash and export")


def test_fatx(cli, root):
    fixtures = VENDOR / "fatx-reader/tests/fixtures"
    payload = bytes((n * 17 + n // 117) % 256 for n in range(13013))
    sources = []
    for name in ("xbox-fat16", "xbox-fat32", "xtaf-fat16-compact", "xtaf-fat32-compact"):
        encoded = (fixtures / f"{name}.zlib").read_bytes()
        length = struct.unpack_from("<I", encoded)[0]
        prefix = zlib.decompress(encoded[4:])
        path = root / f"{name}.img"
        with path.open("xb") as image:
            image.write(prefix)
            image.truncate(length)
        sources.append((path, 0, 0, length, None))
        if name == "xbox-fat16":
            # Two inactive and invalid table records precede the valid slot.
            offset = 0x100000
            disk = root / "xbox-lba48.img"
            table = bytearray(512)
            table[:16] = b"****PARTINFO****"
            struct.pack_into("<16sIII", table, 48, b"INACTIVE", 0, offset // 512, length // 512)
            struct.pack_into("<16sIII", table, 80, b"OUTSIDE", 0x80000000, 0xFFFFFF00, 0x800)
            struct.pack_into("<16sIII", table, 112, b"XBOX DATA", 0x80000000, offset // 512, length // 512)
            with disk.open("xb") as image:
                image.write(table)
                image.seek(offset)
                image.write(prefix)
                image.truncate(offset + length)
            sources.append((disk, 2, offset, length, "XBOX DATA"))
        if name == "xtaf-fat16-compact":
            offset = 0x140000
            disk = root / "xbox360-devkit.img"
            table = bytearray(512)
            struct.pack_into(">I", table, 0, 0x00020000)
            struct.pack_into(">II", table, 16, offset // 512, length // 512)
            with disk.open("xb") as image:
                image.write(table)
                image.seek(offset)
                image.write(prefix)
                image.truncate(offset + length)
            sources.append((disk, 1, offset, length, "Xbox 360 Devkit Dashboard"))
            offset = 0x7FF000
            disk = root / "xbox360-memory.img"
            with disk.open("xb") as image:
                image.seek(offset)
                image.write(prefix)
                image.truncate(offset + length)
            sources.append((disk, 0, offset, length, "Xbox 360 Memory Unit"))

    for source, partition, base, length, label in sources:
        address = f"/ev/part:{partition}"
        exported = root / f"{source.stem}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", address),
            ls("dir", f"{address}/cache"),
            cat("range", f"{address}/CACHE/data.bin", 2043, 4097),
            cat("empty", at(path_id(f"fatx:part:{partition}:", "/Empty"))),
            request("stat", "stat", f"{address}/Cache/Data.bin"),
            digest("hash", f"{address}/Cache/Data.bin"),
            export("export", f"{address}/Cache/Data.bin", exported),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["id"] == partition and part["kind"] == "fatx"
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        if label is not None:
            assert part["label"] == label and info["tableScheme"] == "xbox"
        assert names(rows["root"]) == {"Cache", "Empty"} and names(rows["dir"]) == {"Data.bin"}
        assert rows["range"] == hex_of(payload[2043:6140]) and rows["empty"] == hex_of(b"")
        assert rows["stat"]["size"] == len(payload)
        assert datetime.fromisoformat(rows["stat"]["timestamps"]["created"]).tzinfo is None
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert exported.read_bytes() == payload
    print("FATX CLI tests passed: both FAT widths and byte orders, LBA48, Devkit and memory unit, ranges, hash and export")


def test_mfs(cli, root):
    fixtures = VENDOR / "mfs-reader/tests/fixtures"
    data = bytes((n * 13 + n // 47) % 256 for n in range(8199))
    resource = bytes((n * 7 + n // 81) % 256 for n in range(4673))
    stream = "com.apple.ResourceFork"
    sources = []
    for name in ("raw-400", "raw-800", "dc42-400", "dc42-800"):
        volume = zlib.decompress((fixtures / f"{name}.zlib").read_bytes())
        source = root / f"mfs-{name}.img"
        source.write_bytes(volume)
        sources.append((source, 0, len(volume)))
        if name == "raw-400":
            base = 8 * 512
            apm = bytearray(base)
            struct.pack_into(">2sH", apm, 0, b"ER", 512)
            struct.pack_into(">2sHIII32s32s", apm, 512, b"PM", 0, 1, 8, len(volume) // 512, b"MFS", b"Apple_MFS")
            source = root / "mfs-apm.img"
            source.write_bytes(apm + volume)
            sources.append((source, base, len(volume)))

    address = "/ev/part:0"
    entry = at(path_id("mfs:part:0:", "/Résumé"))
    resource_only = f"{address}/Only Resource"
    for source, base, length in sources:
        data_output = root / f"{source.stem}-data.bin"
        resource_output = root / f"{source.stem}-resource.bin"
        rows = cli.results([
            add(source),
            ls("root", address),
            cat("range", f"{address}/RÉSUMÉ", 1001, 3101),
            cat("empty-data", resource_only),
            cat("only-resource", resource_only, length=93, stream=stream),
            request("stat", "stat", entry),
            request("streams", "streams", entry),
            cat("resource", entry, length=2341, stream=stream),
            digest("hash", entry),
            export("data-export", entry, data_output),
            export("resource-export", entry, resource_output, stream=stream),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["id"] == 0 and part["kind"] == "mfs", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        if base:
            assert info["tableScheme"] == "apm" and part["label"] == "MFS (Apple_MFS)"
        assert names(rows["root"]) == {"Résumé", "Only Resource", "Empty"}
        assert rows["range"] == hex_of(data[1001:4102]) and rows["empty-data"] == hex_of(b"")
        assert rows["stat"]["size"] == len(data)
        assert datetime.fromisoformat(rows["stat"]["timestamps"]["created"]).tzinfo is None
        assert {item["name"]: item["size"] for item in rows["streams"]["items"]} == {"": len(data), stream: len(resource)}
        assert rows["resource"] == hex_of(resource[:2341]) and rows["only-resource"] == hex_of(resource[:93])
        assert sha256(rows["hash"]) == hashlib.sha256(data).hexdigest()
        assert data_output.read_bytes() == data and resource_output.read_bytes() == resource

    apple_image = os.environ.get("LOVELYCARVE_MFS_APPLE_ORACLE")
    if apple_image:
        entry = f"{address}/TN.002.Compatibility"
        output = root / "apple-mfs-resource.bin"
        rows = cli.results([
            add(Path(apple_image)),
            ls("root", address),
            digest("hash", entry),
            export("resource-export", entry, output, stream=stream),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "mfs"
        assert names(rows["root"]) == {
            "DeskTop", "TN.002.Compatibility", "TN.002.Compatibility.pdf", "CSillyBalls", "CSillyBalls.make",
            "PSillyBalls", "PSillyBalls.make", "SCN.003.SillyBalls", "SillyBalls.c", "SillyBalls.p",
        }
        assert sha256(rows["hash"]) == "bc23503a695764123759f006dfc4fa0052d96ec293c35168090aedbc9e497a8a"
        assert hashlib.sha256(output.read_bytes()).hexdigest() == "65d4a524d31c5c9c76579ef3656d592266bd11bd073c255c331029c20ea55cf5"
        print("MFS Apple oracle passed: original Disk Copy image and both fork hashes")
    print("MFS CLI tests passed: 400K and 800K raw and Disk Copy images, APM, local timestamps and both forks")


def test_littlefs(cli, root):
    fixtures = VENDOR / "littlefs-reader/tests/fixtures"
    payload = bytes((n * 21 + n // 49) % 256 for n in range(19473))
    sources = []
    for name in ("128-2.0", "512-2.1", "4096-2.1"):
        volume = zlib.decompress((fixtures / f"{name}.zlib").read_bytes())
        source = root / f"littlefs-{name}.img"
        source.write_bytes(volume)
        sources.append((source, 0, len(volume)))
        if name == "512-2.1":
            base = 8 * 512
            mbr = bytearray(base)
            mbr[446 + 4] = 0x83
            struct.pack_into("<II", mbr, 446 + 8, 8, len(volume) // 512)
            mbr[510:512] = b"\x55\xaa"
            partitioned = root / "littlefs-mbr.img"
            partitioned.write_bytes(mbr + volume)
            sources.append((partitioned, base, len(volume)))
    for source, base, length in sources:
        entry = at(path_id("littlefs:part:0:", "/Logs/event.bin"))
        output = root / f"{source.stem}.bin"
        rows = cli.results([
            add(source),
            ls("root", "/ev/part:0"),
            ls("split", "/ev/part:0/Split"),
            cat("inline", "/ev/part:0/tiny"),
            cat("byte-name", "/ev/part:0/Logs/odd-%FF"),
            cat("range", entry, 4291, 6987),
            cat("empty", "/ev/part:0/Logs/Nested/EmptyLater"),
            digest("hash", entry),
            export("export", entry, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "littlefs", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        if base:
            assert info["tableScheme"] == "mbr"
        assert names(rows["root"]) == {"Logs", "Split", "tiny"}
        assert names(rows["split"]) == {f"entry-{n:02}-long-file-name-for-split" for n in range(20)}
        assert rows["inline"] == hex_of(b"init-end") and rows["byte-name"] == hex_of(b"name")
        assert rows["range"] == hex_of(payload[4291:11278]) and rows["empty"] == hex_of(b"")
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload
    print("LittleFS CLI tests passed: v2.0 and v2.1, bare and MBR volumes, split directories, ranges, hash and export")


def test_spiffs(cli, root):
    fixtures = VENDOR / "spiffs-reader/tests/fixtures"
    payload = bytes((n * 23 + n // 61) % 256 for n in range(49281))
    sources = []
    for name in ("esp32-default", "esp32-no-magic", "be-aligned", "constant", "esp32-native"):
        volume = zlib.decompress((fixtures / f"{name}.zlib").read_bytes())
        source = root / f"spiffs-{name}.img"
        source.write_bytes(volume)
        sources.append((source, 0, len(volume)))
        if name == "esp32-default":
            base = 8 * 512
            mbr = bytearray(base)
            mbr[446 + 4] = 0x83
            struct.pack_into("<II", mbr, 446 + 8, 8, len(volume) // 512)
            mbr[510:512] = b"\x55\xaa"
            partitioned = root / "spiffs-mbr.img"
            partitioned.write_bytes(mbr + volume)
            sources.append((partitioned, base, len(volume)))

    for source, base, length in sources:
        name = "%2Flogs%2Fevent.bin"
        entry = at(path_id("spiffs:part:0:", f"/{name}"))
        output = root / f"{source.stem}.bin"
        rows = cli.results([
            add(source),
            ls("root", "/ev/part:0"),
            cat("read", f"/ev/part:0/{name}", 24991, 14087),
            cat("empty", "/ev/part:0/%2Fempty"),
            request("stat", "stat", entry),
            digest("hash", entry),
            export("export", entry, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "spiffs", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        if base:
            assert info["tableScheme"] == "mbr"
        assert names(rows["root"]) == {"%2Fempty", "%2Fhello.txt", name}
        assert rows["read"] == hex_of(payload[24991:39078]) and rows["empty"] == hex_of(b"")
        assert rows["stat"]["size"] == len(payload)
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload
    print("SPIFFS CLI tests passed: signed and unsigned layouts, big endian, MBR, ranges, hash and export")


def materialize_sparse_image(packed, source, base, partition_type):
    archive = zlib.decompress(packed.read_bytes())
    length, = struct.unpack_from("<Q", archive)
    source.touch(exist_ok=False)
    if os.name == "nt":
        subprocess.run(["fsutil", "sparse", "setflag", source], capture_output=True, check=False)
    with source.open("r+b") as image:
        if base:
            mbr = bytearray(512)
            mbr[450] = partition_type
            struct.pack_into("<II", mbr, 454, base // 512, length // 512)
            mbr[510:512] = b"\x55\xaa"
            image.write(mbr)
        offset = 8
        while offset < len(archive):
            extent_start, count = struct.unpack_from("<QI", archive, offset)
            offset += 12
            assert offset + count <= len(archive) and extent_start + count <= length
            image.seek(base + extent_start)
            image.write(archive[offset:offset + count])
            offset += count
        image.truncate(base + length)
    return length


def test_hammer(cli, root):
    fixtures = VENDOR / "hammer-reader/tests/fixtures"
    live = bytes((i * 17 + i // 47) % 256 for i in range(16384))
    old = bytes((i * 49 + i // 71) % 256 for i in range(16384))
    tail = bytes(32768) + b"HAMMER sparse tail"
    address = "/ev/part:0"
    snapshot = f"{address}/@snapshots/0000000100000001"
    for name, base in (("v7-empty", 0), ("v1-content", 0), ("v7-content", 4096)):
        source = root / f"hammer-{name}.img"
        length = materialize_sparse_image(fixtures / f"hammer1-{name}.h1z", source, base, 0xA5)
        entry = at(path_id("hammer:part:0:", "/payload.bin"))
        output = root / f"hammer-{name}-export.bin"
        requests = [add(source), ls("root", address)]
        expected_names = set()
        if name.endswith("content"):
            expected_names = {"payload.bin", "same-inode", "latest"}
            requests.extend([
                request("file", "stat", entry),
                request("hardlink", "stat", f"{address}/same-inode"),
                request("link-meta", "stat", f"{address}/latest"),
                cat("link", f"{address}/latest", 16000, 1500),
                cat("tail", entry, 49145, 40),
                digest("hash", entry),
                export("export", entry, output),
            ])
        if base:
            expected_names.add("@snapshots")
            requests.extend([
                ls("snapshots", f"{address}/@snapshots"),
                ls("history", snapshot),
                cat("retired", f"{snapshot}/retired", 16000, 1500),
                cat("history-link", f"{snapshot}/latest", 16000, 1500),
                digest("history-hash", f"{snapshot}/payload.bin"),
            ])
        rows = cli.results(requests)
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "hammer", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        assert names(rows["root"]) == expected_names
        if name.endswith("content"):
            assert rows["file"]["inode"] == rows["hardlink"]["inode"] == 2
            assert rows["link-meta"]["linkTarget"] == "payload.bin"
            assert rows["link"] == hex_of((live + tail)[16000:17500])
            assert rows["tail"] == hex_of((live + tail)[49145:])
            assert sha256(rows["hash"]) == "4e04f28fd9244bb9f8d53e63a825952d3796ebc9cc5bd94fd572215e7be74d59"
            assert output.read_bytes() == live + tail
        if base:
            assert info["tableScheme"] == "mbr"
            assert names(rows["snapshots"]) == {"0000000100000001"}
            assert names(rows["history"]) == expected_names - {"@snapshots"} | {"retired"}
            assert rows["retired"] == rows["history-link"] == hex_of((old + tail)[16000:17500])
            assert sha256(rows["history-hash"]) == hashlib.sha256(old + tail).hexdigest()
        source.unlink()
    print("HAMMER CLI tests passed: v1/v7, MBR, snapshot history, links, sparse ranges, hash and export")


def test_hammer2(cli, root):
    fixtures = VENDOR / "hammer2-reader/tests/fixtures"
    payload = bytes((n * 17 + n // 51) % 256 for n in range(196613))
    address = "/ev/part:0"
    for name in ("lz4", "zlib", "mirror", "v1", "snapshot", "mbr"):
        base = 4096 if name == "mbr" else 0
        source = root / f"hammer2-{name}.img"
        packed = fixtures / f"{'lz4' if name == 'mbr' else name}.h2z"
        length = materialize_sparse_image(packed, source, base, 0xA5)

        entry = at(path_id("hammer2:part:0:", "/DATA/var/log/event.bin"))
        output = root / f"hammer2-{name}-export.bin"
        requests = [
            add(source),
            ls("pfs", address),
            cat("data", entry, 65381, 12923),
            cat("link", f"{address}/DATA/latest", 65381, 12923),
            cat("sparse", f"{address}/DATA/sparse.bin", 5 * 65536 - 31, 150),
            request("hi", "stat", f"{address}/DATA/hello"),
            request("hardlink", "stat", f"{address}/DATA/same-inode"),
            digest("hash", entry),
            export("export", entry, output),
        ]
        if name == "snapshot":
            requests.extend([
                cat("absolute", f"{address}/ARCHIVE/absolute-link", 65381, 12923),
                request("absolute-stat", "stat", f"{address}/ARCHIVE/absolute-link"),
            ])
        rows = cli.results(requests)
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "hammer2", info
        assert part["startLba"] * info["sectorSize"] == base and part["lengthLba"] * info["sectorSize"] == length
        expected_pfs = {"DATA", "LOCAL"}
        if name == "snapshot":
            expected_pfs.add("ARCHIVE")
            assert rows["absolute"] == hex_of(payload[65381:78304])
            assert rows["absolute-stat"]["linkTarget"] == "/var/log/event.bin"
        assert names(rows["pfs"]) == expected_pfs
        assert rows["data"] == rows["link"] == hex_of(payload[65381:78304])
        assert rows["sparse"] == hex_of(bytes(124) + b"hole-tail")
        assert rows["hi"]["inode"] > 0 and rows["hi"]["inode"] == rows["hardlink"]["inode"], (name, rows["hi"], rows["hardlink"])
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload
        source.unlink()
    print("HAMMER2 CLI tests passed: v1/v2, current mirror, MBR, PFS snapshots, links, sparse ranges and export")


def test_gfs1(cli, root):
    fixtures = VENDOR / "gfs1-reader/tests/fixtures"
    address = "/ev/part:0"
    for block, sparse, base, expected_hash in (
        (512, 42, 0, "11a0adc308355c21cc464bbf3f1ad6860838fc19ca38b3cd164213a69fd20e37"),
        (4096, 520, 4096, "30b1d0589647cd7829b5ed17b747edd101f3973cc5c6932f7f5a9d9e47f2dc9b"),
    ):
        label = f"gfs1-{block}"
        source = root / f"{label}.img"
        length = materialize_sparse_image(fixtures / f"{label}.gfs1z", source, base, 0x83)
        payload = bytes((i * 11 + i // 43) & 255 for i in range(block))
        payload += bytes((sparse - 1) * block + 7) + b"GFS1 sparse tail"
        entry = at(path_id("gfs1:part:0:", "/data/payload.bin"))
        output = root / f"{label}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", address),
            ls("data", f"{address}/data"),
            request("file", "stat", entry),
            request("hardlink", "stat", f"{address}/data/same-inode"),
            request("link", "stat", f"{address}/data/latest"),
            cat("begin", f"{address}/data/latest", block - 11, 73),
            cat("tail", entry, sparse * block - 23, 99),
            digest("hash", entry),
            export("export", entry, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "gfs1", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == length
        if base:
            assert info["tableScheme"] == "mbr"
        assert names(rows["root"]) == {"data"}
        assert names(rows["data"]) == {"empty", "payload.bin", "same-inode", "latest"}
        assert rows["file"]["inode"] == rows["hardlink"]["inode"] > 0
        assert rows["link"]["linkTarget"] == "payload.bin"
        assert rows["begin"] == hex_of(payload[block - 11:block + 62])
        assert rows["tail"] == hex_of(payload[sparse * block - 23:])
        assert sha256(rows["hash"]) == expected_hash
        assert output.read_bytes() == payload
        source.unlink()
    print("GFS1 CLI tests passed: native block sizes, MBR, exhash, hard links, indirect sparse ranges and export")


def test_reiser4(cli, root):
    fixtures = VENDOR / "reiser4-reader/tests/fixtures"
    volume = "/ev/part:0"
    expected_hash = "1ba2c07e284591e221fc293083eba7ea89a5be0000de60ff9792026f1fbda751"
    payload = bytes((n * 31 + n // 211) % 256 for n in range(11113))
    path = "/nested/native-long-name-for-hash.bin"
    node = at(path_id("reiser4:part:0:", path))
    for label, base in (("node40-large", 0), ("node41-short", 4096)):
        source = root / f"reiser4-{label}.img"
        volume_bytes = zlib.decompress((fixtures / f"{label}.reiser4z").read_bytes())
        write_partition_image(source, volume_bytes, base, 0x83)
        output = root / f"reiser4-{label}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", volume),
            ls("nested", f"{volume}/nested"),
            request("file", "stat", node),
            request("tail", "stat", f"{volume}/nested/tail.txt"),
            request("hardlink", "stat", f"{volume}/hard-link"),
            request("symbolic", "stat", f"{volume}/symbolic"),
            cat("link_bytes", f"{volume}/symbolic", 0, 30),
            cat("range", f"{volume}{path}", 3103, 271),
            cat("sparse_range", f"{volume}/sparse.bin", 32768 - 17, 75),
            digest("sparse_hash", f"{volume}/sparse.bin"),
            digest("hash", node),
            export("export", node, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "reiser4", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == len(volume_bytes)
        if base:
            assert info["tableScheme"] == "mbr"
        assert names(rows["root"]) == {"nested", "hard-link", "sparse.bin", "symbolic"}
        assert names(rows["nested"]) == {"tail.txt", "native-long-name-for-hash.bin"}
        assert rows["file"]["inode"] == 0x10002
        assert rows["tail"]["inode"] == rows["hardlink"]["inode"] > 0
        assert rows["symbolic"]["linkTarget"] == "nested/tail.txt"
        assert rows["link_bytes"] == hex_of(b"reiser4 inline bytes\n")
        assert rows["range"] == hex_of(payload[3103:3374])
        assert rows["sparse_range"] == hex_of(bytes(17) + payload[:58])
        assert sha256(rows["sparse_hash"]) == "fa156d414f5af9579eb6d30f917c1629772811682bb39232bc752821fadd2ee6"
        assert sha256(rows["hash"]) == expected_hash
        assert output.read_bytes() == payload
        source.unlink()
    print("Reiser4 CLI tests passed: both key sizes, protected nodes, MBR, links, sparse ranges, hash and export")


def test_ocfs1(cli, root):
    fixture = VENDOR / "ocfs1-reader/tests/fixtures/oraclecfs-12.zlib"
    raw = zlib.decompress(fixture.read_bytes())
    volume = "/ev/part:0"
    path = "/archive/archive-file.dbf"
    entry = at(path_id("ocfs1:part:0:", path))
    chunk = 131072
    payload = b"".join(bytes((i * 29 + i // 193 + run * 7) & 255 for i in range(chunk)) for run in range(4))[:-71]
    for base in (0, 4096):
        source = root / f"ocfs1-{base}.img"
        write_partition_image(source, raw, base, 0x83)
        output = root / f"ocfs1-{base}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", volume),
            ls("archive", f"{volume}/archive"),
            request("file", "stat", entry),
            request("link", "stat", f"{volume}/shortcut"),
            cat("direct", f"{volume}/control.bin", 690, 100),
            cat("cross_leaf", entry, 2 * chunk - 9, 75),
            cat("empty", f"{volume}/empty.dbf"),
            digest("hash", f"{volume}/shortcut"),
            export("export", entry, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "ocfs1", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == len(raw)
        if base:
            assert info["tableScheme"] == "mbr"
        assert names(rows["root"]) == {"archive", "control.bin", "shortcut", "empty.dbf"}
        assert names(rows["archive"]) == {"archive-file.dbf"}
        assert rows["file"]["size"] == len(payload)
        assert rows["link"]["linkTarget"] == "archive/archive-file.dbf"
        assert rows["direct"] == hex_of(bytes((i ^ 0xA5) & 255 for i in range(690, 715)))
        assert rows["cross_leaf"] == hex_of(payload[2 * chunk - 9:2 * chunk + 66])
        assert rows["empty"] == hex_of(b"")
        assert sha256(rows["hash"]) == hashlib.sha256(payload).hexdigest()
        assert output.read_bytes() == payload
        source.unlink()
    print("OCFS1 CLI tests passed: bare and MBR volumes, directory chains, local and external extents, hash and export")


def write_partition_image(source, volume_bytes, base, partition_type):
    if base:
        mbr = bytearray(base)
        mbr[450] = partition_type
        struct.pack_into("<II", mbr, 454, base // 512, len(volume_bytes) // 512)
        mbr[510:512] = b"\x55\xaa"
        source.write_bytes(mbr + volume_bytes)
    else:
        source.write_bytes(volume_bytes)


def test_qnx_etfs(cli, root):
    fixtures = VENDOR / "qnx-reader/tests/fixtures"
    volume = "/ev/part:0"
    for label, base in (("nfi-etfs-old", 0), ("nfi-etfs", 4096), ("nfi-etfs-transactions", 0)):
        volume_bytes = zlib.decompress((fixtures / f"{label}.zlib").read_bytes())
        source = root / f"{label}.img"
        write_partition_image(source, volume_bytes, base, 0x77)
        message = "/directory_1/directory_1_1/message.txt"
        entry = at(path_id("qnxetfs:part:0:", message))
        output = root / f"{label}-export.bin"
        requests = [
            add(source),
            ls("root", volume),
            request("link", "stat", f"{volume}/directory_1/link_message.txt"),
            cat("linked", f"{volume}/directory_1/link_message.txt"),
            export("export", entry, output),
        ]
        if label != "nfi-etfs-transactions":
            with tarfile.open(fixtures / f"{label}.tar.gz") as archive:
                large = archive.extractfile("this_file_is_large").read()
            requests.extend([
                cat("range", f"{volume}/this_file_is_large", 5091, 2089),
                digest("hash", f"{volume}/this_file_is_large"),
            ])
        rows = cli.results(requests)
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "qnxetfs", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == len(volume_bytes)
        if base:
            assert info["tableScheme"] == "mbr"
        assert "this_file_is_removed" not in names(rows["root"])
        assert "this_file_is_not_renamed" not in names(rows["root"])
        assert rows["link"]["linkTarget"] == "directory_1_1/message.txt"
        assert rows["linked"] == hex_of(b"This is a test file system!\n")
        assert output.read_bytes() == b"This is a test file system!\n"
        if label != "nfi-etfs-transactions":
            assert rows["range"] == hex_of(large[5091:7180])
            assert sha256(rows["hash"]) == hashlib.sha256(large).hexdigest()
        source.unlink()
    print("QNX ETFS CLI tests passed: native flash, exact transaction export length, MBR, links, ranges and export")


def test_qnx_efs(cli, root):
    fixtures = VENDOR / "qnx-reader/tests/fixtures"
    original = zlib.decompress((fixtures / "nfi-efs.zlib").read_bytes())
    with tarfile.open(fixtures / "nfi-efs.tar.gz") as archive:
        large = archive.extractfile("this_file_is_large").read()
    unit = 131072
    relocated = bytearray(original[unit:2 * unit] + original[:unit] + original[2 * unit:])
    struct.pack_into("<H", relocated, unit + 52, 1)
    volume = "/ev/part:0"
    for label, volume_bytes, base in (("native", original, 0), ("relocated", relocated, 4096)):
        source = root / f"qnx-efs-{label}.img"
        write_partition_image(source, volume_bytes, base, 0x77)
        large_path = f"{volume}/this_file_is_large"
        entry = at(path_id("qnxefs:part:0:", "/this_file_is_large"))
        link = f"{volume}/directory_1/link_message.txt"
        output = root / f"qnx-efs-{label}-export.bin"
        rows = cli.results([
            add(source),
            ls("root", volume),
            request("link", "stat", link),
            cat("message", link),
            cat("range", entry, 1001, 2089),
            digest("hash", large_path),
            export("export", entry, output),
        ])
        info = opened(rows)
        part, = info["partitions"]
        assert part["kind"] == "qnxefs", info
        assert part["startLba"] * info["sectorSize"] == base
        assert part["lengthLba"] * info["sectorSize"] == len(volume_bytes)
        if base:
            assert info["tableScheme"] == "mbr"
        assert "this_file_is_removed" not in names(rows["root"])
        assert "this_file_is_not_renamed" not in names(rows["root"])
        assert rows["link"]["linkTarget"] == "directory_1_1/message.txt"
        assert rows["message"] == hex_of(b"This is a test file system!\n")
        assert rows["range"] == hex_of(large[1001:3090])
        assert sha256(rows["hash"]) == hashlib.sha256(large).hexdigest()
        assert output.read_bytes() == large
        source.unlink()
    print("QNX EFS CLI tests passed: native flash, relocated logical units, MBR, extents, links, hash and export")


def test_formats(cli, root):
    firmware_node = path_id("squashfs:part:0:", "/hello")
    firmware_content = b"Independent DD-WRT fixture.\n" * 64

    # Independent synthetic Buffalo ENC/bgn + TRX + DD-WRT SquashFS v3.
    for fixture_name in ("synthetic.enc", "synthetic-bgn.enc"):
        rows = cli.results([
            add(FIRMWARE / fixture_name),
            ls("list", "/ev/part:0"),
            cat("read", at(firmware_node), 11, 93),
            digest("hash", "/ev/part:0/hello"),
        ])
        assert opened(rows)["sourceFormat"] == "firmware" and opened(rows)["sectorSize"] == 1
        assert rows["list"]["items"][0]["name"] == "hello"
        assert rows["read"]["data"] == firmware_content[11:104].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(firmware_content).hexdigest()

    # Independent MINIFS v3 fixture: shared LZMA chunks and native commands.
    minifs_fixture = VENDOR / "minifs/tests/fixtures/synthetic.minifs"
    minifs_content = bytes(range(256)) * 289 + b"MINIFS test\n"
    minifs_source = minifs_fixture.read_bytes()
    minifs_header = bytearray(148)
    for offset, value in [(0x18, 128 + len(minifs_source)), (0x20, 1), (0x54, 128), (0x58, len(minifs_source))]:
        struct.pack_into(">I", minifs_header, offset, value)
    minifs_wrapped = root / "minifs-firmware.bin"
    minifs_wrapped.write_bytes(minifs_header + minifs_source)
    minifs_node = path_id("minifs:part:0:", "/web/js/app.js")
    for minifs_path in (minifs_fixture, minifs_wrapped):
        rows = cli.results([
            add(minifs_path),
            ls("list", "/ev/part:0"),
            cat("read", at(minifs_node), 65530, 100),
            digest("hash", "/ev/part:0/web/js/app.js"),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "minifs" and opened(rows)["sectorSize"] == 1
        assert any(item["name"] == "web" and item["kind"] == "dir" for item in rows["list"]["items"])
        assert rows["read"]["data"] == minifs_content[65530:65630].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(minifs_content).hexdigest()

    # EROFS and F2FS: independent/reference fixtures.
    f2fs_path = root / "synthetic.f2fs"
    f2fs_path.write_bytes(zlib.decompress((VENDOR / "f2fs-reader/tests/fixtures/synthetic.f2fs.zlib").read_bytes()))
    f2fs_body = bytearray(bytes(range(256)) * (1000 * 16) + bytes(range(37)))
    f2fs_body[3 * 4096:5 * 4096] = bytes(8192)
    for flash_kind, flash_path, flash_file, flash_body, flash_offset in [
        ("erofs", VENDOR / "lamfold-erofs/tests/fixtures/erofs.img", "/big.dat", bytes(range(256)) * 1171 + bytes(range(224)), 4090),
        ("f2fs", f2fs_path, "/big.bin", bytes(f2fs_body), 913 * 4096 - 9),
    ]:
        rows = cli.results([
            add(flash_path),
            ls("list", "/ev/part:0"),
            cat("read", at(path_id(flash_kind + ":part:0:", flash_file)), flash_offset, 121),
            digest("hash", "/ev/part:0" + flash_file),
        ])
        assert opened(rows)["partitions"][0]["kind"] == flash_kind and opened(rows)["sectorSize"] == 1
        assert rows["list"]["items"]
        assert rows["read"]["data"] == flash_body[flash_offset:flash_offset + 121].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(flash_body).hexdigest()

    # bcachefs: native snapshot namespace, checksummed reflinks and >5 GiB holes.
    bcachefs_source = zlib.decompress((VENDOR / "bcachefs-reader/tests/fixtures/invented.zlib").read_bytes())
    bcachefs_path = root / "invented.bcachefs"
    bcachefs_path.write_bytes(bcachefs_source)
    bcachefs_export = root / "bcachefs-export.bin"
    bcachefs_node = path_id("bcachefs:part:0:", "/checksum.bin")
    bcachefs_body = bytes((index * 17 + 3) % 256 for index in range(513))
    rows = cli.results([
        add(bcachefs_path),
        ls("list", "/ev/part:0"),
        cat("live", "/ev/part:0/sv/version.txt", length=4),
        cat("past", at(path_id("bcachefs:part:0:", "/snap/version.txt")), length=4),
        cat("sparse", at(path_id("bcachefs:part:0:", "/sparse.bin")), 5 * 1024**3, 5),
        digest("hash", at(bcachefs_node)),
        export("export", at(bcachefs_node), bcachefs_export),
    ])
    assert opened(rows)["partitions"][0]["kind"] == "bcachefs"
    assert len(rows["list"]["items"]) == 10
    assert rows["live"] == hex_of(b"LIVE") and rows["past"] == hex_of(b"PAST") and rows["sparse"] == hex_of(b"TAIL!")
    assert sha256(rows["hash"]) == hashlib.sha256(bcachefs_body).hexdigest()
    assert bcachefs_export.read_bytes() == bcachefs_body
    assert bcachefs_path.read_bytes() == bcachefs_source
    bcachefs_backup = bytearray(bcachefs_source)
    bcachefs_backup[4096] ^= 1
    bcachefs_backup_path = root / "backup-only.bcachefs"
    bcachefs_backup_path.write_bytes(bcachefs_backup)
    rows = cli.results([add(bcachefs_backup_path), digest("hash", at(bcachefs_node))])
    assert opened(rows)["partitions"][0]["kind"] == "bcachefs"
    assert sha256(rows["hash"]) == hashlib.sha256(bcachefs_body).hexdigest()
    if os.environ.get("BCACHEFS_ORACLE_DIR"):
        native_root = Path(os.environ["BCACHEFS_ORACLE_DIR"])
        for native_name in ["512-none", "4096-lz4", "4096-zstd", "4096-gzip"]:
            manifest = json.loads((native_root / (native_name + ".json")).read_text(encoding="utf8"))
            expected = {entry["path"]: entry for entry in manifest["entries"]}
            rows = cli.results([
                add(native_root / (native_name + ".img")),
                ls("list", "/ev/part:0"),
                digest("hash", at(path_id("bcachefs:part:0:", "/compressible.bin"))),
            ])
            assert opened(rows)["partitions"][0]["kind"] == "bcachefs"
            assert "中文\\证据.txt" in names(rows["list"])
            assert sha256(rows["hash"]) == expected["compressible.bin"]["sha256"]

    # NILFS2: current checkpoint, sparse/direct B-tree reads and absent native atime.
    nilfs2_source = zlib.decompress((VENDOR / "nilfs2-reader/tests/fixtures/invented-1024.zlib").read_bytes())
    nilfs2_path = root / "invented.nilfs2"
    nilfs2_path.write_bytes(nilfs2_source)
    nilfs2_node = path_id("nilfs2:part:0:", "/hello")
    nilfs2_hello = hashlib.sha256(b"hello NILFS2 evidence\n").hexdigest()
    rows = cli.results([
        add(nilfs2_path),
        ls("list", "/ev/part:0"),
        digest("hash", "/ev/part:0/hello"),
        cat("range", at(path_id("nilfs2:part:0:", "/sparse")), 1019, 97),
    ])
    assert opened(rows)["partitions"][0]["kind"] == "nilfs2"
    assert len(rows["list"]["items"]) == 9
    assert sha256(rows["hash"]) == nilfs2_hello
    assert rows["range"]["data"] == (bytes(5) + bytes((index * 23 + 7) & 255 for index in range(92))).hex()
    assert nilfs2_path.read_bytes() == nilfs2_source
    nilfs2_backup = bytearray(nilfs2_source)
    nilfs2_backup[1024 + 16] ^= 1
    nilfs2_backup_path = root / "backup-only.nilfs2"
    nilfs2_backup_path.write_bytes(nilfs2_backup)
    rows = cli.results([add(nilfs2_backup_path), digest("hash", at(nilfs2_node))])
    assert opened(rows)["partitions"][0]["kind"] == "nilfs2" and sha256(rows["hash"]) == nilfs2_hello
    assert nilfs2_backup_path.read_bytes() == nilfs2_backup
    if os.environ.get("NILFS2_ORACLE_DIR"):
        native_root = Path(os.environ["NILFS2_ORACLE_DIR"])
        for native_name in ["nilfs2-1024-2048-block_count", "nilfs2-2048-1024-block_count", "nilfs2-4096-512-block_count",
                            "nilfs2-4096-2048-none", "nilfs2-4096-512-cleaned"]:
            selected_root = Path(os.environ.get("NILFS2_GC_ORACLE_DIR", str(native_root))) if native_name.endswith("cleaned") else native_root
            manifest = json.loads((selected_root / (native_name + ".json")).read_text(encoding="utf8"))
            image = selected_root / (native_name + ".img")
            rows = cli.results([
                add(image),
                ls("list", "/ev/part:0"),
                *[digest(name, at(path_id("nilfs2:part:0:", name))) for name in ["/large", "/sparse", "/fragmented", "/versioned"]],
            ])
            assert opened(rows)["partitions"][0]["kind"] == "nilfs2"
            assert "中文\\evidence" in names(rows["list"])
            for name in ["/large", "/sparse", "/fragmented", "/versioned"]:
                assert sha256(rows[name]) == manifest["records"][name]["sha256"], name

    # OCFS2 and GFS2: resource allocation, a native-sized clean journal, sparse ranges and EAs.
    for cluster_kind, fixture, entries, hello in [
        ("ocfs2", "ocfs2-reader/tests/fixtures/discontiguous-ecc.zlib", 8, b"hello OCFS2 evidence\n"),
        ("gfs2", "gfs2-reader/tests/fixtures/invented-4096.zlib", 6, b"hello native-style GFS2\n"),
    ]:
        source = zlib.decompress((VENDOR / fixture).read_bytes())
        image = root / ("invented." + cluster_kind)
        image.write_bytes(source)
        hello_node = path_id(cluster_kind + ":part:0:", "/hello")
        attribute_export = root / (cluster_kind + "-attribute.bin")
        rows = cli.results([
            add(image),
            ls("list", "/ev/part:0"),
            digest("hash", "/ev/part:0/hello"),
            cat("range", at(path_id(cluster_kind + ":part:0:", "/sparse")), 4091, 97),
            request("streams", "streams", at(hello_node)),
            cat("inline", at(hello_node), length=100, stream="user.inline"),
            export("export", at(hello_node), attribute_export, stream="user.external"),
        ])
        assert opened(rows)["partitions"][0]["kind"] == cluster_kind
        assert len(rows["list"]["items"]) == entries
        assert sha256(rows["hash"]) == hashlib.sha256(hello).hexdigest()
        assert rows["range"]["data"] == (bytes(5) + bytes((index * 17 + 5) & 255 for index in range(92))).hex()
        assert "user.external" in names(rows["streams"])
        assert rows["inline"] == hex_of(b"\0\xfffixture")
        assert attribute_export.read_bytes() == bytes(range(100))
        assert image.read_bytes() == source

    # Opt-in native GFS2 and OCFS2 inputs generated by mkfs/Linux/fsck, verified by the reader audit.
    for cluster_kind, variable, native_names, extra_files in [
        ("gfs2", "GFS2_ORACLE_DIR", ["gfs2-512-1801", "gfs2-1024-1802", "gfs2-2048-1801", "gfs2-4096-1802"], []),
        ("ocfs2", "OCFS2_ORACLE_DIR", ["ocfs2-512-4096", "ocfs2-1024-4096", "ocfs2-2048-16384", "ocfs2-4096-65536"],
         ["/fragmented", "/clone-fragmented", "/unwritten", "/preallocated-empty"]),
    ]:
        if not os.environ.get(variable):
            continue
        native_root = Path(os.environ[variable])
        for native_name in native_names:
            manifest = json.loads((native_root / (native_name + ".json")).read_text(encoding="utf8"))
            present = [name for name in ["/large", *extra_files] if name in manifest["records"]]
            rows = cli.results([
                add(native_root / (native_name + ".img")),
                ls("list", "/ev/part:0"),
                *[digest(name, at(path_id(cluster_kind + ":part:0:", name))) for name in present],
                cat("attribute", at(path_id(cluster_kind + ":part:0:", "/hello")), length=97, stream="user.large"),
            ])
            assert opened(rows)["partitions"][0]["kind"] == cluster_kind
            assert "中文\\evidence" in names(rows["list"])
            for name in present:
                assert sha256(rows[name]) == manifest["records"][name]["sha256"], name
            assert rows["attribute"] == hex_of(bytes((index * 23 + 7) & 255 for index in range(97)))

    # BeFS: both endian orders, indirect/double runs and typed attributes.
    for befs_name, befs_block in [("le-1024-2560", 1024), ("be-4096-2560", 4096), ("le-1024-10000", 1024)]:
        befs_path = root / (befs_name + ".befs")
        befs_source = zlib.decompress((VENDOR / ("befs-reader/tests/fixtures/" + befs_name + ".zlib")).read_bytes())
        befs_path.write_bytes(befs_source)
        befs_node = at(path_id("befs:part:0:", "/big.bin"))
        befs_body = bytes((index * 17 + 5) & 255 for index in range(26 * befs_block - 37))
        befs_attribute = bytes((index * 23 + 7) & 255 for index in range(12003))
        befs_export = root / (befs_name + "-export.bin")
        befs_attribute_export = root / (befs_name + "-attribute.bin")
        befs_offset = 18 * befs_block - 11
        rows = cli.results([
            add(befs_path),
            ls("list", "/ev/part:0"),
            cat("read", befs_node, befs_offset, 73),
            digest("hash", "/ev/part:0/big.bin"),
            export("export", befs_node, befs_export),
            request("streams", "streams", befs_node),
            cat("inline", befs_node, length=100, stream="custom"),
            cat("attribute", befs_node, length=97, stream="big-attribute"),
            export("attribute-export", befs_node, befs_attribute_export, stream="big-attribute"),
            cat("name", at(path_id("befs:part:0:", "/中文\\evidence")), length=100),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "befs"
        assert "中文\\evidence" in names(rows["list"])
        assert rows["read"]["data"] == befs_body[befs_offset:befs_offset + 73].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(befs_body).hexdigest()
        assert befs_export.read_bytes() == befs_body
        assert "big-attribute" in names(rows["streams"])
        assert rows["inline"] == hex_of(b"\x00\xffinline\x80")
        assert rows["attribute"] == hex_of(befs_attribute[:97])
        assert befs_attribute_export.read_bytes() == befs_attribute
        assert rows["name"] == hex_of(b"names")
        assert befs_path.read_bytes() == befs_source

    # Opt-in publisher-verified hrev60295 native image from test-befs-native.
    if os.environ.get("BEFS_ORACLE_IMAGE"):
        package = "/system/packages/haiku-r1~beta6_hrev60295-1-x86_64.hpkg"
        rows = cli.results([add(os.environ["BEFS_ORACLE_IMAGE"]), digest("hash", at(path_id("befs:part:0:", package)))])
        assert opened(rows)["partitions"][0]["kind"] == "befs"
        assert sha256(rows["hash"]) == "f624ff224ac0cc68c22300e15babc3e058b09fc145f0af19f3257c0c5e9c85dc"

    # Amiga: independent OFS/FFS variants, native multi-block RDB and links.
    amiga_body = bytes((index * 17 + 5) & 255 for index in range(200013))
    for amiga_name, amiga_parts in [("dos0-512-1760", (0,)), ("links-dos3", (0,)), ("dos7-4096-1760", (0,)), ("rdb-multiblock", (0, 1, 3))]:
        amiga_path = root / (amiga_name + ".hdf")
        amiga_source = zlib.decompress((VENDOR / ("amiga-reader/tests/fixtures/" + amiga_name + ".zlib")).read_bytes())
        amiga_path.write_bytes(amiga_source)
        for amiga_part in amiga_parts:
            prefix = f"amiga:part:{amiga_part}:"
            amiga_export = root / f"{amiga_name}{amiga_part}-export.bin"
            rows = cli.results([
                add(amiga_path),
                ls("list", f"/ev/part:{amiga_part}"),
                ls("nested", at(path_id(prefix, "/sub"))),
                cat("read", at(path_id(prefix, "/Café.bin")), 73720, 73),
                digest("hash", f"/ev/part:{amiga_part}/Café.bin"),
                export("export", at(path_id(prefix, "/Café.bin")), amiga_export),
                cat("name", at(path_id(prefix, "/back\\slash")), length=100),
            ])
            assert opened(rows)["partitions"][amiga_part]["kind"] == "amiga"
            assert "Café.bin" in names(rows["list"])
            assert len(rows["nested"]["items"]) == 101
            assert rows["read"]["data"] == amiga_body[73720:73793].hex()
            assert sha256(rows["hash"]) == hashlib.sha256(amiga_body).hexdigest()
            assert amiga_export.read_bytes() == amiga_body
            assert rows["name"] == hex_of(b"Unix-safe evidence name")
            assert amiga_path.read_bytes() == amiga_source

    # ReiserFS v3: both key/bitmap layouts, sparse indirect and packed tails.
    for reiser_name, reiser_block in [("v35-1024-old", 1024), ("v36-4096-relocated", 4096)]:
        reiser_path = root / (reiser_name + ".reiserfs")
        reiser_path.write_bytes(zlib.decompress((VENDOR / ("reiserfs-reader/tests/fixtures/" + reiser_name + ".zlib")).read_bytes()))
        reiser_body = (bytes((index * 17 + 5) & 255 for index in range(reiser_block)) + bytes(reiser_block)
                       + bytes((index * 23 + 7) & 255 for index in range(reiser_block)) + b"packed tail")
        reiser_node = at(path_id("reiserfs:part:0:", "/big.bin"))
        reiser_export = root / (reiser_name + "-export.bin")
        reiser_offset = reiser_block - 7
        rows = cli.results([
            add(reiser_path),
            ls("list", "/ev/part:0"),
            ls("nested", "/ev/part:0/sub"),
            cat("read", reiser_node, reiser_offset, 71),
            digest("hash", "/ev/part:0/big.bin"),
            export("export", reiser_node, reiser_export),
            cat("unicode", at(path_id("reiserfs:part:0:", "/中文\\evidence")), length=100),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "reiserfs"
        assert "中文\\evidence" in names(rows["list"]) and "hidden" not in names(rows["list"])
        assert rows["nested"]["items"][0]["name"] == "nested"
        assert rows["read"]["data"] == reiser_body[reiser_offset:reiser_offset + 71].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(reiser_body).hexdigest()
        assert reiser_export.read_bytes() == reiser_body
        assert rows["unicode"] == hex_of(b"names")

    # JFS: multiple IAGs, sparse/unwritten data, Unix names and hardlinks.
    for jfs_name, jfs_block in [("multi-iag-512-legacy", 512), ("multi-iag-4096-indexed", 4096)]:
        jfs_path = root / (jfs_name + ".jfs")
        jfs_path.write_bytes(zlib.decompress((VENDOR / ("jfs-reader/tests/fixtures/" + jfs_name + ".zlib")).read_bytes()))
        jfs_expected = bytearray(12 * jfs_block + 37)
        for logical, count, factor in [(0, 2 * jfs_block, 17), (10 * jfs_block, jfs_block, 23), (12 * jfs_block, 37, 29)]:
            jfs_expected[logical:logical + count] = bytes((index * factor + 5) & 255 for index in range(count))
        jfs_node = at(path_id("jfs:part:0:", "/big.bin"))
        jfs_long_name = "long_中文_" + "x" * 180 + "\\tail"
        jfs_export = root / (jfs_name + "-export.bin")
        jfs_offset = 6 * jfs_block - 7
        rows = cli.results([
            add(jfs_path),
            ls("list", "/ev/part:0"),
            ls("nested", "/ev/part:0/sub"),
            cat("read", jfs_node, jfs_offset, 71),
            digest("hash", "/ev/part:0/big.bin"),
            export("export", jfs_node, jfs_export),
            cat("unicode", at(path_id("jfs:part:0:", "/" + jfs_long_name)), length=100),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "jfs"
        assert jfs_long_name in names(rows["list"])
        assert rows["nested"]["items"][0]["name"] == "nested.txt"
        assert rows["read"]["data"] == jfs_expected[jfs_offset:jfs_offset + 71].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(jfs_expected).hexdigest()
        assert jfs_export.read_bytes() == jfs_expected
        assert rows["unicode"] == hex_of(b"unicode path")

    # Classic HFS: retain local times and independently export both forks.
    hfs_path = root / "classic-hfs.img"
    hfs_path.write_bytes(zlib.decompress((VENDOR / "classic-hfs/tests/fixtures/fragmented-1024.zlib").read_bytes()))
    hfs_node = at(path_id("hfs:part:0:", "/Café.bin"))
    hfs_body = bytes((index * 17 + 3) & 255 for index in range(3 * 1024 + 37))
    hfs_resource = bytes((index * 23 + 5) & 255 for index in range(3 * 1024 + 19))
    hfs_data_export = root / "hfs-data.bin"
    hfs_resource_export = root / "hfs-resource.bin"
    rows = cli.results([
        add(hfs_path),
        ls("list", "/ev/part:0"),
        cat("read", hfs_node, 3065, 121),
        digest("hash", "/ev/part:0/Café.bin"),
        export("export", hfs_node, hfs_data_export),
        request("streams", "streams", hfs_node),
        cat("resource", hfs_node, length=100, stream="com.apple.ResourceFork"),
        export("resource-export", hfs_node, hfs_resource_export, stream="com.apple.ResourceFork"),
    ])
    assert opened(rows)["partitions"][0]["kind"] == "hfs"
    assert "Café.bin" in names(rows["list"])
    assert rows["read"]["data"] == hfs_body[3065:3065 + 121].hex()
    assert sha256(rows["hash"]) == hashlib.sha256(hfs_body).hexdigest()
    assert hfs_data_export.read_bytes() == hfs_body
    assert "com.apple.ResourceFork" in names(rows["streams"])
    assert rows["resource"] == hex_of(hfs_resource[:100])
    assert hfs_resource_export.read_bytes() == hfs_resource

    # Minix V1/V2/V3: all read paths share sparse and indirect mappings.
    for minix_name, zone_size, pointers in [("v1-30-1024-1-be", 2048, 512), ("v2-30-1024-2-le", 4096, 256),
                                            ("v3-60-1024-0-le", 1024, 256)]:
        minix_path = root / (minix_name + ".img")
        minix_path.write_bytes(zlib.decompress((VENDOR / "minix-reader/tests/fixtures" / (minix_name + ".zlib")).read_bytes()))
        minix_expected = bytearray((7 + pointers + 2) * zone_size + 37)
        for logical_zone, factor in [(0, 13), (2, 17), (7, 17), (7 + pointers + 2, 19)]:
            start = logical_zone * zone_size
            count = min(zone_size, len(minix_expected) - start)
            minix_expected[start:start + count] = bytes((index * factor + 7) & 255 for index in range(count))
        minix_node = at(path_id("minix:part:0:", "/big.bin"))
        minix_export = root / (minix_name + "-export.bin")
        minix_offset = 7 * zone_size - 7
        requests = [
            add(minix_path),
            ls("list", "/ev/part:0"),
            ls("nested", "/ev/part:0/sub"),
            cat("read", minix_node, minix_offset, 121),
            digest("hash", "/ev/part:0/big.bin"),
            export("export", minix_node, minix_export),
        ]
        if not minix_name.startswith("v1"):
            requests.append(cat("triple", at(path_id("minix:part:0:", "/triple.bin")),
                                (7 + pointers + pointers * pointers) * zone_size, 37))
        rows = cli.results(requests)
        assert opened(rows)["partitions"][0]["kind"] == "minix"
        assert "text.txt" in names(rows["list"]) and "nested.txt" in names(rows["nested"])
        assert rows["read"]["data"] == minix_expected[minix_offset:minix_offset + 121].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(minix_expected).hexdigest()
        assert minix_export.read_bytes() == minix_expected
        if "triple" in rows:
            assert rows["triple"] == hex_of(bytes((index * 19 + 7) & 255 for index in range(37)))

    # UDF reference images: FE/EFE, nested tree, range/hash/export.
    for udf_name, udf_file, udf_expected in [
        ("udf102", "/big.dat", bytes(index % 256 for index in range(5000))),
        ("udf201", "/big.bin", bytes((index * 11 + index // 257) & 255 for index in range(70000))),
    ]:
        udf_path = root / (udf_name + ".iso")
        udf_path.write_bytes(zlib.decompress((VENDOR / "lamfold-udf/tests/fixtures" / (udf_name + ".zlib")).read_bytes()))
        udf_node = at(path_id("udf:part:0:", udf_file))
        udf_export = root / (udf_name + "-export.bin")
        rows = cli.results([
            add(udf_path),
            ls("list", "/ev/part:0"),
            ls("nested", "/ev/part:0/sub"),
            cat("read", udf_node, 2041, 121),
            digest("hash", "/ev/part:0" + udf_file),
            export("export", udf_node, udf_export),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "udf"
        assert "sub" in names(rows["list"]) and "inner.txt" in names(rows["nested"])
        assert rows["read"]["data"] == udf_expected[2041:2162].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(udf_expected).hexdigest()
        assert udf_export.read_bytes() == udf_expected

    # QNX4 extent-chain and independently QNX-created QNX6 reference image.
    qnx_fixtures = VENDOR / "qnx-reader/tests/fixtures"
    with tarfile.open(qnx_fixtures / "nfi-qnx6.tar.gz", "r:gz") as archive:
        qnx6_body = archive.extractfile("this_file_is_large").read()
    for qnx_kind, qnx_fixture, qnx_file, qnx_expected in [
        ("qnx4", "qnx4.zlib", "/big.bin", bytes((index * 13 + 7) % 256 for index in range(1300))),
        ("qnx6", "nfi-qnx6.zlib", "/this_file_is_large", qnx6_body),
    ]:
        qnx_path = root / (qnx_kind + ".img")
        qnx_path.write_bytes(zlib.decompress((qnx_fixtures / qnx_fixture).read_bytes()))
        rows = cli.results([
            add(qnx_path),
            ls("list", "/ev/part:0"),
            cat("read", at(path_id(qnx_kind + ":part:0:", qnx_file)), 500, 121),
            digest("hash", "/ev/part:0" + qnx_file),
        ])
        assert opened(rows)["partitions"][0]["kind"] == qnx_kind
        assert rows["list"]["items"]
        assert rows["read"]["data"] == qnx_expected[500:621].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(qnx_expected).hexdigest()

    # Native CramFS / ROMFS: library-backed tree, page-crossing reads and hash.
    rom_fixtures = REPOSITORY / "src-tauri/test_data/rom-images"
    rom_content = (rom_fixtures / "payload.bin").read_bytes()
    for rom_name, rom_kind in [("synthetic-le.cramfs", "cramfs"), ("synthetic-be.cramfs", "cramfs"), ("synthetic.romfs", "romfs")]:
        rows = cli.results([
            add(rom_fixtures / rom_name),
            ls("list", "/ev/part:0"),
            cat("read", at(path_id(rom_kind + ":part:0:", "/payload.bin")), 8180, 100),
            digest("hash", "/ev/part:0/payload.bin"),
        ])
        assert opened(rows)["partitions"][0]["kind"] == rom_kind and opened(rows)["sectorSize"] == 1
        assert any(item["name"] == "nested" and item["kind"] == "dir" for item in rows["list"]["items"])
        assert rows["read"]["data"] == rom_content[8180:8280].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(rom_content).hexdigest()

    # Native firmware envelopes retain the established CramFS content paths.
    for wrapper in ("synthetic-uimage.bin", "synthetic-safeloader.bin", "synthetic.chk"):
        rows = cli.results([
            add(FIRMWARE / wrapper),
            ls("root", "/ev/part:0"),
            cat("read", at(path_id("cramfs:part:0:", "/payload.bin")), 8180, 100),
            digest("hash", "/ev/part:0/payload.bin"),
        ])
        assert opened(rows)["sourceFormat"] == "firmware"
        assert rows["read"]["data"] == rom_content[8180:8280].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(rom_content).hexdigest()

    # Independently encoded v3 gzip / raw LZMA / LZMA-alone / properties,
    # in both byte orders and with range reads crossing compressed blocks.
    squashfs_fixtures = VENDOR / "backhand/test_data"
    squashfs_content = b"Independent Rust LZMA fixture.\n" * 350
    for mode in ("raw", "alone", "props", "gzip"):
        for endian in ("le", "be"):
            rows = cli.results([
                add(squashfs_fixtures / f"lzma-v3-{mode}-{endian}.squashfs"),
                ls("root", "/ev/part:0"),
                cat("read", at(firmware_node), 4080, 100),
                digest("hash", "/ev/part:0/hello"),
            ])
            assert rows["root"]["items"][0]["name"] == "hello"
            assert rows["read"]["data"] == squashfs_content[4080:4180].hex()
            assert sha256(rows["hash"]) == hashlib.sha256(squashfs_content).hexdigest()

    # Legacy SquashFS: independent v1/v2 bitfields, fragments, both byte
    # orders, bounded markerless LZMA and the encrypted firmware path.
    legacy_fixtures = squashfs_fixtures / "legacy"
    legacy_paths = sorted(legacy_fixtures.glob("*.squashfs"))
    legacy_paths.append(FIRMWARE / "synthetic-legacy-squashfs.enc")
    for path in legacy_paths:
        payload_name = "payload-32k.bin" if "32k" in path.name else "payload-64k.bin" if "64k" in path.name else "payload.bin"
        legacy_content = (legacy_fixtures / payload_name).read_bytes()
        rows = cli.results([
            add(path),
            ls("root", "/ev/part:0"),
            cat("read", at(path_id("squashfs:part:0:", "/payload.bin")), 8180, 100),
            digest("hash", "/ev/part:0/payload.bin"),
        ])
        assert opened(rows)["partitions"][0]["kind"] == "squashfs"
        assert names(rows["root"]) == {"payload.bin", "alias.bin", "子目录", "link", "huge-link", "empty.txt",
                                       "block", "char", "fifo", "socket"}
        assert rows["read"]["data"] == legacy_content[8180:8280].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(legacy_content).hexdigest()

    # Sercomm source geometry stays compressed; UBI navigation uses decoded bytes.
    sercomm_stored = root / "sercomm-stored.img"
    sercomm_stored.write_bytes(zlib.decompress((FIRMWARE / "synthetic-sercomm-stored.img.zlib").read_bytes()))
    sercomm_node = at(path_id("squashfs:ubi:part:0:2:", "/hello"))
    for sercomm_path in [FIRMWARE / "synthetic-sercomm-deflate.img", sercomm_stored]:
        rows = cli.results([
            add(sercomm_path),
            ls("root", "/ev/part:0"),
            ls("files", at("squashfs:ubi:part:0:2:")),
            cat("read", sercomm_node, 11, 93),
            digest("hash", sercomm_node),
        ])
        image = opened(rows)
        assert image["sourceFormat"] == "firmware" and image["sizeBytes"] == sercomm_path.stat().st_size
        assert image["unlockedPartitions"] == []
        assert names(rows["root"]) == {"root", "root.img"}
        assert rows["files"]["items"][0]["name"] == "hello"
        assert rows["read"]["data"] == firmware_content[11:104].hex()
        assert sha256(rows["hash"]) == hashlib.sha256(firmware_content).hexdigest()

    # UBIFS: independent stored/LZO/DEFLATE/Zstd trees and encrypted firmware.
    # JFFS2: version replay, both byte orders, four codecs and firmware bytes.
    ubifs_fixtures = VENDOR / "ubidump/test_data"
    jffs2_fixtures = VENDOR / "jffs2/test_data"
    flash_sets = [
        ("ubifs", (ubifs_fixtures / "payload.bin").read_bytes(), None,
         [ubifs_fixtures / f"synthetic-{codec}.ubifs" for codec in ("stored", "lzo", "deflate", "zstd", "zstd-compressed")]
         + [FIRMWARE / "synthetic-ubifs.enc"]),
        ("jffs2", (jffs2_fixtures / "payload.bin").read_bytes(), {"payload.bin", "alias.bin", "子目录", "link", "empty.txt", "cut.txt"},
         [jffs2_fixtures / f"synthetic-{order}-{codec}.jffs2" for order in ("le", "be") for codec in ("stored", "rtime", "zlib", "lzo")]
         + [FIRMWARE / "synthetic-jffs2.enc"]),
    ]
    for flash_kind, flash_content, root_names, paths in flash_sets:
        for path in paths:
            rows = cli.results([
                add(path),
                ls("root", "/ev/part:0"),
                cat("read", at(path_id(flash_kind + ":part:0:", "/payload.bin")), 8180, 100),
                digest("hash", "/ev/part:0/payload.bin"),
            ])
            assert opened(rows)["partitions"][0]["kind"] == flash_kind
            if root_names is not None:
                assert names(rows["root"]) == root_names
            assert rows["read"]["data"] == flash_content[8180:8280].hex()
            assert sha256(rows["hash"]) == hashlib.sha256(flash_content).hexdigest()

    # YAFFS2 and YAFFS1: NAND geometry, hard links, UTF-8 names and page-crossing ranges.
    yaffs_fixtures = VENDOR / "yaffs2-reader/test_data"
    for yaffs_kind, yaffs_file, yaffs_content, root_names, offset, length, paths in [
        ("yaffs2", "/目录/hello", b"Native YAFFS2 fixture.\n" * 300, {"目录", "alias", "link", "console", "empty"}, 2047, 4097,
         [yaffs_fixtures / "base-le.yaffs", yaffs_fixtures / "base-be-ecc.yaffs", FIRMWARE / "synthetic-yaffs2.enc"]),
        ("yaffs1", "/目录/message", b"Native YAFFS1 data.\n" * 61, {"目录", "alias", "sym", "console", "empty"}, 511, 517,
         [yaffs_fixtures / "legacy-le.yaffs", yaffs_fixtures / "legacy-be.yaffs", FIRMWARE / "synthetic-yaffs1.enc"]),
    ]:
        for path in paths:
            rows = cli.results([
                add(path),
                ls("root", "/ev/part:0"),
                cat("read", at(path_id(yaffs_kind + ":part:0:", yaffs_file)), offset, length),
                digest("hash", "/ev/part:0" + yaffs_file),
            ])
            assert opened(rows)["partitions"][0]["kind"] == yaffs_kind
            assert names(rows["root"]) == root_names
            assert rows["read"]["data"] == yaffs_content[offset:offset + length].hex()
            assert sha256(rows["hash"]) == hashlib.sha256(yaffs_content).hexdigest()
    print("CLI format tests passed: firmware, flash, cluster and legacy file systems by node ID and path")
