"""Image containers and encrypted volumes through the public CLI commands:
the bytes each one exposes, as files and as whole-evidence exports, and the
credentials that unlock them.

Credentials only ever reach the CLI as @files, and never appear in its output.
"""
import gzip
import hashlib
import io
import json
from pathlib import Path
import struct
import tarfile

from cli_support import REPOSITORY, node, path_id, request

ALIAS = "ev"
VENDOR = REPOSITORY / "src-tauri/vendor"
ENCRYPTED = REPOSITORY / "src-tauri/tests/data/encrypted"


def add(path, *options, identifier="open"):
    return request(identifier, "evidence", "add", path, "--as", ALIAS, *options)


def export_evidence(identifier, target, destination, *options):
    return request(identifier, "evidence", "export", target, "-o", destination, *options)


def opened(rows, identifier="open"):
    return rows[identifier]["items"][0]


def secret(root, name, text):
    path = Path(root) / name
    path.write_text(text, encoding="utf-8")
    return "@" + str(path)


def test_containers(cli, root):
    content = b"hello forensic world\npassword test\n"

    # Parallels: an independently built split plain/sparse disk is exposed as guest bytes.
    bundle = root / "Parallels evidence.hdd"
    bundle.mkdir()
    plain = b"plain guest sector".ljust(512, b"\0")
    (bundle / "plain.hds").write_bytes(plain)
    sparse = bytearray(1024)
    sparse[:16] = b"WithoutFreeSpace"
    for offset, value in [(16, 2), (28, 1), (32, 2), (48, 1), (64, 1)]:
        struct.pack_into("<I", sparse, offset, value)
    struct.pack_into("<Q", sparse, 36, 2)
    sparse[512:] = b"sparse guest sector".ljust(512, b"\0")
    (bundle / "sparse.hds").write_bytes(sparse)
    (bundle / "DiskDescriptor.xml").write_text("""<?xml version="1.0" encoding="UTF-8"?>
<Parallels_disk_image Version="1.0"><Disk_Parameters><Disk_size>3</Disk_size></Disk_Parameters>
<StorageData><Storage><Start>0</Start><End>1</End><Blocksize>1</Blocksize><Image>
<GUID>{11111111-1111-1111-1111-111111111111}</GUID><Type>Plain</Type><File>plain.hds</File>
</Image></Storage><Storage><Start>1</Start><End>3</End><Blocksize>1</Blocksize><Image>
<GUID>{11111111-1111-1111-1111-111111111111}</GUID><Type>Compressed</Type><File>sparse.hds</File>
</Image></Storage></StorageData></Parallels_disk_image>""", encoding="utf-8")
    for suffix, source in [("bundle", bundle), ("descriptor", bundle / "DiskDescriptor.xml")]:
        destination = root / f"phdi-{suffix}.raw"
        rows = cli.results([add(source), export_evidence("export", ALIAS, destination, "--strict"),
                            request("close", "evidence", "close", ALIAS)])
        assert opened(rows)["sourceFormat"] == "phdi", rows
        assert destination.read_bytes() == plain + bytes(sparse[512:]) + bytes(512)
        assert (bundle / "plain.hds").read_bytes() == plain and (bundle / "sparse.hds").read_bytes() == sparse
    # A recognized corrupt descriptor must fail rather than become RAW.
    (bundle / "DiskDescriptor.xml").write_text("broken", encoding="utf-8")
    _, rows = cli.batch([add(bundle / "DiskDescriptor.xml")], expected=1)
    assert "error" in rows[0], rows

    # QCOW: transient credentials, logical bytes, and the source untouched.
    qcow_fixtures = VENDOR / "qcow2/tests/data"
    qcow_plain = (qcow_fixtures / "plain.bin").read_bytes()
    qcow_key = (b"qcow-test" + bytes(7)).hex()
    password = secret(root, "qcow-password.txt", "qcow-test")
    key = secret(root, "qcow-key.txt", qcow_key)
    for version in (1, 2, 3):
        for encoding, credential in [("deflate", []), ("aes", ["--qcow-password", password]), ("aes", ["--qcow-key", key])]:
            source = qcow_fixtures / f"v{version}-{encoding}.qcow"
            original = source.read_bytes()
            destination = root / f"qcow-{version}-{encoding}-{len(credential) and credential[0][2:]}.raw"
            result, rows = cli.batch([add(source, *credential), export_evidence("export", ALIAS, destination, "--strict")])
            assert all("result" in row for row in rows), rows
            assert rows[0]["result"]["items"][0]["sourceFormat"] == "qcow2", rows
            expected = qcow_plain + bytes(4096) if encoding == "deflate" else bytes(4096) + qcow_plain
            assert destination.read_bytes() == expected and source.read_bytes() == original
            assert "qcow-test" not in result.stdout + result.stderr and qcow_key not in result.stdout + result.stderr
    for credential in [[], ["--qcow-password", password, "--qcow-key", key],
                       ["--qcow-key", secret(root, "short-key.txt", "aa")],
                       ["--qcow-password", secret(root, "wide-password.txt", "非 ASCII")]]:
        _, rows = cli.batch([add(qcow_fixtures / "v2-aes.qcow", *credential)], expected=1)
        assert "error" in rows[0], (credential, rows)
    # A secret is never a literal value on the command line.
    refused = cli.run("evidence", "add", qcow_fixtures / "v2-aes.qcow", "--qcow-password", "qcow-test", expected=1)
    assert "qcow-test" not in refused.stdout + refused.stderr, refused.stderr

    # Multi-member GZIP built with Python's zlib: headers and data cross member
    # boundaries, including empty members.
    tar_stream = io.BytesIO()
    with tarfile.open(fileobj=tar_stream, mode="w") as archive:
        for name, data in [("first.txt", b"first"), ("second.txt", content)]:
            header = tarfile.TarInfo(name)
            header.size = len(data)
            archive.addfile(header, io.BytesIO(data))
    tar_bytes = tar_stream.getvalue()
    gzip_bytes = b"".join(gzip.compress(chunk, mtime=0) for chunk in
                          [b"", tar_bytes[:600], tar_bytes[600:1300], tar_bytes[1300:], b""])
    second = node(ALIAS, path_id("tar:", "second.txt"))
    for suffix in [".tgz", ".tar.gz"]:
        compressed = root / ("multi-member" + suffix)
        compressed.write_bytes(gzip_bytes)
        output = root / ("gzip-read" + suffix + ".txt")
        rows = cli.results([
            add(compressed),
            request("children", "ls", "/ev/tar:"),
            request("read", "cat", second, output=output),
            request("digest", "hash", "/ev/tar:/second.txt", "--algo", "sha256"),
            request("wait", "tasks", "wait"),
            request("close", "evidence", "close", ALIAS),
        ])
        assert opened(rows)["sourceFormat"] == "tar", rows
        assert {entry["name"] for entry in rows["children"]["items"]} == {"first.txt", "second.txt"}, rows
        assert output.read_bytes() == content
        assert rows["digest"]["items"][0]["sha256"] == hashlib.sha256(content).hexdigest()
        assert compressed.read_bytes() == gzip_bytes
    # A bad trailer after a valid tar must not publish a partial tar tree; the
    # extension probe falls back to RAW for inspection.
    corrupt = bytearray(gzip_bytes)
    corrupt[-8] ^= 1
    (root / "corrupt-tail.tgz").write_bytes(corrupt)
    rows = cli.results([add(root / "corrupt-tail.tgz")])
    assert opened(rows)["sourceFormat"] == "raw", rows

    # Split RAW exports include every member, including unaligned cuts.
    split_bytes = bytes(range(256)) * 513
    members = [root / f"split evidence.raw.00{number}" for number in (1, 2, 3)]
    for member, (start, end) in zip(members, [(0, 517), (517, 65539), (65539, len(split_bytes))]):
        member.write_bytes(split_bytes[start:end])
    split_export = root / "split-export.raw"
    rows = cli.results([add(members[0]), export_evidence("export", ALIAS, split_export, "--strict")])
    assert opened(rows)["sourceFormat"] == "raw" and opened(rows)["sizeBytes"] == len(split_bytes), rows
    assert split_export.read_bytes() == split_bytes
    assert b"".join(member.read_bytes() for member in members) == split_bytes
    members[1].unlink()
    _, rows = cli.batch([add(members[0])], expected=1)
    assert "error" in rows[0], rows
    # Forced RAW opens and exports the prefix, without silently changing the
    # read mode of evidence already open.
    forced = root / "forced-prefix.raw"
    _, rows = cli.batch([
        add(members[0], "--raw-read-mode", "force"),
        export_evidence("export", ALIAS, forced, "--strict"),
        request("mode-conflict", "evidence", "add", members[0], "--raw-read-mode", "strict"),
        request("close", "evidence", "close", ALIAS),
    ], expected=1)
    rows = {row["id"]: row for row in rows}
    image = rows["open"]["result"]["items"][0]
    assert image["rawReadMode"] == "force" and image["sizeBytes"] == 517, image
    assert "result" in rows["export"] and "error" in rows["mode-conflict"] and "result" in rows["close"], rows
    assert forced.read_bytes() == split_bytes[:517]

    # EWF: evidence hashes and exports of actual EWF fixtures.
    fixtures = VENDOR / "ewf/tests/data"
    for name, expected_md5 in [("ntfs_sample.E01", "e4e9578a9c8bf6c6b375a80c630196a4"),
                               ("zeros_128s_compressed.Ex01", hashlib.md5(bytes(65536)).hexdigest())]:
        raw_export = root / (name + ".raw")
        rows = cli.results([
            add(fixtures / name),
            request("hash", "evidence", "hash", ALIAS, "--algo", "md5,sha1", "--strict"),
            export_evidence("export", ALIAS, raw_export),
        ])
        raw_bytes = raw_export.read_bytes()
        hashes = rows["hash"]
        assert hashlib.md5(raw_bytes).hexdigest() == expected_md5 == hashes["md5"], hashes
        assert hashes["sha1"] == hashlib.sha1(raw_bytes).hexdigest() and not hashes["readFaultsDetected"], hashes

    # VMDK compressed and split extents export the original virtual bytes.
    fixtures = VENDOR / "vmdk/tests/data"
    for name, expected in [("compressed_stream_opt.vmdk", bytes(range(64)) * 1024),
                           ("tw_sparse_data.vmdk", bytes(range(256)) * 16384),
                           ("mono_flat.vmdk", bytes(1024 * 1024))]:
        raw_export = root / (name + ".raw")
        cli.results([add(fixtures / name), export_evidence("export", ALIAS, raw_export)])
        assert raw_export.read_bytes() == expected, name

    # Full logical streams match hashes captured with an independent libvhdi
    # reader; the CLI itself does not load that library.
    for kind in ("vhd", "vhdx"):
        fixtures = VENDOR / kind / "tests/data"
        oracle = json.loads((fixtures / "LOVELYCARVE_ORACLE.json").read_text(encoding="utf-8"))
        for sample in oracle["images"]:
            raw_export = root / (sample["name"] + ".raw")
            cli.results([add(fixtures / sample["name"]), export_evidence("export", ALIAS, raw_export)])
            assert raw_export.stat().st_size == sample["size"]
            assert hashlib.sha256(raw_export.read_bytes()).hexdigest() == sample["sha256"], sample
    fixtures = VENDOR / "vhdx/tests/data"
    child_raw = root / "fat-differential.vhdx.raw"
    cli.results([
        add(fixtures / "fat-differential.vhdx"),
        export_evidence("export", ALIAS, child_raw, "--strict"),
    ])
    parent = (root / "fat-parent.vhdx.raw").read_bytes()
    child = child_raw.read_bytes()
    assert len(child) == len(parent)
    start, changed, end = (sector * 512 for sector in (133, 134, 136))
    assert child[start:changed] == parent[start:changed]
    assert child[changed:changed + 512] != parent[changed:changed + 512]
    assert child[changed + 6] == 0xFF
    assert child[changed + 512:end] == parent[changed + 512:end]
    print("CLI container tests passed: PHDI, QCOW credentials, multi-member GZIP, split and forced RAW, EWF, VMDK, VHD and VHDX")


def test_encryption(cli, root):
    plaintext = (ENCRYPTED / "plain.img").read_bytes()
    hello = b"LovelyCarve encrypted evidence fixture.\n"
    password = secret(root, "password.txt", "lovelycarve-test")
    fixtures = [
        ("luks1.img", ["--scheme", "luks"], "/ev/unsupported:0"),
        ("luks2.img", ["--scheme", "luks"], "/ev/unsupported:0"),
        ("veracrypt.hc", ["--scheme", "veracrypt", "--pim", "1"], "/ev"),
        # The hidden header opens without asking for it, as VeraCrypt mounts it.
        ("hidden.hc", ["--scheme", "veracrypt", "--pim", "1"], "/ev"),
        ("hidden.hc", ["--scheme", "veracrypt", "--pim", "1", "--header", "hidden", "--prf", "sha512"], "/ev"),
    ]
    for number, (filename, scheme, target) in enumerate(fixtures):
        stem = f"{number}-{filename}"
        # A decrypted container as evidence of its own: it closes with its source.
        exported_plain = root / (stem + ".raw")
        exported_file = root / (stem + ".txt")
        result, rows = cli.batch([
            add(ENCRYPTED / filename),
            request("unlock", "unlock", target, *scheme, "--password", password, "--new-image", "--as", "plain"),
            request("again", "evidence", "add", {"$ref": "unlock#/image/path"}),
            request("files", "ls", "/plain/part:0"),
            request("read", "cat", "/plain/part:0/HELLO.TXT", output=exported_file),
            export_evidence("export", "plain", exported_plain, "--strict"),
            request("close", "evidence", "close", ALIAS),
            request("remaining", "evidence", "ls"),
        ])
        rows = {row["id"]: row.get("result", row) for row in rows}
        assert all("error" not in row for row in rows.values()), rows
        assert rows["unlock"]["path"] == "/plain" and rows["unlock"]["image"]["sourceFormat"] == "decrypted", rows["unlock"]
        if scheme[1] == "veracrypt":
            header = "hidden" if filename.startswith("hidden") else "normal"
            details = rows["unlock"]["veracrypt"]
            assert (details["header"], details["prf"], details["iterations"], details["cipher"]) == (header, "sha512", 16000, "AES"), details
            assert rows["unlock"]["hidden"] == (header == "hidden"), rows["unlock"]
        assert rows["again"]["items"][0]["alias"] == "plain", rows["again"]
        assert "HELLO.TXT" in [item["name"] for item in rows["files"]["items"]], rows["files"]
        assert exported_plain.read_bytes() == plaintext, filename
        assert exported_file.read_bytes() == hello, filename
        assert rows["remaining"]["items"] == [], "decrypted evidence survived closing its source"
        assert "lovelycarve-test" not in result.stdout + result.stderr

        # In place in a case: the source evidence shows the decrypted volume,
        # indexes it, and exports either its source or its plaintext bytes.
        attached_file = root / (stem + ".attached.txt")
        attached_raw = root / (stem + ".source.raw")
        attached_plain = root / (stem + ".attached.raw")
        case = root / (stem + "-case")
        cli.run("case", "new", case, "--name", "attached unlock")
        rows = cli.results([
            add(ENCRYPTED / filename),
            request("unlock", "unlock", target, *scheme, "--password", password),
            request("evidence", "evidence", "ls"),
            request("wait", "tasks", "wait"),
            request("indexes", "index", "ls"),
            request("read", "cat", "/ev/part:0/HELLO.TXT", output=attached_file),
            export_evidence("raw", ALIAS, attached_raw, "--strict"),
            export_evidence("plaintext", ALIAS, attached_plain, "--strict", "--unlocked"),
        ], "-C", case)
        unlocked = rows["unlock"]
        assert unlocked["path"] == "/ev/part:0" and unlocked["volumeKey"] == "part:0", unlocked
        assert unlocked["image"]["id"] == opened(rows)["id"] and unlocked["image"]["unlockedPartitions"] == [{"id": 0, "scheme": scheme[1]}], unlocked
        assert len(rows["evidence"]["items"]) == 1, "unlocking in place created another evidence item"
        assert any(index["address"] == "/ev/part:0" and index["state"] == "ready" for index in rows["indexes"]["items"]), rows["indexes"]
        assert attached_file.read_bytes() == hello
        assert attached_raw.read_bytes() == (ENCRYPTED / filename).read_bytes(), "source export changed after unlock"
        converted = attached_plain.read_bytes()
        assert len(converted) == attached_raw.stat().st_size
        assert converted[:len(plaintext)] == plaintext, filename
        assert converted[len(plaintext):] == bytes(len(converted) - len(plaintext)), filename
        # Deleting a case asks first; without anyone to ask it needs -y.
        refused = cli.run("case", "rm", case, expected=1)
        assert "Aborted." in refused.stderr and (case / "case.db").exists(), refused.stderr
        cli.run("case", "rm", case, "-y")

    # BitLocker unlocks like the containers: in place, then exported with the
    # unlocked partitions in plaintext. Only the retained oracle sectors of the
    # minimized dfvfs projection are checked (not the omitted data).
    fixture = VENDOR / "bitlocker/tests/data/bdetogo-projection.bin.gz"
    encrypted = root / "bitlocker.bin"
    encrypted.write_bytes(gzip.decompress(fixture.read_bytes()))
    decrypted = root / "bitlocker-decrypted.bin"
    result, rows = cli.batch([
        add(encrypted),
        request("locked", "volumes", ALIAS),
        request("unlock", "unlock", "/ev/part:0", "--password", secret(root, "bde.txt", "bde-TEST")),
        request("status", "encryption", "/ev/part:0"),
        request("unlocked", "volumes", ALIAS),
        export_evidence("export", ALIAS, decrypted, "--unlocked"),
    ])
    rows = {row["id"]: row.get("result", row) for row in rows}
    unlocked = rows["unlock"]
    assert all("error" not in row for row in rows.values()), rows
    assert unlocked["scheme"] == "bitlocker" and unlocked["path"] == "/ev/part:0" and unlocked["volumeKey"] == "part:0", unlocked
    assert unlocked["cipher"] == "BitLocker AES-128-CBC + diffuser" and "recoveryPassword" not in unlocked, unlocked
    assert unlocked["image"]["unlockedPartitions"] == [{"id": 0, "scheme": "bitlocker"}], unlocked
    assert rows["status"]["scheme"] == "bitlocker" and rows["status"]["unlocked"] is True, rows["status"]
    assert rows["status"]["metadata"]["encryptionMethod"] == 0x8000, rows["status"]
    for key, state in [("locked", "locked"), ("unlocked", "unlocked")]:
        assert rows[key]["items"][0]["encryption"] == {"scheme": "bitlocker", "state": state}, rows[key]
    assert rows["locked"]["items"][0]["path"] == "/ev/unsupported:0", rows["locked"]
    assert rows["unlocked"]["items"][0]["path"] == "/ev/part:0", rows["unlocked"]
    assert "bde-TEST" not in result.stdout + result.stderr
    assert decrypted.stat().st_size == encrypted.stat().st_size
    with decrypted.open("rb") as stream:
        assert hashlib.sha256(stream.read(512)).hexdigest() == "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3"
        stream.seek(0x8000)
        assert hashlib.sha256(stream.read(4096)).hexdigest() == "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d"

    # Independent cipher projections exercise every credential path.
    bde_data = fixture.parent
    for method, sector_size, credential in [
        (0x8001, 512, []),
        (0x8005, 4096, ["--startup-key", bde_data / "parity-startup.bek"]),
        (0x8004, 512, ["--vmk", secret(root, "vmk.txt", "46" * 32)]),
        (0x8001, 4096, ["--fvek", secret(root, "fvek.txt", bytes(range(1, 33)).hex()),
                        "--tweak", secret(root, "tweak.txt", bytes(range(65, 97)).hex())]),
    ]:
        projection = (bde_data / f"parity-{method:04x}-{sector_size}.bin").read_bytes()
        block_size = struct.unpack_from("<I", projection, 4)[0]
        image = bytearray(0x80000)
        image[:512] = projection[8:520]
        for offset in [0x20000, 0x30000, 0x40000]:
            image[offset:offset + block_size] = projection[520:520 + block_size]
        sectors = projection[520 + block_size:]
        image[0x10000:0x10000 + sector_size] = sectors[:sector_size]
        image[0x12000:0x12000 + sector_size] = sectors[sector_size:]
        encrypted = root / f"bde-{method:04x}-{sector_size}.bin"
        encrypted.write_bytes(image)
        decrypted = root / f"bde-{method:04x}-{sector_size}-plain.bin"
        rows = cli.results([
            add(encrypted),
            request("metadata", "encryption", "/ev/part:0"),
            request("unlock", "unlock", "/ev/part:0", *credential),
            export_evidence("export", ALIAS, decrypted, "--unlocked"),
        ])
        metadata = rows["metadata"]["metadata"]
        assert metadata["encryptionMethod"] == method and metadata["bytesPerSector"] == sector_size, metadata
        assert metadata["protectorTypes"] == [0, 0x0200], metadata
        unlocked = rows["unlock"]
        assert unlocked["scheme"] == "bitlocker" and unlocked["path"] == "/ev/part:0", unlocked
        assert unlocked["encryptionSectorSize"] == sector_size and "recoveryPassword" not in unlocked, unlocked
        plain = bytearray((index * 7 + 19) % 256 for index in range(sector_size))
        plain[3:11] = b"NTFS    "
        decrypted_bytes = decrypted.read_bytes()
        assert len(decrypted_bytes) == len(image) and decrypted_bytes[:sector_size] == plain
        assert decrypted_bytes[0x12000:0x12000 + sector_size] == plain
        assert decrypted_bytes[0x10000:0x10000 + sector_size] == bytes(sector_size)
        assert decrypted_bytes[0x20000:0x30000] == bytes(0x10000)
    print("CLI encryption tests passed: LUKS, VeraCrypt and BitLocker through one unlock, as new evidence and in place; BitLocker password, clear key, startup key, VMK and FVEK")
