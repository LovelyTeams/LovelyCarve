"""Readers through the public CLI commands: each fixture is opened, listed,
read by range, hashed and exported, by node ID and by path.

Fixtures are the readers' committed synthetic or reference images; the
optional native images come from the environment variables named below.
"""
import hashlib
import json
import os
from pathlib import Path
import struct
import tarfile
import zlib

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
