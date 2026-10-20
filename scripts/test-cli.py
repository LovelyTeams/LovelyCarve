"""End-to-end tests against the standalone CLI process (no desktop runtime, window or web server)."""
import gzip
import hashlib
import ctypes
from ctypes import wintypes
import json
import io
import os
import queue
from pathlib import Path
import struct
import subprocess
import signal
import sys
import tempfile
import tarfile
import threading
import zlib

from cli_resident_tests import test_resident_sessions
from cli_observability_tests import test_observability
from cli_timezone_tests import test_timezones
from cli_integrity_tests import test_integrity
from cli_navigation_tests import test_navigation


def documents(text):
    """The JSON documents a shell script printed, with raw output between them as stripped strings."""
    decoder = json.JSONDecoder()
    items, raw, position = [], "", 0
    while position < len(text):
        if text[position] in "{[":
            try:
                value, end = decoder.raw_decode(text, position)
            except json.JSONDecodeError:
                pass
            else:
                if raw.strip():
                    items.append(raw.strip())
                raw = ""
                items.append(value)
                position = end
                continue
        raw += text[position]
        position += 1
    if raw.strip():
        items.append(raw.strip())
    return items


def main():
    executable = Path(sys.argv[1]).resolve()
    # Scope variables of the caller's shell must not leak into the cases below.
    environment = {key: value for key, value in os.environ.items()
                   if key not in {"LOVELYCARVE_IMAGE", "LOVELYCARVE_CASE", "LOVELYCARVE_SESSION"}}
    with tempfile.TemporaryDirectory(prefix="lovelycarve-cli-test-") as directory:
        root = Path(directory)
        evidence = root / "evidence with spaces"
        volume_name = "evidence_with_spaces"
        evidence.mkdir()
        content = b"hello forensic world\npassword test\n"
        (evidence / "hello.txt").write_bytes(content)
        copied = root / "copy.bin"
        node = "folder:" + b"hello.txt".hex()

        def run(arguments, stdin=None, expected=0):
            process = subprocess.run([str(executable), *arguments], input=stdin,
                                     text=True, encoding="utf-8", capture_output=True,
                                     timeout=120, cwd=root, env=environment)
            assert process.returncode == expected, (arguments, process.returncode, process.stdout, process.stderr)
            return process

        def run_bytes(arguments, expected=0):
            process = subprocess.run([str(executable), *arguments], capture_output=True,
                                     timeout=120, cwd=root, env=environment)
            assert process.returncode == expected, (arguments, process.returncode, process.stdout, process.stderr)
            return process

        def session(requests, expected=0, arguments=()):
            result = run([*arguments, "session"], "\n".join(json.dumps(request) for request in requests), expected)
            return result, [json.loads(line) for line in result.stdout.splitlines()]

        # Discovery needs no evidence: system facts, request schemas, search and completion.
        system = json.loads(run(["system", "get"]).stdout)
        assert system["standalone"], "the CLI must be built without the desktop runtime"
        assert system["session"]["maxJobs"] == 64 and system["session"]["protocolVersion"] == 3
        schemas = json.loads(run(["schema", "--list"]).stdout)
        assert system["commands"] >= len(schemas) >= 140, (system["commands"], len(schemas))
        assert {"command": "lovelycarve-cli files media create", "ipcCommand": "fs_media_temp",
                "operationId": "files.media.create"} in schemas
        schema = json.loads(run(["schema", "images", "open"]).stdout)
        assert schema["ipcCommand"] == "image_load" and schema["operationId"] == "images.open"
        assert next(field for field in schema["requestBodyFields"] if field["name"] == "qcowPassword")["sensitive"]
        for command in (["files", "list"], ["files", "tree", "list"], ["files", "get"], ["files", "read"], ["files", "hash"]):
            contract = json.loads(run(["schema", *command]).stdout)
            fields = {field["name"] for field in contract["requestBodyFields"]}
            assert "path" in fields and "imageId" in fields, contract
            assert contract["targetAlternatives"] == ["path", "parentId" if command == ["files", "list"] else "nodeId"]
        run(["files", "children", "list"], expected=1)
        for body in ({"offset": "1"}, {"offset": -1}, {"typo": True}):
            run(["files", "read", "--body", json.dumps({"path": "/x", **body}), "--dry-run"], expected=1)
        found = json.loads(run(["cli", "search", "export a file"]).stdout)
        assert found[0]["command"] == "lovelycarve-cli files export", found
        candidates = [line.split("\t")[0] for line in run(["complete", "--", "files", "s"]).stdout.splitlines()]
        assert {"search", "sqlite", "strings"} <= set(candidates), candidates
        run(["--help"])
        run(["files", "read", "--help"])
        assert "schema files hash" in run(["files", "hash", "--help"]).stdout
        assert "images open" in run(["image_load"], expected=1).stderr
        run(["--unknown"], expected=1)
        run(["session", "--jobs", "0"], expected=1)
        run(["session", "--jobs", "65"], expected=1)
        # --dry-run shows the backend request without touching evidence or printing secrets.
        missing = root / "missing.E01"
        preview = json.loads(run(["images", "open", str(missing), "--qcow-password", "hunter2", "--dry-run"]).stdout)
        assert preview["ipcCommand"] == "image_load" and preview["body"]["path"] == str(missing), preview
        assert preview["body"]["qcowPassword"] == "<redacted>" and not missing.exists()
        # Destructive commands need --force where nobody can be asked.
        refused = run(["cases", "delete", str(root / "no such case")], expected=1)
        assert "Aborted." in refused.stderr, refused.stderr
        # Results keep the order the backend wrote their fields in.
        assert run(["schema", "images", "open"]).stdout.startswith('{\n  "operationId": "images.open",')
        # A required subject may come from --body; arrays take several values after one option.
        preview = json.loads(run(["images", "open", "--body", json.dumps({"path": str(missing)}), "--dry-run"]).stdout)
        assert preview["body"]["path"] == str(missing), preview
        preview = json.loads(run(["files", "hash", "/x", "--algos", "md5", "sha256", "--dry-run"]).stdout)
        assert preview["body"]["algos"] == ["md5", "sha256"], preview
        assert json.loads(run(["complete", "bash", "--dry-run"]).stdout)["executed"] is False
        # Hidden MCP tool definitions: every command, none of them listed in help or completion.
        tools = json.loads(run(["tools"]).stdout)
        assert tools["version"] == "2.0" and len(tools["tools"]) == system["commands"], len(tools["tools"])
        assert next(tool for tool in tools["tools"]
                    if tool["name"] == "lovelycarve_cli_images_open")["inputSchema"]["required"] == ["path"]
        assert "lovelycarve-cli tools" not in run(["--help"]).stdout
        assert "tools" not in run(["complete", "--", "t"]).stdout.split()
        assert "tools" not in run(["toolz"], expected=1).stderr.split("未知命令：toolz")[1]
        # The bundled documentation is searchable and readable offline.
        hits = json.loads(run(["docs", "search", "JSONL 会话"]).stdout)
        assert hits and {"title", "section", "description", "page"} == set(hits[0]), hits
        assert {"page": "CLI.md", "title": "LovelyCarve CLI"} in json.loads(run(["docs", "list"]).stdout)
        assert run(["docs", "get", "CLI.md", "--section", "JSONL 会话"]).stdout.startswith("## JSONL 会话")
        run(["docs", "get", "missing.md"], expected=1)
        _, rows = session([
            {"id": "schema", "command": "schema", "args": {"command": ["files", "hash"]}},
            {"id": "docs", "command": "docs get", "args": {"page": "CLI.md", "section": "JSONL 会话"}},
            {"id": "tools", "command": "tools"},
        ])
        assert rows[0]["result"]["ipcCommand"] == "fs_digest", rows[0]
        assert rows[1]["result"]["markdown"].startswith("## JSONL 会话"), rows[1]
        assert len(rows[2]["result"]["tools"]) == system["commands"]
        # .env.local overrides .env, and a scope taken from either is announced.
        (root / ".env").write_text(f"LOVELYCARVE_IMAGE={root / 'missing.E01'}\n", encoding="utf-8")
        (root / ".env.local").write_text(f"LOVELYCARVE_IMAGE={evidence}\n", encoding="utf-8")
        try:
            scoped = run(["images", "get"])
            assert Path(json.loads(scoped.stdout)["path"]).name == evidence.name, scoped.stdout
            assert "来自 .env.local 中的 LOVELYCARVE_IMAGE" in scoped.stderr, scoped.stderr
            assert "使用检材" not in run(["--quiet", "images", "get"]).stderr
        finally:
            (root / ".env").unlink()
            (root / ".env.local").unlink()
        missing_scope = run(["images", "get"], expected=1).stderr
        assert "未指定检材" in missing_scope and "LOVELYCARVE_IMAGE" in missing_scope, missing_scope

        # Selecting an already loaded image must change the persistent context,
        # while supplying imageId to an ordinary query must not change it.
        second_evidence = root / "second evidence"
        second_evidence.mkdir()
        _, selections = session([
            {"id": "first", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "cd-first", "command": "context set", "args": {"dir": volume_name}},
            {"id": "second", "command": "images open", "args": {"path": str(second_evidence)}},
            {"id": "set-first", "command": "context set", "args": {"imageId": {"$ref": "first#/id"}}},
            {"id": "use-second", "command": "context use", "args": {"imageId": {"$ref": "second#/id"}}},
            {"id": "query-first", "command": "files list", "args": {"imageId": {"$ref": "first#/id"}}},
            {"id": "current", "command": "context get"},
            {"id": "open-id", "command": "images open", "args": {"path": {"$ref": "first#/id"}}},
            {"id": "expired", "command": "images open", "args": {"path": "assembled://missing-array"}},
            {"id": "unknown-id", "command": "images open", "args": {"path": "d462a771-85d2-410b-ad72-016b0ced24a1"}},
        ], expected=1)
        selected = {row["id"]: row for row in selections}
        assert selected["set-first"]["result"]["image"]["id"] == selected["first"]["result"]["id"]
        assert selected["use-second"]["result"]["image"]["id"] == selected["second"]["result"]["id"]
        assert selected["current"]["result"]["image"]["id"] == selected["second"]["result"]["id"]
        assert selected["use-second"]["result"]["dir"] == "/"
        assert selected["open-id"]["result"]["id"] == selected["first"]["result"]["id"]
        assert all(selected[key]["error"]["code"] == "notFound" for key in ("expired", "unknown-id"))
        run(["context", "use", "d462a771-85d2-410b-ad72-016b0ced24a1", "--dry-run"])

        concurrent_requests = [
            {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "cd", "command": "context set", "args": {"dir": volume_name}},
            {"id": "read", "command": "files read", "args": {"nodeId": node, "offset": 0, "length": 8}},
            {"id": "digest", "command": "files hash", "args": {"nodeId": node, "algos": ["sha256"]}},
            {"id": "stat", "command": "files get", "args": {"path": "hello.txt"}},
            {"id": "dependent", "command": "files read", "args": {"nodeId": {"$ref": "stat#/nodeId"}, "offset": 0, "length": 8}},
            {"id": "bad", "command": "images list", "args": {"typo": True}},
            {"id": "invalid-args", "command": "images list", "args": []},
            {"id": "legacy", "command": "image_load", "args": {"path": str(evidence)}},
            {"id": "wait", "command": "tasks wait"},
            {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
            {"id": "remaining", "command": "images list"},
        ]
        _, serial = session(concurrent_requests, expected=1)
        result = run(["session", "--jobs", "4"], "\n".join(json.dumps(request) for request in concurrent_requests), expected=1)
        concurrent_rows = {row["id"]: row for row in map(json.loads, result.stdout.splitlines())}
        assert len(concurrent_rows) == len(concurrent_requests), result.stdout
        assert concurrent_rows["read"]["result"] == {"encoding": "hex", "data": content[:8].hex()}
        assert concurrent_rows["dependent"]["result"] == concurrent_rows["read"]["result"]
        assert concurrent_rows["digest"]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert concurrent_rows["cd"]["result"]["dir"] == "/" + volume_name
        assert concurrent_rows["remaining"]["result"] == []
        errors = {"bad", "invalid-args", "legacy"}
        assert all("result" in row for key, row in concurrent_rows.items() if key not in errors), concurrent_rows
        assert all("code" in concurrent_rows[key]["error"] and "message" in concurrent_rows[key]["error"] for key in errors)
        assert "images open" in concurrent_rows["legacy"]["error"]["message"]
        # The serial run of the same requests returns the same results, in request order.
        assert [row["id"] for row in serial] == [request["id"] for request in concurrent_requests]
        serial_rows = {row["id"]: row for row in serial}
        assert all(serial_rows[key] == concurrent_rows[key] for key in ["read", "digest", "dependent", "remaining"])

        # AI clients keep stdin open and wait for a response before sending more.
        process = subprocess.Popen([str(executable), "--image", str(evidence), "session", "-j", "4"],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, encoding="utf-8", cwd=root, env=environment)
        response_queue = queue.Queue()
        reader_thread = threading.Thread(target=lambda: response_queue.put(process.stdout.readline()), daemon=True)
        try:
            process.stdin.write(json.dumps({"id": "live", "command": "files read", "args": {"nodeId": node, "offset": 0, "length": 8}}) + "\n")
            process.stdin.flush()
            reader_thread.start()
            live = json.loads(response_queue.get(timeout=30))
            assert live == {"id": "live", "result": concurrent_rows["read"]["result"]}, live
            assert process.poll() is None, "session exited before stdin EOF"
            process.stdin.close()
            process.wait(timeout=30)
            assert process.returncode == 0, process.stderr.read()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            for stream in [process.stdin, process.stdout, process.stderr]:
                stream.close()
            reader_thread.join(timeout=5)

        # Optional private evidence stays outside the repository. Compare actual
        # filesystem queries and bounded bytes between serial and parallel runs.
        real_image = os.environ.get("LOVELYCARVE_CLI_TEST_IMAGE")
        if real_image:
            def real_queries(jobs):
                # Keep one evidence session per mode, avoiding repeated E01 opens.
                with tempfile.TemporaryFile(mode="w+t", encoding="utf-8") as diagnostics:
                    process = subprocess.Popen([str(executable), "--image", real_image, "session", "--jobs", str(jobs)],
                                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics,
                                               text=True, encoding="utf-8", cwd=root, env=environment)
                    replies = queue.Queue()
                    def collect():
                        for line in process.stdout:
                            replies.put(line)
                        replies.put(None)
                    thread = threading.Thread(target=collect, daemon=True)
                    thread.start()
                    def batch(requests):
                        for request in requests:
                            process.stdin.write(json.dumps(request) + "\n")
                        process.stdin.flush()
                        pending = {request["id"] for request in requests}
                        results = {}
                        while pending:
                            line = replies.get(timeout=120)
                            assert line is not None, "real evidence session ended early"
                            row = json.loads(line)
                            assert row["id"] in pending and "result" in row, row
                            pending.remove(row["id"])
                            results[row["id"]] = row["result"]
                        return results
                    def children(parents, prefix):
                        return batch([{"id": f"{prefix}-{index}", "command": "files list", "args": {"parentId": parent}}
                                      for index, parent in enumerate(parents)])
                    try:
                        roots = children([""], "roots")["roots-0"]
                        directories = [entry["id"] for entry in roots["items"] if entry["kind"] == "dir"][:4]
                        assert directories, "real evidence has no browsable volumes"
                        directory_rows = children(directories, "dir")
                        def files_in(pages):
                            return sorted((entry for page in pages.values() for entry in page["items"]
                                           if entry["kind"] == "file" and entry["size"] > 0 and not entry["deleted"]),
                                          key=lambda entry: entry["id"])[:4]
                        files = files_in(directory_rows)
                        # Files may live below a filesystem's top-level directories.
                        nested = sorted(entry["id"] for page in directory_rows.values() for entry in page["items"]
                                        if entry["kind"] == "dir" and not entry["deleted"])[:8]
                        if not files and nested:
                            nested_rows = children(nested, "nested")
                            directory_rows.update(nested_rows)
                            files = files_in(nested_rows)
                        assert files, "real evidence has no file for bounded read verification"
                        byte_rows = batch([{"id": f"bytes-{index}", "command": "files read", "args": {
                            "nodeId": entry["id"], "offset": 0, "length": min(4096, entry["size"])}}
                            for index, entry in enumerate(files)])
                        process.stdin.close()
                        process.wait(timeout=120)
                        assert process.returncode == 0
                        return directory_rows, byte_rows
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()
                        thread.join(timeout=5)
                        process.stdin.close()
                        process.stdout.close()
            serial_directories, serial_bytes = real_queries(1)
            parallel_directories, parallel_bytes = real_queries(4)
            assert parallel_directories == serial_directories, "real evidence directory queries differ under concurrency"
            assert parallel_bytes == serial_bytes, "real evidence range reads differ under concurrency"
            print(f"Real evidence concurrency passed: {len(serial_directories)} directories, {len(serial_bytes)} bounded file reads", flush=True)

        # Parallels: an independently built split plain/sparse disk is exposed
        # as guest bytes by the same images open/export/close command surface.
        phdi_bundle = root / "Parallels evidence.hdd"
        phdi_bundle.mkdir()
        phdi_plain = b"plain guest sector".ljust(512, b"\0")
        (phdi_bundle / "plain.hds").write_bytes(phdi_plain)
        phdi_sparse = bytearray(1024)
        phdi_sparse[:16] = b"WithoutFreeSpace"
        for offset, value in [(16, 2), (28, 1), (32, 2), (48, 1), (64, 1)]:
            struct.pack_into("<I", phdi_sparse, offset, value)
        struct.pack_into("<Q", phdi_sparse, 36, 2)
        phdi_sparse[512:] = b"sparse guest sector".ljust(512, b"\0")
        (phdi_bundle / "sparse.hds").write_bytes(phdi_sparse)
        phdi_descriptor = """<?xml version="1.0" encoding="UTF-8"?>
<Parallels_disk_image Version="1.0"><Disk_Parameters><Disk_size>3</Disk_size></Disk_Parameters>
<StorageData><Storage><Start>0</Start><End>1</End><Blocksize>1</Blocksize><Image>
<GUID>{11111111-1111-1111-1111-111111111111}</GUID><Type>Plain</Type><File>plain.hds</File>
</Image></Storage><Storage><Start>1</Start><End>3</End><Blocksize>1</Blocksize><Image>
<GUID>{11111111-1111-1111-1111-111111111111}</GUID><Type>Compressed</Type><File>sparse.hds</File>
</Image></Storage></StorageData></Parallels_disk_image>"""
        (phdi_bundle / "DiskDescriptor.xml").write_text(phdi_descriptor, encoding="utf-8")
        for suffix, source_path in [("bundle", phdi_bundle), ("descriptor", phdi_bundle / "DiskDescriptor.xml")]:
            destination = root / f"phdi-{suffix}.raw"
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(source_path)}},
                {"id": "export", "command": "images export", "args": {
                    "destination": str(destination), "taskId": f"phdi-{suffix}", "readFaultPolicy": "strict"}},
                {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
            ])
            assert all("result" in row for row in rows), rows
            assert rows[0]["result"]["sourceFormat"] == "phdi", rows
            assert destination.read_bytes() == phdi_plain + bytes(phdi_sparse[512:]) + bytes(512)
            assert (phdi_bundle / "plain.hds").read_bytes() == phdi_plain
            assert (phdi_bundle / "sparse.hds").read_bytes() == phdi_sparse

        # A recognized corrupt descriptor must fail, rather than become RAW.
        (phdi_bundle / "DiskDescriptor.xml").write_text("broken", encoding="utf-8")
        result, rows = session([{"id": "broken-phdi", "command": "images open", "args": {
            "path": str(phdi_bundle / "DiskDescriptor.xml")}}], expected=1)
        assert "error" in rows[0], result

        # The generated images open dispatch must accept transient QCOW
        # credentials, export logical bytes, and preserve the source image.
        qcow_fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor/qcow2/tests/data"
        qcow_plain = (qcow_fixtures / "plain.bin").read_bytes()
        qcow_key_hex = (b"qcow-test" + bytes(7)).hex()
        for version in (1, 2, 3):
            for encoding, credential in [("deflate", {}), ("aes", {"qcowPassword": "qcow-test"}),
                                         ("aes", {"qcowKeyHex": qcow_key_hex})]:
                source_path = qcow_fixtures / f"v{version}-{encoding}.qcow"
                original = source_path.read_bytes()
                destination = root / f"qcow-{version}-{encoding}-{len(credential)}-{list(credential)}.raw"
                result, rows = session([
                    {"id": "open", "command": "images open", "args": {"path": str(source_path), **credential}},
                    {"id": "export", "command": "images export", "args": {
                        "destination": str(destination), "taskId": "qcow-export", "readFaultPolicy": "strict"}},
                    {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
                ])
                assert all("result" in row for row in rows), rows
                assert rows[0]["result"]["sourceFormat"] == "qcow2", rows
                expected = qcow_plain + bytes(4096) if encoding == "deflate" else bytes(4096) + qcow_plain
                assert destination.read_bytes() == expected
                assert source_path.read_bytes() == original
                assert "qcow-test" not in result.stdout + result.stderr
                assert qcow_key_hex not in result.stdout + result.stderr
        for credential in [{}, {"qcowPassword": "qcow-test", "qcowKeyHex": qcow_key_hex},
                           {"qcowKeyHex": "aa"}, {"qcowPassword": "非 ASCII"}]:
            result, rows = session([{"id": "invalid", "command": "images open", "args": {
                "path": str(qcow_fixtures / "v2-aes.qcow"), **credential}}], expected=1)
            assert "error" in rows[0], result

        # Independent Python/zlib multi-member GZIP through the actual evidence
        # commands: headers/data may cross member boundaries, including empties.
        tar_stream = io.BytesIO()
        with tarfile.open(fileobj=tar_stream, mode="w") as archive:
            for name, data in [("first.txt", b"first"), ("second.txt", content)]:
                header = tarfile.TarInfo(name)
                header.size = len(data)
                archive.addfile(header, io.BytesIO(data))
        tar_bytes = tar_stream.getvalue()
        gzip_bytes = b"".join(gzip.compress(chunk, mtime=0) for chunk in
                              [b"", tar_bytes[:600], tar_bytes[600:1300], tar_bytes[1300:], b""])
        for suffix in [".tgz", ".tar.gz"]:
            compressed_tar = root / ("multi-member" + suffix)
            compressed_tar.write_bytes(gzip_bytes)
            output = root / ("gzip-read" + suffix + ".txt")
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(compressed_tar)}},
                {"id": "children", "command": "files list", "args": {"parentId": "tar:"}},
                {"id": "read", "command": "files read", "args": {"nodeId": "tar:" + b"second.txt".hex()}, "output": str(output)},
                {"id": "digest", "command": "files hash", "args": {"nodeId": "tar:" + b"second.txt".hex(), "algos": ["sha256"]}},
                {"id": "wait", "command": "tasks wait"},
                {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
            ])
            rows = {row["id"]: row for row in rows}
            assert all("result" in row for row in rows.values()), rows
            assert rows["open"]["result"]["sourceFormat"] == "tar", rows
            assert {entry["name"] for entry in rows["children"]["result"]["items"]} == {"first.txt", "second.txt"}, rows
            assert output.read_bytes() == content
            assert rows["digest"]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
            assert compressed_tar.read_bytes() == gzip_bytes

        # A bad trailer after a valid tar must not publish a partial tar tree.
        # Preserve the extension probe's existing RAW fallback for inspection.
        corrupt_gzip = bytearray(gzip_bytes)
        corrupt_gzip[-8] ^= 1
        corrupt_tar = root / "corrupt-tail.tgz"
        corrupt_tar.write_bytes(corrupt_gzip)
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(corrupt_tar)}},
            {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
        ])
        assert all("result" in row for row in rows), rows
        assert rows[0]["result"]["sourceFormat"] == "raw", rows

        encrypted_fixtures = Path(__file__).resolve().parents[1] / "src-tauri/tests/data/encrypted"
        plaintext = (encrypted_fixtures / "plain.img").read_bytes()
        for filename, scheme, hidden in [
            ("luks1.img", "luks", False), ("luks2.img", "luks", False),
            ("veracrypt.hc", "veracrypt", False), ("hidden.hc", "veracrypt", True),
        ]:
            exported_plain = root / (filename + ".raw")
            exported_file = root / (filename + ".txt")
            options = {"scheme": scheme, "password": "lovelycarve-test", "hidden": hidden,
                       "pim": 1 if scheme == "veracrypt" else 0}
            if filename == "luks1.img":
                options["partitionId"] = 0
            encrypted_result, encrypted_rows = session([
                {"id": "source", "command": "images open", "args": {"path": str(encrypted_fixtures / filename)}},
                {"id": "unlock", "command": "encrypted-volumes unlock", "args": {"options": options}},
                {"id": "plain", "command": "images open", "args": {"path": {"$ref": "unlock#/image/path"}}},
                {"id": "files", "command": "files list", "args": {"parentId": "part:0"}},
                {"id": "read", "command": "files read", "args": {"nodeId": "fat:part:0:" + b"/HELLO.TXT".hex()}, "output": str(exported_file)},
                {"id": "export", "command": "images export", "args": {"destination": str(exported_plain), "taskId": "encrypted-export", "readFaultPolicy": "strict"}},
                {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "source#/id"}}},
                {"id": "remaining", "command": "images list"},
            ])
            assert all("result" in row for row in encrypted_rows), encrypted_rows
            assert "HELLO.TXT" in [item["name"] for item in encrypted_rows[3]["result"]["items"]], encrypted_rows[3]
            assert exported_plain.read_bytes() == plaintext, filename
            assert exported_file.read_bytes() == b"LovelyCarve encrypted evidence fixture.\n", filename
            assert encrypted_rows[1]["result"]["image"]["sourceFormat"] == "decrypted"
            assert encrypted_rows[-1]["result"] == [], "decrypted child survived source unload"
            assert "lovelycarve-test" not in encrypted_result.stdout + encrypted_result.stderr

            attached_file = root / (filename + ".attached.txt")
            attached_raw = root / (filename + ".source.raw")
            attached_plain = root / (filename + ".attached.raw")
            attached_case = root / (filename + "-case")
            attached_result, attached_rows = session([
                {"id": "case", "command": "cases create", "args": {"dir": str(attached_case), "name": "attached unlock"}},
                {"id": "source", "command": "images open", "args": {"path": str(encrypted_fixtures / filename)}},
                {"id": "unlock", "command": "encrypted-volumes unlock", "args": {"options": {**options, "target": "sourcePartition"}}},
                {"id": "images", "command": "images list"},
                {"id": "wait", "command": "tasks wait"},
                {"id": "indexes", "command": "indexes list"},
                {"id": "read", "command": "files read", "args": {"nodeId": "fat:part:0:" + b"/HELLO.TXT".hex()}, "output": str(attached_file)},
                {"id": "raw", "command": "images export", "args": {"destination": str(attached_raw), "taskId": "attached-raw", "readFaultPolicy": "strict"}},
                {"id": "plaintext", "command": "images export", "args": {"destination": str(attached_plain), "taskId": "attached-plaintext", "readFaultPolicy": "strict", "mode": "unlockedPartitions"}},
                {"id": "close", "command": "cases close"},
                {"id": "unconfirmed", "command": "cases delete", "args": {"dir": str(attached_case)}},
                {"id": "remove", "command": "cases delete", "args": {"dir": str(attached_case)}, "force": True},
            ], expected=1)
            attached_rows = {row["id"]: row for row in attached_rows}
            assert attached_rows["unconfirmed"]["error"]["code"] == "aborted", attached_rows["unconfirmed"]
            assert all("result" in row for key, row in attached_rows.items() if key != "unconfirmed"), attached_rows
            source_id = attached_rows["source"]["result"]["id"]
            attached = attached_rows["unlock"]["result"]
            assert attached["image"]["id"] == source_id and attached["volumeKey"] == "part:0"
            assert attached["image"]["unlockedPartitions"] == [0]
            assert len(attached_rows["images"]["result"]) == 1, "in-place unlock created another image"
            assert any(index["imageId"] == source_id and index["volumeKey"] == "part:0" and index["indexed"]
                       for index in attached_rows["indexes"]["result"]), (filename, attached_rows["indexes"], attached_result.stderr)
            assert attached_file.read_bytes() == b"LovelyCarve encrypted evidence fixture.\n"
            assert attached_raw.read_bytes() == (encrypted_fixtures / filename).read_bytes(), "source export changed after unlock"
            converted = attached_plain.read_bytes()
            assert len(converted) == attached_raw.stat().st_size
            assert converted[:len(plaintext)] == plaintext, filename
            assert converted[len(plaintext):] == bytes(len(converted) - len(plaintext)), filename

        result, rows = session([
            {"id": "image", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "roots", "command": "files list", "args": {"parentId": ""}},
            {"id": "read", "command": "files read", "args": {"imageId": {"$ref": "image#/id"}, "nodeId": node}, "output": str(copied)},
            {"id": "digest", "command": "files hash", "args": {"nodeId": node, "algos": ["sha256"]}},
            {"id": "preview", "command": "files media create", "args": {"nodeId": node, "ext": "txt", "taskId": "preview-1"}},
            {"id": "wait", "command": "tasks wait"},
        ])
        assert len(rows) == 6, result.stdout
        assert all("result" in row for row in rows), rows
        assert copied.read_bytes() == content
        assert rows[3]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert not Path(rows[4]["result"]).exists(), "preview lease leaked after process exit"

        # A shell script runs one command per line and stops at the first failure.
        exported = root / "export.txt"
        script = f'''images open "{evidence}"
cd "{volume_name}"
files list
files read hello.txt --length 5
files hash hello.txt
files export hello.txt --destination "{exported}"
'''
        outputs = documents(run(["shell"], script).stdout)
        assert exported.read_bytes() == content
        assert outputs[0]["id"] and outputs[0]["path"] == str(evidence), outputs
        assert "hello.txt" in [item["name"] for item in outputs[1]["items"]], outputs
        assert outputs[2] == "hello", outputs
        assert outputs[3]["sha256"] == hashlib.sha256(content).hexdigest(), outputs
        refused = run(["shell"], script, expected=1)
        assert "已存在" in refused.stderr, refused.stderr
        assert exported.read_bytes() == content

        # Path-based browsing, ranged reads, strings and content identification.
        # Content recognition must ignore the misleading filename extension.
        python_content = b"#!/usr/bin/env python3\nimport sys\nprint('LovelyCarve forensic CLI')\n" * 20
        (evidence / "signature.bin").write_bytes(python_content)
        unicode_text = "中文取证字符串"
        (evidence / "unicode.bin").write_bytes(b"\0" + unicode_text.encode("utf-8") + b"\0")
        (evidence / "wide.bin").write_bytes(("\0" + unicode_text + "\0").encode("utf-16le"))
        (evidence / "latin1.bin").write_bytes(b"caf\xe9")
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "root", "command": "files list", "args": {"path": "/"}},
            {"id": "wait", "command": "tasks wait"},
            {"id": "cd", "command": "context set", "args": {"dir": "/" + volume_name}},
            {"id": "pwd", "command": "context get"},
            {"id": "page", "command": "files list", "args": {"path": ".", "pageSize": 2}},
            {"id": "page-by-id", "command": "files list", "args": {"parentId": "folder:", "pageSize": 2}},
            {"id": "next", "command": "files list", "args": {"cursor": {"$ref": "page#/nextCursor"}, "pageSize": 1000}},
            {"id": "all", "command": "files list", "args": {"pageSize": 1000}},
            {"id": "tree", "command": "files tree list", "args": {"depth": 2}},
            {"id": "stat", "command": "files get", "args": {"path": "hello.txt"}},
            {"id": "hex", "command": "files read", "args": {"path": "hello.txt", "offset": 6, "length": 8}},
            {"id": "strings", "command": "files strings list", "args": {"path": "hello.txt"}},
            {"id": "utf8", "command": "files strings list", "args": {"path": "unicode.bin", "encoding": "utf-8"}},
            {"id": "utf16", "command": "files strings list", "args": {"path": "wide.bin", "encoding": "utf-16le"}},
            {"id": "limited", "command": "files strings list", "args": {"path": "hello.txt", "limit": 1}},
            {"id": "file", "command": "files identify", "args": {"path": "signature.bin"}},
        ])
        browsed = {row["id"]: row["result"] for row in rows}
        assert browsed["root"]["items"][0]["name"] == volume_name, browsed["root"]
        assert browsed["cd"]["dir"] == browsed["pwd"]["dir"] == "/" + volume_name, browsed["pwd"]
        assert browsed["page"] == browsed["page-by-id"]
        assert browsed["page"]["nextCursor"] and browsed["next"]["nextCursor"] is None
        assert browsed["page"]["items"] + browsed["next"]["items"] == browsed["all"]["items"]
        path_args = ["--image", str(evidence), "files", "list"]
        page_body = {"path": "/" + volume_name, "pageSize": 2}
        body_page = json.loads(run([*path_args, "--body", json.dumps(page_body)]).stdout)
        option_page = json.loads(run([*path_args, "--path", page_body["path"], "--page-size", "2"]).stdout)
        # Reading a live folder can update host access times after the session
        # captured its index. Compare every other field across processes.
        for page in (body_page, option_page, browsed["page"]):
            for item in page["items"]:
                item["timestamps"].pop("accessed", None)
                item["timestampsOriginal"].pop("accessed", None)
        assert body_page == option_page == browsed["page"], (body_page, option_page, browsed["page"])
        assert all("parentId" in item and "parent_id" not in item for item in browsed["all"]["items"])
        assert "hello.txt" in [item["node"]["name"] for item in browsed["tree"]["items"]]
        assert browsed["stat"]["size"] == len(content) and browsed["stat"]["nodeId"] == node
        assert browsed["stat"]["path"] == f"/{volume_name}/hello.txt", browsed["stat"]
        assert "timestamps" in browsed["stat"] and not browsed["stat"]["indexOnly"]
        assert browsed["hex"] == {"encoding": "hex", "data": content[6:14].hex()}
        assert browsed["strings"]["strings"] == [
            {"offset": 0, "text": "hello forensic world", "truncated": False},
            {"offset": content.index(b"password"), "text": "password test", "truncated": False},
        ]
        assert browsed["utf8"]["strings"] == [{"offset": 1, "text": unicode_text, "truncated": False}]
        assert browsed["utf16"]["strings"] == [{"offset": 2, "text": unicode_text, "truncated": False}]
        assert len(browsed["limited"]["strings"]) == 1 and browsed["limited"]["truncated"]
        assert browsed["file"]["label"] == "python" and browsed["file"]["inferred"], browsed["file"]
        assert browsed["file"]["fileLen"] == len(python_content)
        # One-shot reads write the bytes themselves to stdout.
        image_path = ["--image", str(evidence)]
        assert run_bytes([*image_path, "files", "read", f"/{volume_name}/hello.txt", "--offset", "6", "--length", "8"]).stdout == b"forensic"
        latin1 = f"/{volume_name}/latin1.bin"
        assert run_bytes([*image_path, "files", "read", latin1]).stdout == b"caf\xe9"
        # --text decodes UTF-8 and replaces what is not.
        assert run_bytes([*image_path, "files", "read", latin1, "--text"]).stdout == "caf�".encode("utf-8")
        identified = json.loads(run([*image_path, "files", "identify", f"/{volume_name}/signature.bin"]).stdout)
        assert identified["label"] == "python", identified

        result, rows = session([
            {"id": "bad", "command": "images list", "args": {"typo": True}},
            {"id": "good", "command": "images list"},
        ], expected=1)
        assert ["error" in row for row in rows] == [True, False]

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
        png += chunk(b"IDAT", zlib.compress(b"\0\xff\0\0")) + chunk(b"IEND", b"")
        disk = root / "disk.img"
        disk.write_bytes((b"\0" * 512 + png).ljust(16384, b"\0"))
        recovered = root / "recovered"
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(disk)}},
            {"id": "scan", "command": "carve scans create", "args": {"options": {"format_ids": ["png"]}}},
            {"id": "wait", "command": "tasks wait"},
            {"id": "status", "command": "carve scans get", "args": {"scanId": {"$ref": "scan#/scanId"}}},
            {"id": "recover", "command": "carve scans export", "args": {"scanId": {"$ref": "scan#/scanId"}, "options": {"destination": str(recovered), "task_id": "recover-1"}}},
            {"id": "tasks", "command": "tasks list"},
        ])
        assert rows[3]["result"]["status"] == "completed", rows
        assert rows[3]["result"]["artifactsFound"] == 1, rows
        assert rows[4]["result"]["recovered"] == 1, rows
        assert next(recovered.glob("*.png")).read_bytes() == png
        # The task center lists the scan by its session and the export by its task id.
        entries = {entry.get("kind"): entry for entry in rows[5]["result"]}
        scan_entry = entries["carveScan"]
        assert scan_entry["source"] == "builtin" and scan_entry["status"] == "success", scan_entry
        assert scan_entry["scanId"] == rows[1]["result"]["scanId"], scan_entry
        assert scan_entry["view"]["counts"]["found"] == 1, scan_entry
        export_entry = entries["carveRecovery"]
        assert export_entry["id"] == "recover-1" and export_entry["status"] == "success", export_entry
        assert export_entry["view"]["subject"] == [str(recovered)], export_entry
        assert export_entry["view"]["counts"]["files"] == 1, export_entry

        # Filesystem recovery shares the scan session: a deleted FAT12 entry is
        # listed by carve files list and exported by candidate ID.
        floppy = bytearray(2880 * 512)
        floppy[0:3] = b"\xeb\x3c\x90"
        floppy[3:11] = b"MSDOS5.0"
        struct.pack_into("<HBHBHHBH", floppy, 11, 512, 1, 1, 2, 224, 2880, 0xF0, 9)
        floppy[38] = 0x29
        floppy[43:62] = b"RECOVERY   FAT12   "
        floppy[510:512] = b"\x55\xaa"
        for table in (512, 10 * 512):
            floppy[table:table + 3] = b"\xf0\xff\xff"
        entry = 19 * 512
        floppy[entry:entry + 12] = b"\xe5ELETED TXT\x20"
        struct.pack_into("<HI", floppy, entry + 26, 2, 5)
        floppy[33 * 512:33 * 512 + 5] = b"hello"
        deleted_image = root / "deleted-fat.img"
        deleted_image.write_bytes(bytes(floppy))
        restored = root / "restored"
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(deleted_image)}},
            {"id": "scan", "command": "carve scans create", "args": {"options": {"signatures": False, "filesystems": {"fat": True}}}},
            {"id": "wait", "command": "tasks wait"},
            {"id": "status", "command": "carve scans get", "args": {"scanId": {"$ref": "scan#/scanId"}}},
            {"id": "files", "command": "carve files list", "args": {"scanId": {"$ref": "scan#/scanId"}}},
            {"id": "recover", "command": "carve scans export", "args": {"scanId": {"$ref": "scan#/scanId"}, "options": {"artifact_ids": [], "file_ids": ["0"], "destination": str(restored), "task_id": "recover-files-1"}}},
        ])
        assert rows[3]["result"]["status"] == "completed", rows
        assert rows[3]["result"]["filesystem"]["files"] == 1, rows
        candidate = rows[4]["result"]["candidates"][0]
        assert candidate["node"]["deleted"] and candidate["readable"], rows
        exported = rows[5]["result"]["files"]
        assert rows[5]["result"]["recovered"] == 0 and exported["recovered"] == 1, rows
        # Recovered files keep their folders under one folder per volume.
        export_root = Path(exported["directory"])
        outputs = sorted(path for path in export_root.rglob("*") if path.is_file() and path.name != "recovery-report.jsonl")
        assert [path.read_bytes() for path in outputs] == [b"hello"], outputs
        relative = outputs[0].relative_to(export_root).parts
        assert len(relative) == 2 and relative[0].startswith("v0-"), relative
        report = [json.loads(line) for line in Path(exported["reportPath"]).read_text(encoding="utf-8").splitlines()]
        assert report[1]["relative_output"] == "/".join(relative), report

        # Split RAW exports must include every member, including unaligned cuts.
        split_bytes = bytes(range(256)) * 513
        split_first = root / "split evidence.raw.001"
        split_second = root / "split evidence.raw.002"
        split_third = root / "split evidence.raw.003"
        split_first.write_bytes(split_bytes[:517])
        split_second.write_bytes(split_bytes[517:65539])
        split_third.write_bytes(split_bytes[65539:])
        split_export = root / "split-export.raw"
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(split_first)}},
            {"id": "export", "command": "images export", "args": {"destination": str(split_export), "taskId": "split-export", "readFaultPolicy": "strict"}},
            {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
        ])
        assert all("result" in row for row in rows), rows
        assert rows[0]["result"]["sourceFormat"] == "raw", rows
        assert rows[0]["result"]["sizeBytes"] == len(split_bytes), rows
        assert split_export.read_bytes() == split_bytes
        assert split_first.read_bytes() == split_bytes[:517]
        assert split_second.read_bytes() == split_bytes[517:65539]
        assert split_third.read_bytes() == split_bytes[65539:]
        split_second.unlink()
        result, rows = session([{"id": "open", "command": "images open", "args": {"path": str(split_first)}}], expected=1)
        assert "error" in rows[0], result.stdout

        # Forced RAW opens and exports the prefix, without silently changing a live policy.
        forced_export = root / "forced-prefix.raw"
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(split_first), "rawReadMode": "force"}},
            {"id": "export", "command": "images export", "args": {"destination": str(forced_export), "taskId": "forced-export", "readFaultPolicy": "strict"}},
            {"id": "mode-conflict", "command": "images open", "args": {"path": str(split_first), "rawReadMode": "strict"}},
            {"id": "close", "command": "images close", "args": {"imageId": {"$ref": "open#/id"}}},
        ], expected=1)
        assert rows[0]["result"]["rawReadMode"] == "force", rows
        assert rows[0]["result"]["sizeBytes"] == 517, rows
        assert "result" in rows[1] and "error" in rows[2] and "result" in rows[3], rows
        assert forced_export.read_bytes() == split_bytes[:517]

        # Hash and export actual EWF evidence through the public headless commands.
        fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor/ewf/tests/data"
        for name, expected_md5 in [
            ("ntfs_sample.E01", "e4e9578a9c8bf6c6b375a80c630196a4"),
            ("zeros_128s_compressed.Ex01", hashlib.md5(bytes(65536)).hexdigest()),
        ]:
            raw_export = root / (name + ".raw")
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(fixtures / name)}},
                {"id": "hash", "command": "images hash", "args": {"algorithms": ["md5", "sha1"], "taskId": "ewf-hash", "readFaultPolicy": "strict"}},
                {"id": "export", "command": "images export", "args": {"destination": str(raw_export), "taskId": "ewf-export"}},
            ])
            assert all("result" in row for row in rows), rows
            raw_bytes = raw_export.read_bytes()
            assert hashlib.md5(raw_bytes).hexdigest() == expected_md5
            hashes = rows[1]["result"]
            assert hashes["md5"] == expected_md5, hashes
            assert hashes["sha1"] == hashlib.sha1(raw_bytes).hexdigest(), hashes
            assert not hashes["readFaultsDetected"], hashes

        # VMDK compressed and split extents must export the original virtual bytes.
        fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor/vmdk/tests/data"
        for name, expected in [
            ("compressed_stream_opt.vmdk", bytes(range(64)) * 1024),
            ("tw_sparse_data.vmdk", bytes(range(256)) * 16384),
            ("mono_flat.vmdk", bytes(1024 * 1024)),
        ]:
            raw_export = root / (name + ".raw")
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(fixtures / name)}},
                {"id": "export", "command": "images export", "args": {"destination": str(raw_export), "taskId": "vmdk-export"}},
            ])
            assert all("result" in row for row in rows), rows
            assert raw_export.read_bytes() == expected, name

        # Full logical streams must match hashes captured with the previous
        # independent libvhdi reader. The CLI itself does not load that DLL.
        for kind in ("vhd", "vhdx"):
            fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor" / kind / "tests/data"
            oracle = json.loads((fixtures / "LOVELYCARVE_ORACLE.json").read_text(encoding="utf-8"))
            for sample in oracle["images"]:
                raw_export = root / (sample["name"] + ".raw")
                result, rows = session([
                    {"id": "open", "command": "images open", "args": {"path": str(fixtures / sample["name"])}},
                    {"id": "export", "command": "images export", "args": {"destination": str(raw_export), "taskId": "vhd-export"}},
                ])
                assert all("result" in row for row in rows), rows
                assert raw_export.stat().st_size == sample["size"]
                assert hashlib.sha256(raw_export.read_bytes()).hexdigest() == sample["sha256"], sample

        # Unlock and export through the public commands, checking only retained
        # oracle sectors of the minimized dfvfs projection (not the omitted data).
        fixture = Path(__file__).resolve().parents[1] / "src-tauri/vendor/bitlocker/tests/data/bdetogo-projection.bin.gz"
        encrypted = root / "bitlocker.bin"
        encrypted.write_bytes(gzip.decompress(fixture.read_bytes()))
        decrypted = root / "bitlocker-decrypted.bin"
        current_raw = root / "bitlocker-current.raw"
        partition = {"$ref": "open#/partitions/0/id"}
        result, rows = session([
            {"id": "open", "command": "images open", "args": {"path": str(encrypted)}},
            {"id": "unlock", "command": "bitlocker unlock", "args": {"partitionId": partition, "password": "bde-TEST"}},
            {"id": "status", "command": "bitlocker get", "args": {"partitionId": partition}},
            {"id": "export", "command": "bitlocker export", "args": {"partitionId": partition, "destination": str(decrypted), "taskId": "bde-export"}},
            {"id": "current", "command": "images export", "args": {"destination": str(current_raw), "taskId": "bde-current", "mode": "unlockedPartitions"}},
        ])
        assert all("result" in row for row in rows), rows
        assert rows[1]["result"]["locked"] is False and rows[2]["result"]["locked"] is False
        assert "bde-TEST" not in result.stdout + result.stderr
        assert decrypted.stat().st_size == encrypted.stat().st_size
        assert current_raw.read_bytes() == decrypted.read_bytes()
        with decrypted.open("rb") as stream:
            assert hashlib.sha256(stream.read(512)).hexdigest() == "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3"
            stream.seek(0x8000)
            assert hashlib.sha256(stream.read(4096)).hexdigest() == "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d"

        # Independent cipher projections exercise every credential path.
        bde_data = fixture.parent
        for method, sector_size, credential in [
            (0x8001, 512, {}),
            (0x8005, 4096, {"startupKeyPath": str(bde_data / "parity-startup.bek")}),
            (0x8004, 512, {"vmkHex": "46" * 32}),
            (0x8001, 4096, {"fvekHex": bytes(range(1, 33)).hex(), "tweakHex": bytes(range(65, 97)).hex()}),
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
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(encrypted)}},
                {"id": "metadata", "command": "bitlocker metadata get", "args": {"partitionId": partition}},
                {"id": "unlock", "command": "bitlocker vmks unlock" if "vmkHex" in credential else "bitlocker unlock", "args": {"partitionId": partition, **credential}},
                {"id": "export", "command": "bitlocker export", "args": {"partitionId": partition, "destination": str(decrypted), "taskId": "parity-export"}},
            ])
            assert all("result" in row for row in rows), rows
            metadata = rows[1]["result"]
            assert metadata["encryptionMethod"] == method and metadata["bytesPerSector"] == sector_size
            assert metadata["protectorTypes"] == [0, 0x0200]
            if "vmkHex" in credential:
                assert rows[2]["result"]["status"]["locked"] is False
                assert rows[2]["result"]["recoveryKey"] == ""
            else:
                assert rows[2]["result"]["locked"] is False
            plain = bytearray((index * 7 + 19) % 256 for index in range(sector_size))
            plain[3:11] = b"NTFS    "
            decrypted_bytes = decrypted.read_bytes()
            assert len(decrypted_bytes) == len(image) and decrypted_bytes[:sector_size] == plain
            assert decrypted_bytes[0x12000:0x12000 + sector_size] == plain
            assert decrypted_bytes[0x10000:0x10000 + sector_size] == bytes(sector_size)
            assert decrypted_bytes[0x20000:0x30000] == bytes(0x10000)

        if system["qrcode"] or system["ocr"]:
            blank = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 256, 64, 8, 2, 0, 0, 0))
            blank += chunk(b"IDAT", zlib.compress((b"\0" + b"\xff" * (256 * 3)) * 64)) + chunk(b"IEND", b"")
            (evidence / "blank.png").write_bytes(blank)
            requests = [{"id": "image", "command": "images open", "args": {"path": str(evidence)}}]
            image_node = "folder:" + b"blank.png".hex()
            if system["qrcode"]:
                requests.append({"id": "qr", "command": "files qr-codes scan", "args": {"nodeId": image_node, "expectedCount": 1, "taskId": "qr-1"}})
            if system["ocr"]:
                requests.append({"id": "ocr", "command": "files ocr scan", "args": {"nodeId": image_node, "taskId": "ocr-1"}})
            result, rows = session(requests)
            assert all("result" in row for row in rows), result

        # A real case must close cleanly and be reopenable by the next process.
        case = root / "case"
        nested = evidence / "nested"
        nested.mkdir()
        (nested / "report.txt").write_text("Password nested\npassword second\n", encoding="utf-8")
        run(["cases", "create", str(case), "--name", "CLI smoke test"])
        result, rows = session([
            {"id": "image", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "wait", "command": "tasks wait"},
            {"id": "search", "command": "searches create", "args": {"searchId": "cli-search", "options": {"keys": ["password"]}}},
            {"id": "timeline", "command": "timeline build"},
            {"id": "plugins", "command": "plugins list"},
        ], arguments=["--case", str(case)])
        assert all("result" in row for row in rows), rows
        assert rows[2]["result"]["totalMatches"] >= 1, rows
        # files search uses the same persisted global-search results, with scopes,
        # case handling, regex matching, and a display limit independent of storage.
        grep_script = f'''images open "{evidence}"
tasks wait
cd "{volume_name}"
files search Password hello.txt --codepage UTF-8 --search-id case-sensitive
files search password nested --ignore-case --codepage UTF-8 --limit 1 --search-id scoped-grep
files search "pass(word)?" hello.txt --regex --codepage UTF-8 --search-id regex-grep
searches hits list --search-id scoped-grep --limit 10
'''
        grep_rows = documents(run(["--case", str(case), "shell"], grep_script).stdout)
        assert len(grep_rows) == 5, grep_rows
        assert grep_rows[1]["total"] == 0, grep_rows
        scoped = grep_rows[2]
        assert scoped["summary"]["status"] == "success" and scoped["summary"]["searchId"] == "scoped-grep"
        assert scoped["total"] == 2 and len(scoped["hits"]) == 1 and scoped["truncated"], scoped
        assert scoped["hits"][0]["nodeId"] == "folder:" + b"nested/report.txt".hex(), scoped
        assert grep_rows[3]["total"] >= 1 and all(hit["patternKind"] == "regex" for hit in grep_rows[3]["hits"])
        assert grep_rows[4]["total"] == 2 and len(grep_rows[4]["items"]) == 2, grep_rows
        run([*image_path, "files", "search", "password"], expected=1)

        # Missing source bytes force offline sessions; path resolution and the
        # recursive listing must still come entirely from the published case indexes.
        moved_evidence = root / "temporarily absent evidence"
        evidence.rename(moved_evidence)
        offline_script = f'''images open "{evidence}"
cd "{volume_name}"
files list --path nested
files tree list --path . --depth 128 --name REPORT
cd nested
context get
'''
        try:
            offline_rows = documents(run(["--case", str(case), "shell"], offline_script).stdout)
            assert offline_rows[0]["indexOnly"], offline_rows
            assert [item["name"] for item in offline_rows[1]["items"]] == ["report.txt"]
            assert [item["path"] for item in offline_rows[2]["items"]] == [f"/{volume_name}/nested/report.txt"]
            assert offline_rows[3]["dir"] == f"/{volume_name}/nested"
            result, rows = session([
                {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
                {"id": "cd", "command": "context set", "args": {"dir": "/" + volume_name}},
                {"id": "stat", "command": "files get", "args": {"path": "nested/report.txt"}},
                {"id": "tree", "command": "files tree list", "args": {"path": "nested", "depth": 2}},
                {"id": "hex", "command": "files read", "args": {"path": "hello.txt"}},
                {"id": "strings", "command": "files strings list", "args": {"path": "hello.txt"}},
                {"id": "file", "command": "files identify", "args": {"path": "hello.txt"}},
                {"id": "pwd", "command": "context get"},
            ], expected=1, arguments=["--case", str(case)])
            offline_results = {row["id"]: row for row in rows}
            assert offline_results["stat"]["result"]["indexOnly"] and offline_results["stat"]["result"]["name"] == "report.txt"
            assert [item["path"] for item in offline_results["tree"]["result"]["items"]] == [f"/{volume_name}/nested/report.txt"]
            assert all("error" in offline_results[command] for command in ("hex", "strings", "file"))
            assert offline_results["pwd"]["result"]["dir"] == "/" + volume_name
            run(["--case", str(case), *image_path, "files", "search", "password"], expected=1)
        finally:
            moved_evidence.rename(evidence)
        active = json.loads(run(["--case", str(case), "cases", "get"]).stdout)
        assert active["name"] == "CLI smoke test", active
        run(["cases", "delete", str(case), "--force"])

        if os.name == "nt":
            # Nothing from the desktop runtime is linked, let alone started.
            binary = executable.read_bytes()
            assert b"WebView2Loader" not in binary, "the CLI links the desktop WebView"
            assert b"requireAdministrator" not in binary, "the CLI requests elevation"
            process = subprocess.Popen([str(executable), "session"], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                       encoding="utf-8", creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
                                       cwd=root, env=environment)
            try:
                process.stdin.write('{"id":"ready","command":"images list"}\n')
                process.stdin.flush()
                assert json.loads(process.stdout.readline())["id"] == "ready"
                windows = []
                callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
                ctypes.windll.user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
                ctypes.windll.user32.GetWindowTextW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
                ctypes.windll.user32.GetClassNameW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
                ctypes.windll.user32.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.RECT)]

                @callback_type
                def inspect_window(window, _):
                    owner = ctypes.c_ulong()
                    ctypes.windll.user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
                    if owner.value == process.pid:
                        title = ctypes.create_unicode_buffer(512)
                        class_name = ctypes.create_unicode_buffer(512)
                        ctypes.windll.user32.GetWindowTextW(window, title, len(title))
                        ctypes.windll.user32.GetClassNameW(window, class_name, len(class_name))
                        rectangle = wintypes.RECT()
                        ctypes.windll.user32.GetWindowRect(window, ctypes.byref(rectangle))
                        windows.append((window, title.value, class_name.value, tuple(getattr(rectangle, field) for field in ("left", "top", "right", "bottom"))))
                    return True

                ctypes.windll.user32.EnumWindows(inspect_window, None)
                assert not windows, f"CLI created a window: {windows}"
                process.send_signal(signal.CTRL_BREAK_EVENT)
                assert process.wait(timeout=15) == 130
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=15)

    test_resident_sessions(executable, environment)
    test_navigation(executable, environment)
    test_observability(executable, environment)
    test_timezones(executable, environment)
    test_integrity(executable, environment)
    print("CLI smoke tests passed: discovery, dry run, JSONL, browsing, binary read, hash, preview cleanup, export, errors, carving, search, timeline, case lifecycle, windowless startup and interrupt")


if __name__ == "__main__":
    main()
