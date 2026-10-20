"""Forensic export and read-fault regressions using the committed VMDK corpus."""
import hashlib
import json
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import zipfile


def test_integrity(executable, base_environment):
    fixture = Path(__file__).resolve().parents[1] / "src-tauri/vendor/vmdk/tests/data/dfvfs_ext2.vmdk"
    with tempfile.TemporaryDirectory(prefix="lovelycarve-integrity-") as directory:
        root = Path(directory)
        environment = dict(base_environment, LOVELYCARVE_SESSION_DIR=str(root / "sessions"))

        def run(*arguments, expected=0, stdin=None):
            result = subprocess.run([str(executable), "--quiet", *map(str, arguments)],
                                    input=stdin, capture_output=True, cwd=root,
                                    env=environment, timeout=120)
            assert result.returncode == expected, (arguments, result.returncode, result.stdout[:2000], result.stderr[-4000:])
            return result

        evidence = root / "evidence"
        evidence.mkdir()
        (evidence / "hello.txt").write_bytes(b"authentic")
        source = f"/{evidence.name}"
        # Existing destinations survive single-file, ZIP and tree exports.
        existing = root / "existing.bin"
        existing.write_bytes(b"do not replace")
        for command in (("files", "export", source + "/hello.txt"),
                        ("files", "directories", "export", source, "--mode", "zip")):
            run("--image", evidence, *command, "--destination", existing, expected=1)
            assert existing.read_bytes() == b"do not replace"
        tree = root / "existing-tree"
        tree.mkdir()
        run("--image", evidence, "files", "directories", "export", source,
            "--destination", tree, expected=1)
        assert list(tree.iterdir()) == []
        archive = root / "new.zip"
        run("--image", evidence, "files", "directories", "export", source,
            "--mode", "zip", "--destination", archive)
        with zipfile.ZipFile(archive) as exported:
            assert exported.read("hello.txt") == b"authentic"
        new_tree = root / "parent/new-tree"
        run("--image", evidence, "files", "directories", "export", source,
            "--destination", new_tree)
        assert (new_tree / "hello.txt").read_bytes() == b"authentic"
        # A directory passed to the single-file exporter must leave no output;
        # the exact same destination is immediately reusable.
        retry = root / "retry.bin"
        run("--image", evidence, "files", "export", source,
            "--destination", retry, expected=1)
        assert not retry.exists()
        run("--image", evidence, "files", "export", source + "/hello.txt", "--destination", retry)
        assert retry.read_bytes() == b"authentic"

        damaged = root / "truncated.vmdk"
        shutil.copyfile(fixture, damaged)
        with damaged.open("r+b") as stream:
            stream.truncate(192 * 1024)
        path = "/分区0/passwords.txt"
        intact = json.loads(run("--image", fixture, "files", "hash", path, "--algos", "md5").stdout)
        assert intact["md5"] == "39cb097008d17660abd0539891a672af"
        assert intact["readFaultsDetected"] is False and intact["syntheticBytes"] == 0
        assert intact["badRegions"] == []
        digest = json.loads(run("--image", damaged, "files", "hash", path, "--algos", "md5").stdout)
        assert digest["md5"] == hashlib.md5(bytes(116)).hexdigest(), digest
        assert digest["readFaultsDetected"] and digest["syntheticBytes"] == 116, digest
        assert sum(region["len"] for region in digest["badRegions"]) == 116
        # stdout remains raw in one-shot mode; its exit status is authoritative,
        # even with --quiet or --text. Healthy reads still succeed.
        assert len(run("--image", fixture, "files", "read", path).stdout) == 116
        for options in ((), ("--text",), ("--offset", "4", "--length", "8")):
            run("--image", damaged, "files", "read", path, *options, expected=1)
        saved = root / "failed-read.bin"
        requests = [
            {"id": "hash", "command": "files hash", "args": {"path": path, "algos": ["md5"]}},
            {"id": "read", "command": "files read", "args": {"path": path}, "output": str(saved)},
            {"id": "range", "command": "files read", "args": {"path": path, "offset": 4, "length": 8}},
            # Prior faults (and cached bad sectors) must not suppress subsequent
            # operation reports or contaminate a zero-length read.
            {"id": "again", "command": "files hash", "args": {"path": path, "algos": ["md5"]}},
            {"id": "empty", "command": "files read", "args": {"path": path, "length": 0}},
        ]
        response = run("--image", damaged, "session", expected=1,
                       stdin="\n".join(map(json.dumps, requests)).encode())
        rows = {row["id"]: row for row in map(json.loads, response.stdout.splitlines())}
        assert rows["hash"]["result"] == rows["again"]["result"] == digest
        assert not saved.exists()
        for request_id, length in (("read", 116), ("range", 8)):
            row = rows[request_id]
            assert "error" in row and "result" not in row, row
            assert row["error"]["code"] == "readFaultsDetected", row
            faults = row["error"]["details"]
            assert faults["readFaultsDetected"] and faults["syntheticBytes"] == length, row
            assert sum(region["len"] for region in faults["badRegions"]) == length
        assert "error" not in rows["empty"]
        # Resident byte streams route faults through the final error frame.
        run("--image", damaged, "sessions", "open", "damaged")
        try:
            for options in ((), ("--offset", "4", "--length", "8")):
                result = run("--session", "damaged", "files", "read", path, *options, expected=1)
                assert result.stdout == b"", result.stdout
            descriptor = json.loads((root / "sessions/session-damaged.json").read_text())
            with socket.create_connection(("127.0.0.1", descriptor["port"]), timeout=15) as connection:
                connection.sendall((json.dumps({"token": descriptor["token"], "command": "files read",
                                               "args": {"path": path}}) + "\n").encode())
                with connection.makefile("rb") as stream:
                    assert json.loads(stream.readline())["kind"] == "stream"
                    assert struct.unpack("<I", stream.read(4))[0] == 0
                    failure = json.loads(stream.readline())
                    assert failure["kind"] == "error" and failure["error"]["code"] == "readFaultsDetected", failure
                    assert failure["error"]["details"] == digest_faults(digest)
            # A failed stream does not tear down the resident session.
            run("--session", "damaged", "context", "get")
        finally:
            run("sessions", "close", "damaged")


def digest_faults(digest):
    return {key: digest[key] for key in ("readFaultsDetected", "syntheticBytes", "badRegions")}
