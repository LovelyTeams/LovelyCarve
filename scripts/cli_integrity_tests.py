"""Forensic export and read-fault rules on the committed VMDK corpus: exports
never overwrite, a digest over synthesized zeroes is incomplete (exit 2), and
raw bytes over them fail, in one-shot commands, batches and the case backend."""
import hashlib
import json
import shutil
import socket
import struct
import zipfile

from cli_support import REPOSITORY, request

PATH = "/dfvfs_ext2/part:0/passwords.txt"


def faults_of(digest):
    return {key: digest[key] for key in ("readFaultsDetected", "syntheticBytes", "badRegions")}


def test_integrity(cli, root):
    fixture = REPOSITORY / "src-tauri/vendor/vmdk/tests/data/dfvfs_ext2.vmdk"
    evidence = root / "evidence"
    evidence.mkdir()
    (evidence / "hello.txt").write_bytes(b"authentic")
    scope = ("--image", evidence)
    source = "/evidence/folder:"
    # Existing destinations survive file, ZIP and tree exports.
    existing = root / "existing.bin"
    existing.write_bytes(b"do not replace")
    for command in (("export", source + "/hello.txt"), ("export", source, "--zip"), ("export", source)):
        cli.run(*scope, *command, "-o", existing, expected=1)
        assert existing.read_bytes() == b"do not replace"
    tree = root / "existing-tree"
    tree.mkdir()
    cli.run(*scope, "export", source, "-o", tree, expected=1)
    assert list(tree.iterdir()) == []
    archive = root / "new.zip"
    cli.run(*scope, "export", source, "--zip", "-o", archive)
    with zipfile.ZipFile(archive) as exported:
        assert exported.read("hello.txt") == b"authentic"
    new_tree = root / "parent/new-tree"
    cli.run(*scope, "export", source, "-o", new_tree)
    assert (new_tree / "hello.txt").read_bytes() == b"authentic"
    cli.run(*scope, "export", source + "/hello.txt", "--zip", "-o", root / "file.zip", expected=1)
    assert not (root / "file.zip").exists()

    # The truncated copy keeps the name, so it has the same alias and addresses.
    damaged = root / "truncated" / fixture.name
    damaged.parent.mkdir()
    shutil.copyfile(fixture, damaged)
    with damaged.open("r+b") as stream:
        stream.truncate(192 * 1024)
    intact = cli.json("--image", fixture, "hash", PATH, "--algo", "md5")["items"][0]
    assert intact["md5"] == "39cb097008d17660abd0539891a672af", intact
    assert intact["readFaultsDetected"] is False and intact["syntheticBytes"] == 0 and intact["badRegions"] == []
    # A digest over zero-filled bytes is still returned, but exits 2 with a
    # warning on stderr (even with --quiet), never as a plain success.
    damaged_scope = ("--image", damaged)
    partial = cli.run("--json", "-q", *damaged_scope, "hash", PATH, "--algo", "md5", expected=2)
    digest = json.loads(partial.stdout)["items"][0]
    assert digest["md5"] == hashlib.md5(bytes(116)).hexdigest(), digest
    assert digest["readFaultsDetected"] and digest["syntheticBytes"] == 116, digest
    assert sum(region["len"] for region in digest["badRegions"]) == 116
    warning = json.loads(partial.stderr)["warning"]
    assert warning["code"] == "incompleteResult", warning
    assert warning["details"]["fields"] == {"items.readFaultsDetected": True, "items.syntheticBytes": 116}, warning
    framed = cli.run("-q", *damaged_scope, "hash", PATH, "--algo", "md5", expected=2)
    assert "readFaultsDetected" in framed.stderr and "┌" in framed.stderr, framed.stderr
    observed = cli.run("--events", *damaged_scope, "hash", PATH, "--algo", "md5", expected=2)
    events = [json.loads(line) for line in observed.stderr.splitlines() if line.startswith("{")]
    warning = next(event["payload"] for event in events if event["event"] == "cli-warning")
    assert warning["code"] == "incompleteResult" and warning["details"]["fields"]["items.syntheticBytes"] == 116, warning
    # Raw bytes cannot carry the audit: reads over synthesized zeroes fail.
    assert len(cli.run("--image", fixture, "cat", PATH, binary=True).stdout) == 116
    for options in ((), ("--offset", "4", "--length", "8")):
        failure = cli.run(*damaged_scope, "cat", PATH, *options, expected=1)
        assert "readFaultsDetected" in failure.stderr or "合成补零" in failure.stderr, failure.stderr
    failure = cli.run("--json", *damaged_scope, "cat", PATH, expected=1)
    assert failure.stdout == "" and json.loads(failure.stderr)["error"]["code"] == "readFaultsDetected", failure
    saved = root / "failed-read.bin"
    _, rows = cli.batch([
        request("hash", "hash", PATH, "--algo", "md5"),
        request("read", "cat", PATH, output=saved),
        request("range", "cat", PATH, "--offset", "4", "--length", "8"),
        # Earlier faults (and cached bad sectors) must not hide later reports
        # or spoil a zero-length read.
        request("again", "hash", PATH, "--algo", "md5"),
        request("empty", "cat", PATH, "--length", "0"),
    ], *damaged_scope, expected=1)
    rows = {row["id"]: row for row in rows}
    assert rows["hash"]["result"]["items"][0] == rows["again"]["result"]["items"][0] == digest
    for key in ("hash", "again"):
        assert rows[key]["warning"]["code"] == "incompleteResult", rows[key]
        assert rows[key]["warning"]["details"]["fields"]["items.syntheticBytes"] == 116, rows[key]
    assert not saved.exists()
    for key, length in (("read", 116), ("range", 8)):
        row = rows[key]
        assert "error" in row and "result" not in row, row
        assert row["error"]["code"] == "readFaultsDetected", row
        faults = row["error"]["details"]
        assert faults["readFaultsDetected"] and faults["syntheticBytes"] == length, row
        assert sum(region["len"] for region in faults["badRegions"]) == length
    assert rows["empty"]["result"] == {"encoding": "hex", "data": ""}, rows["empty"]

    # The case backend judges results the same way, and a failed stream ends
    # with the fault report without taking the backend down.
    case = root / "case"
    cli.run("case", "new", case, "--name", "Integrity")
    cli.run("-C", case, "evidence", "add", damaged, "--as", "dfvfs_ext2")
    cli.json("-C", case, "case", "open")
    try:
        assert cli.json("-C", case, "hash", PATH, "--algo", "md5", expected=2)["items"][0] == digest
        for options in ((), ("--offset", "4", "--length", "8")):
            cli.run("-C", case, "cat", PATH, *options, expected=1, binary=True)
        record = next(value for value in (json.loads(path.read_text(encoding="utf-8"))
                                          for path in (root / "backends").glob("*.json")))
        command = cli.json("--dry-run", "cat", PATH)["request"]
        work = {"kind": "run", "command": command, "options": {"tz": None, "yes": False}}
        with socket.create_connection(("127.0.0.1", record["port"]), timeout=30) as connection:
            connection.sendall((json.dumps({"token": record["token"], "work": work}) + "\n").encode())
            with connection.makefile("rb") as stream:
                assert json.loads(stream.readline())["kind"] == "stream"
                while True:
                    length = struct.unpack("<I", stream.read(4))[0]
                    if length == 0:
                        break
                    stream.read(length)
                failure = json.loads(stream.readline())
        assert failure["kind"] == "error" and failure["error"]["code"] == "readFaultsDetected", failure
        assert failure["error"]["details"] == faults_of(digest), failure
        assert cli.json("-C", case, "case", "status")["process"]["mode"] == "Backend"
    finally:
        cli.run("-C", case, "case", "close")
    print("CLI integrity tests passed: no-clobber exports, incomplete digests, failed raw reads in one-shot, batch and backend")
