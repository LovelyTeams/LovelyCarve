"""End-to-end tests against the actual CLI process (no desktop or web server)."""
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


def main():
    executable = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="lovelycarve-cli-test-") as directory:
        root = Path(directory)
        evidence = root / "evidence with spaces"
        evidence.mkdir()
        content = b"hello forensic world\npassword test\n"
        (evidence / "hello.txt").write_bytes(content)
        copied = root / "copy.bin"
        node = "folder:" + b"hello.txt".hex()

        def run(arguments, stdin=None, expected=0):
            process = subprocess.run([str(executable), *arguments], input=stdin,
                                     text=True, encoding="utf-8", capture_output=True,
                                     timeout=120, cwd=root)
            assert process.returncode == expected, (process.returncode, process.stdout, process.stderr)
            return process

        catalog = json.loads(run(["commands", "--json"]).stdout)
        assert len(catalog) >= 148
        assert any(command["name"] == "fs_media_temp" for command in catalog)
        capabilities = json.loads(run(["capabilities", "--json"]).stdout)
        assert capabilities["windowless"] and capabilities["backend_commands"] == len(catalog)
        assert capabilities["session"]["max_jobs"] == 64
        assert next(command for command in catalog if command["name"] == "fs_read_range")["parallel"]
        assert not next(command for command in catalog if command["name"] == "image_unload")["parallel"]
        run(["--help"])
        run(["fs-read", "--help"])
        run(["--unknown"], expected=2)
        run(["session", "--jobs", "0"], expected=2)
        run(["session", "--jobs", "65"], expected=2)

        concurrent_requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(evidence)}},
            {"id": "cd", "command": "cd", "args": {"path": evidence.name}},
            {"id": "read", "command": "fs_read_range", "args": {"nodeId": node, "offset": 0, "length": 8}},
            {"id": "digest", "command": "fs_digest", "args": {"nodeId": node, "algos": ["sha256"]}},
            {"id": "stat", "command": "stat", "args": {"path": "hello.txt"}},
            {"id": "dependent", "command": "fs_read_range", "args": {"nodeId": {"$ref": "stat#/node_id"}, "offset": 0, "length": 8}},
            {"id": "bad", "command": "image_list", "args": {"typo": True}},
            {"id": "invalid-args", "command": "image_list", "args": []},
            {"id": "wait", "command": "wait"},
            {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
            {"id": "remaining", "command": "image_list"},
        ]
        result = run(["session", "--jobs", "4"], "\n".join(json.dumps(request) for request in concurrent_requests), expected=1)
        concurrent_rows = {row["id"]: row for row in map(json.loads, result.stdout.splitlines())}
        assert len(concurrent_rows) == len(concurrent_requests), result.stdout
        assert concurrent_rows["read"]["result"]["data"] == content[:8].hex()
        assert concurrent_rows["dependent"]["result"] == concurrent_rows["read"]["result"]
        assert concurrent_rows["digest"]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert concurrent_rows["remaining"]["result"] == []
        assert all("result" in row for key, row in concurrent_rows.items() if key not in {"bad", "invalid-args"}), concurrent_rows
        assert all("code" in concurrent_rows[key]["error"] and "message" in concurrent_rows[key]["error"] for key in ["bad", "invalid-args"])

        # AI clients keep stdin open and wait for a response before sending more.
        process = subprocess.Popen([str(executable), "--image", str(evidence), "session", "-j", "4"],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, encoding="utf-8", cwd=root)
        response_queue = queue.Queue()
        reader_thread = threading.Thread(target=lambda: response_queue.put(process.stdout.readline()), daemon=True)
        try:
            process.stdin.write(json.dumps({"id": "live", "command": "fs_read_range", "args": {"nodeId": node, "offset": 0, "length": 8}}) + "\n")
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
                                               text=True, encoding="utf-8", cwd=root)
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
                    try:
                        roots = batch([{"id": "roots", "command": "fs_children", "args": {"parentId": ""}}])
                        directories = [entry["id"] for entry in roots["roots"] if entry["kind"] == "dir"][:4]
                        requests = [{"id": f"dir-{index}", "command": "fs_children", "args": {"parentId": parent}}
                                    for index, parent in enumerate(directories)]
                        assert requests, "real evidence has no browsable volumes"
                        directory_rows = batch(requests)
                        files = sorted((entry for entries in directory_rows.values() for entry in entries
                                        if entry["kind"] == "file" and entry["size"] > 0 and not entry["deleted"]),
                                       key=lambda entry: entry["id"])[:4]
                        # Files may live below a filesystem's top-level directories.
                        children = sorted(entry["id"] for entries in directory_rows.values() for entry in entries
                                          if entry["kind"] == "dir" and not entry["deleted"])[:8]
                        if not files and children:
                            nested = batch([{"id": f"nested-{index}", "command": "fs_children", "args": {"parentId": parent}}
                                            for index, parent in enumerate(children)])
                            directory_rows.update(nested)
                            files = sorted((entry for entries in nested.values() for entry in entries
                                            if entry["kind"] == "file" and entry["size"] > 0 and not entry["deleted"]),
                                           key=lambda entry: entry["id"])[:4]
                        assert files, "real evidence has no file for bounded read verification"
                        byte_requests = [{"id": f"bytes-{index}", "command": "fs_read_range", "args": {
                            "nodeId": entry["id"], "offset": 0, "length": min(4096, entry["size"])}}
                            for index, entry in enumerate(files)]
                        byte_rows = batch(byte_requests)
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
        # as guest bytes by the same image_load/export/unload command surface.
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
            requests = [
                {"id": "open", "command": "image_load", "args": {"path": str(source_path)}},
                {"id": "export", "command": "image_export_raw", "args": {
                    "destination": str(destination), "taskId": f"phdi-{suffix}", "readFaultPolicy": "strict"}},
                {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
            ]
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert all("result" in row for row in rows), rows
            assert rows[0]["result"]["source_format"] == "phdi", rows
            assert destination.read_bytes() == phdi_plain + bytes(phdi_sparse[512:]) + bytes(512)
            assert (phdi_bundle / "plain.hds").read_bytes() == phdi_plain
            assert (phdi_bundle / "sparse.hds").read_bytes() == phdi_sparse

        # A recognized corrupt descriptor must fail, rather than become RAW.
        (phdi_bundle / "DiskDescriptor.xml").write_text("broken", encoding="utf-8")
        result = run(["session"], json.dumps({"id": "broken-phdi", "command": "image_load", "args": {
            "path": str(phdi_bundle / "DiskDescriptor.xml")}}), expected=1)
        assert "error" in json.loads(result.stdout), result

        # The actual generated image_load dispatch must accept transient QCOW
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
                requests = [
                    {"id": "open", "command": "image_load", "args": {"path": str(source_path), **credential}},
                    {"id": "export", "command": "image_export_raw", "args": {
                        "destination": str(destination), "taskId": "qcow-export", "readFaultPolicy": "strict"}},
                    {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
                ]
                result = run(["session"], "\n".join(json.dumps(request) for request in requests))
                rows = [json.loads(line) for line in result.stdout.splitlines()]
                assert all("result" in row for row in rows), rows
                assert rows[0]["result"]["source_format"] == "qcow2", rows
                expected = qcow_plain + bytes(4096) if encoding == "deflate" else bytes(4096) + qcow_plain
                assert destination.read_bytes() == expected
                assert source_path.read_bytes() == original
                assert "qcow-test" not in result.stdout + result.stderr
                assert qcow_key_hex not in result.stdout + result.stderr
        for credential in [{}, {"qcowPassword": "qcow-test", "qcowKeyHex": qcow_key_hex},
                           {"qcowKeyHex": "aa"}, {"qcowPassword": "非 ASCII"}]:
            request = {"id": "invalid", "command": "image_load", "args": {
                "path": str(qcow_fixtures / "v2-aes.qcow"), **credential}}
            result = run(["session"], json.dumps(request), expected=1)
            assert "error" in json.loads(result.stdout), result

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
            requests = [
                {"id": "open", "command": "image_load", "args": {"path": str(compressed_tar)}},
                {"id": "children", "command": "fs_children", "args": {"parentId": "tar:"}},
                {"id": "read", "command": "fs_read", "args": {"nodeId": "tar:" + b"second.txt".hex()}, "output": str(output)},
                {"id": "digest", "command": "fs_digest", "args": {"nodeId": "tar:" + b"second.txt".hex(), "algos": ["sha256"]}},
                {"id": "wait", "command": "wait"},
                {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
            ]
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            rows = {row["id"]: row for row in map(json.loads, result.stdout.splitlines())}
            assert all("result" in row for row in rows.values()), rows
            assert rows["open"]["result"]["source_format"] == "tar", rows
            assert output.read_bytes() == content
            assert rows["digest"]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
            assert compressed_tar.read_bytes() == gzip_bytes

        # A bad trailer after a valid tar must not publish a partial tar tree.
        # Preserve the extension probe's existing RAW fallback for inspection.
        corrupt_gzip = bytearray(gzip_bytes)
        corrupt_gzip[-8] ^= 1
        corrupt_tar = root / "corrupt-tail.tgz"
        corrupt_tar.write_bytes(corrupt_gzip)
        requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(corrupt_tar)}},
            {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert all("result" in row for row in rows), rows
        assert rows[0]["result"]["source_format"] == "raw", rows

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
            encrypted_requests = [
                {"id": "source", "command": "image_load", "args": {"path": str(encrypted_fixtures / filename)}},
                {"id": "unlock", "command": "encrypted_volume_unlock", "args": {"options": options}},
                {"id": "plain", "command": "image_load", "args": {"path": {"$ref": "unlock#/image/path"}}},
                {"id": "files", "command": "fs_children", "args": {"parentId": "part:0"}},
                {"id": "read", "command": "fs_read", "args": {"nodeId": {"$ref": "files#/0/id"}}, "output": str(exported_file)},
                {"id": "export", "command": "image_export_raw", "args": {"destination": str(exported_plain), "taskId": "encrypted-export", "readFaultPolicy": "strict"}},
                {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "source#/id"}}},
                {"id": "remaining", "command": "image_list"},
            ]
            encrypted_result = run(["session"], "\n".join(json.dumps(request) for request in encrypted_requests))
            encrypted_rows = [json.loads(line) for line in encrypted_result.stdout.splitlines()]
            assert all("result" in row for row in encrypted_rows), encrypted_rows
            assert exported_plain.read_bytes() == plaintext, filename
            assert exported_file.read_bytes() == b"LovelyCarve encrypted evidence fixture.\n", filename
            assert encrypted_rows[1]["result"]["image"]["source_format"] == "decrypted"
            assert encrypted_rows[-1]["result"] == [], "decrypted child survived source unload"

            attached_file = root / (filename + ".attached.txt")
            attached_raw = root / (filename + ".source.raw")
            attached_plain = root / (filename + ".attached.raw")
            attached_requests = [
                {"id": "case", "command": "case_create", "args": {"dir": str(root / (filename + "-case")), "name": "attached unlock"}},
                {"id": "source", "command": "image_load", "args": {"path": str(encrypted_fixtures / filename)}},
                {"id": "unlock", "command": "encrypted_volume_unlock", "args": {"options": {**options, "target": "sourcePartition"}}},
                {"id": "images", "command": "image_list"},
                {"id": "wait", "command": "wait"},
                {"id": "indexes", "command": "snapshot_list"},
                {"id": "read", "command": "fs_read", "args": {"nodeId": "fat:part:0:" + b"/HELLO.TXT".hex()}, "output": str(attached_file)},
                {"id": "raw", "command": "image_export_raw", "args": {"destination": str(attached_raw), "taskId": "attached-raw", "readFaultPolicy": "strict"}},
                {"id": "plaintext", "command": "image_export_raw", "args": {"destination": str(attached_plain), "taskId": "attached-plaintext", "readFaultPolicy": "strict", "mode": "unlockedPartitions"}},
                {"id": "close", "command": "case_close"},
                {"id": "remove", "command": "case_remove", "args": {"dir": str(root / (filename + "-case"))}},
            ]
            attached_result = run(["session"], "\n".join(json.dumps(request) for request in attached_requests))
            attached_rows = {row["id"]: row for row in map(json.loads, attached_result.stdout.splitlines())}
            assert all("result" in row for row in attached_rows.values()), attached_rows
            source_id = attached_rows["source"]["result"]["id"]
            attached = attached_rows["unlock"]["result"]
            assert attached["image"]["id"] == source_id and attached["volume_key"] == "part:0"
            assert attached["image"]["unlocked_partitions"] == [0]
            assert len(attached_rows["images"]["result"]) == 1, "in-place unlock created another image"
            assert any(index["image_id"] == source_id and index["volume_key"] == "part:0" and index["indexed"]
                       for index in attached_rows["indexes"]["result"]), (filename, attached_rows["indexes"], attached_result.stderr)
            assert attached_file.read_bytes() == b"LovelyCarve encrypted evidence fixture.\n"
            assert attached_raw.read_bytes() == (encrypted_fixtures / filename).read_bytes(), "source export changed after unlock"
            converted = attached_plain.read_bytes()
            assert len(converted) == attached_raw.stat().st_size
            assert converted[:len(plaintext)] == plaintext, filename
            assert converted[len(plaintext):] == bytes(len(converted) - len(plaintext)), filename

        requests = [
            {"id": "image", "command": "image_load", "args": {"path": str(evidence)}},
            {"id": "roots", "command": "fs_children", "args": {"parentId": ""}},
            {"id": "read", "command": "fs_read", "args": {"imageId": {"$ref": "image#/id"}, "nodeId": node}, "output": str(copied)},
            {"id": "digest", "command": "fs_digest", "args": {"nodeId": node, "algos": ["sha256"]}},
            {"id": "preview", "command": "fs_media_temp", "args": {"nodeId": node, "ext": "txt", "taskId": "preview-1"}},
            {"id": "wait", "command": "wait"},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert len(rows) == len(requests), result.stdout
        assert all("result" in row for row in rows), rows
        assert copied.read_bytes() == content
        assert rows[3]["result"]["sha256"] == hashlib.sha256(content).hexdigest()
        assert not Path(rows[4]["result"]).exists(), "preview lease leaked after process exit"

        exported = root / "export.txt"
        script = root / "commands.txt"
        script.write_text(f'''open "{evidence}"
cd "{evidence.name}"
ls
cat hello.txt --length 5
sha256sum hello.txt
export hello.txt "{exported}"
''', encoding="utf-8")
        result = run(["--json", "run", str(script)])
        assert exported.read_bytes() == content
        assert any(json.loads(line).get("data") == "hello" for line in result.stdout.splitlines())
        run(["--json", "run", str(script)], expected=1)
        assert exported.read_bytes() == content

        # All shell shortcuts also work through the stateful JSONL adapter.
        # Content recognition must ignore the misleading filename extension.
        python_content = b"#!/usr/bin/env python3\nimport sys\nprint('LovelyCarve forensic CLI')\n" * 20
        (evidence / "signature.bin").write_bytes(python_content)
        unicode_text = "中文取证字符串"
        (evidence / "unicode.bin").write_bytes(b"\0" + unicode_text.encode("utf-8") + b"\0")
        (evidence / "wide.bin").write_bytes(("\0" + unicode_text + "\0").encode("utf-16le"))
        shell_requests = [
            {"id": "open", "command": "open", "args": {"path": str(evidence)}},
            {"id": "wait", "command": "wait"},
            {"id": "cd", "command": "cd", "args": {"path": "/" + evidence.name}},
            {"id": "pwd", "command": "pwd"},
            {"id": "tree", "command": "tree", "args": {"depth": 2}},
            {"id": "stat", "command": "stat", "args": {"path": "hello.txt"}},
            {"id": "hex", "command": "hex", "args": {"path": "hello.txt", "offset": 6, "length": 8}},
            {"id": "strings", "command": "strings", "args": {"path": "hello.txt"}},
            {"id": "utf8", "command": "strings", "args": {"path": "unicode.bin", "encoding": "utf-8"}},
            {"id": "utf16", "command": "strings", "args": {"path": "wide.bin", "encoding": "utf-16le"}},
            {"id": "limited", "command": "strings", "args": {"path": "hello.txt", "limit": 1}},
            {"id": "file", "command": "file", "args": {"path": "signature.bin"}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in shell_requests))
        shortcut_rows = {row["id"]: row["result"] for row in map(json.loads, result.stdout.splitlines())}
        assert shortcut_rows["cd"] is None and shortcut_rows["pwd"] == "/" + evidence.name
        assert "hello.txt" in [item["node"]["name"] for item in shortcut_rows["tree"]["items"]]
        assert shortcut_rows["stat"]["size"] == len(content) and shortcut_rows["stat"]["node_id"] == node
        assert "timestamps" in shortcut_rows["stat"] and not shortcut_rows["stat"]["index_only"]
        assert shortcut_rows["hex"]["data"] == content[6:14].hex() and shortcut_rows["hex"]["offset"] == 6
        assert shortcut_rows["strings"]["strings"] == [
            {"offset": 0, "text": "hello forensic world", "truncated": False},
            {"offset": content.index(b"password"), "text": "password test", "truncated": False},
        ]
        assert shortcut_rows["utf8"]["strings"] == [{"offset": 1, "text": unicode_text, "truncated": False}]
        assert shortcut_rows["utf16"]["strings"] == [{"offset": 2, "text": unicode_text, "truncated": False}]
        assert len(shortcut_rows["limited"]["strings"]) == 1 and shortcut_rows["limited"]["truncated"]
        assert shortcut_rows["file"]["label"] == "python" and shortcut_rows["file"]["inferred"], shortcut_rows["file"]
        assert shortcut_rows["file"]["file_len"] == len(python_content)
        hex_output = run(["--image", str(evidence), "hex", f"/{evidence.name}/hello.txt", "--offset", "6", "--length", "8"]).stdout
        assert "0000000000000006" in hex_output and "66 6f 72 65 6e 73 69 63" in hex_output and "|forensic|" in hex_output
        file_output = run(["--image", str(evidence), "file", f"/{evidence.name}/signature.bin"]).stdout
        assert "python" in file_output and "置信度" in file_output, file_output

        bad = run(["session"], '\n'.join([
            '{"id":"bad","command":"image_list","args":{"typo":true}}',
            '{"id":"good","command":"image_list"}',
        ]), expected=1)
        assert ["error" in json.loads(line) for line in bad.stdout.splitlines()] == [True, False]

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
        png += chunk(b"IDAT", zlib.compress(b"\0\xff\0\0")) + chunk(b"IEND", b"")
        disk = root / "disk.img"
        disk.write_bytes((b"\0" * 512 + png).ljust(16384, b"\0"))
        recovered = root / "recovered"
        requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(disk)}},
            {"id": "scan", "command": "carve_start", "args": {"options": {"format_ids": ["png"]}}},
            {"id": "wait", "command": "wait"},
            {"id": "status", "command": "carve_scan_status", "args": {"scanId": {"$ref": "scan#/scan_id"}}},
            {"id": "recover", "command": "carve_recover", "args": {"scanId": {"$ref": "scan#/scan_id"}, "options": {"destination": str(recovered), "task_id": "recover-1"}}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert rows[3]["result"]["status"] == "completed", rows
        assert rows[3]["result"]["artifacts_found"] == 1, rows
        assert rows[4]["result"]["recovered"] == 1, rows
        assert next(recovered.glob("*.png")).read_bytes() == png

        # Filesystem recovery shares the scan session: a deleted FAT12 entry is
        # listed by carve_filesystem_results and exported by candidate ID.
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
        requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(deleted_image)}},
            {"id": "scan", "command": "carve_start", "args": {"options": {"signatures": False, "filesystems": {"fat": True}}}},
            {"id": "wait", "command": "wait"},
            {"id": "status", "command": "carve_scan_status", "args": {"scanId": {"$ref": "scan#/scan_id"}}},
            {"id": "files", "command": "carve_filesystem_results", "args": {"scanId": {"$ref": "scan#/scan_id"}}},
            {"id": "recover", "command": "carve_recover", "args": {"scanId": {"$ref": "scan#/scan_id"}, "options": {"artifact_ids": [], "file_ids": ["0"], "destination": str(restored), "task_id": "recover-files-1"}}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
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
        report = [json.loads(line) for line in Path(exported["report_path"]).read_text(encoding="utf-8").splitlines()]
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
        requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(split_first)}},
            {"id": "export", "command": "image_export_raw", "args": {"destination": str(split_export), "taskId": "split-export", "readFaultPolicy": "strict"}},
            {"id": "close", "command": "image_unload", "args": {"imageId": {"$ref": "open#/id"}}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert all("result" in row for row in rows), rows
        assert rows[0]["result"]["source_format"] == "raw", rows
        assert rows[0]["result"]["size_bytes"] == len(split_bytes), rows
        assert split_export.read_bytes() == split_bytes
        assert split_first.read_bytes() == split_bytes[:517]
        assert split_second.read_bytes() == split_bytes[517:65539]
        assert split_third.read_bytes() == split_bytes[65539:]
        split_second.unlink()
        requests = [{"id": "open", "command": "image_load", "args": {"path": str(split_first)}}]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests), expected=1)
        assert "error" in json.loads(result.stdout.splitlines()[0]), result.stdout

        # Read and export actual EWF evidence through the public headless commands.
        fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor/ewf/tests/data"
        for name, expected_md5 in [
            ("ntfs_sample.E01", "e4e9578a9c8bf6c6b375a80c630196a4"),
            ("zeros_128s_compressed.Ex01", hashlib.md5(bytes(65536)).hexdigest()),
        ]:
            raw_export = root / (name + ".raw")
            requests = [
                {"id": "open", "command": "image_load", "args": {"path": str(fixtures / name)}},
                {"id": "export", "command": "image_export_raw", "args": {"destination": str(raw_export), "taskId": "ewf-export"}},
            ]
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert all("result" in row for row in rows), rows
            assert hashlib.md5(raw_export.read_bytes()).hexdigest() == expected_md5

        # VMDK compressed and split extents must export the original virtual bytes.
        fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor/vmdk/tests/data"
        for name, expected in [
            ("compressed_stream_opt.vmdk", bytes(range(64)) * 1024),
            ("tw_sparse_data.vmdk", bytes(range(256)) * 16384),
            ("mono_flat.vmdk", bytes(1024 * 1024)),
        ]:
            raw_export = root / (name + ".raw")
            requests = [
                {"id": "open", "command": "image_load", "args": {"path": str(fixtures / name)}},
                {"id": "export", "command": "image_export_raw", "args": {"destination": str(raw_export), "taskId": "vmdk-export"}},
            ]
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert all("result" in row for row in rows), rows
            assert raw_export.read_bytes() == expected, name

        # Full logical streams must match hashes captured with the previous
        # independent libvhdi reader. The CLI itself does not load that DLL.
        for kind in ("vhd", "vhdx"):
            fixtures = Path(__file__).resolve().parents[1] / "src-tauri/vendor" / kind / "tests/data"
            oracle = json.loads((fixtures / "LOVELYCARVE_ORACLE.json").read_text(encoding="utf-8"))
            for sample in oracle["images"]:
                raw_export = root / (sample["name"] + ".raw")
                requests = [
                    {"id": "open", "command": "image_load", "args": {"path": str(fixtures / sample["name"])}},
                    {"id": "export", "command": "image_export_raw", "args": {"destination": str(raw_export), "taskId": "vhd-export"}},
                ]
                result = run(["session"], "\n".join(json.dumps(request) for request in requests))
                rows = [json.loads(line) for line in result.stdout.splitlines()]
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
        requests = [
            {"id": "open", "command": "image_load", "args": {"path": str(encrypted)}},
            {"id": "unlock", "command": "bitlocker_unlock", "args": {"partitionId": partition, "password": "bde-TEST"}},
            {"id": "status", "command": "bitlocker_status", "args": {"partitionId": partition}},
            {"id": "export", "command": "bitlocker_export_decrypted_image", "args": {"partitionId": partition, "destination": str(decrypted), "taskId": "bde-export"}},
            {"id": "current", "command": "image_export_raw", "args": {"destination": str(current_raw), "taskId": "bde-current", "mode": "unlockedPartitions"}},
        ]
        result = run(["session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert all("result" in row for row in rows), rows
        assert rows[1]["result"]["locked"] is False and rows[2]["result"]["locked"] is False
        assert decrypted.stat().st_size == encrypted.stat().st_size
        assert current_raw.read_bytes() == decrypted.read_bytes()
        with decrypted.open("rb") as stream:
            assert hashlib.sha256(stream.read(512)).hexdigest() == "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3"
            stream.seek(0x8000)
            assert hashlib.sha256(stream.read(4096)).hexdigest() == "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d"

        # Independent cipher projections exercise the new command surface.
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
            requests = [
                {"id": "open", "command": "image_load", "args": {"path": str(encrypted)}},
                {"id": "metadata", "command": "bitlocker_metadata", "args": {"partitionId": partition}},
                {"id": "unlock", "command": "bitlocker_unlock_with_vmk" if "vmkHex" in credential else "bitlocker_unlock", "args": {"partitionId": partition, **credential}},
                {"id": "export", "command": "bitlocker_export_decrypted_image", "args": {"partitionId": partition, "destination": str(decrypted), "taskId": "parity-export"}},
            ]
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert all("result" in row for row in rows), rows
            metadata = rows[1]["result"]
            assert metadata["encryption_method"] == method and metadata["bytes_per_sector"] == sector_size
            assert metadata["protector_types"] == [0, 0x0200]
            if "vmkHex" in credential:
                assert rows[2]["result"]["status"]["locked"] is False
                assert rows[2]["result"]["recovery_key"] == ""
            else:
                assert rows[2]["result"]["locked"] is False
            plain = bytearray((index * 7 + 19) % 256 for index in range(sector_size))
            plain[3:11] = b"NTFS    "
            content = decrypted.read_bytes()
            assert len(content) == len(image) and content[:sector_size] == plain
            assert content[0x12000:0x12000 + sector_size] == plain
            assert content[0x10000:0x10000 + sector_size] == bytes(sector_size)
            assert content[0x20000:0x30000] == bytes(0x10000)

        if capabilities["qrcode"] or capabilities["ocr"]:
            blank = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 256, 64, 8, 2, 0, 0, 0))
            blank += chunk(b"IDAT", zlib.compress((b"\0" + b"\xff" * (256 * 3)) * 64)) + chunk(b"IEND", b"")
            (evidence / "blank.png").write_bytes(blank)
            requests = [{"id": "image", "command": "image_load", "args": {"path": str(evidence)}}]
            image_node = "folder:" + b"blank.png".hex()
            if capabilities["qrcode"]:
                requests.append({"id": "qr", "command": "qr_scan", "args": {"nodeId": image_node, "expectedCount": 1, "taskId": "qr-1"}})
            if capabilities["ocr"]:
                requests.append({"id": "ocr", "command": "ocr_scan", "args": {"nodeId": image_node, "taskId": "ocr-1"}})
            result = run(["session"], "\n".join(json.dumps(request) for request in requests))
            assert all("result" in json.loads(line) for line in result.stdout.splitlines()), result

        # A real case must close cleanly and be reopenable by the next process.
        case = root / "case"
        nested = evidence / "nested"
        nested.mkdir()
        (nested / "report.txt").write_text("Password nested\npassword second\n", encoding="utf-8")
        run(["case-create", "--dir", str(case), "--name", "CLI smoke test"])
        requests = [
            {"id": "image", "command": "image_load", "args": {"path": str(evidence)}},
            {"id": "wait", "command": "wait"},
            {"id": "search", "command": "global_search_start", "args": {"searchId": "cli-search", "options": {"keys": ["password"]}}},
            {"id": "timeline", "command": "timeline_start"},
            {"id": "plugins", "command": "plugin_list"},
        ]
        result = run(["--case", str(case), "session"], "\n".join(json.dumps(request) for request in requests))
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert all("result" in row for row in rows), rows
        assert rows[2]["result"]["total_matches"] >= 1, rows
        # grep uses the same persisted global-search results, with scopes,
        # case handling, regex matching, and a display limit independent of storage.
        grep_script = root / "grep.txt"
        grep_script.write_text(f'''open "{evidence}"
wait
cd "{evidence.name}"
grep Password hello.txt --codepage UTF-8 --search-id case-sensitive
grep -i password nested --codepage UTF-8 --limit 1 --search-id scoped-grep
grep -E 'pass(word)?' hello.txt --codepage UTF-8 --search-id regex-grep
global-search-hits-page --search-id scoped-grep --limit 10
''', encoding="utf-8")
        result = run(["--case", str(case), "--json", "run", str(grep_script)])
        grep_rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert grep_rows[1]["total"] == 0, grep_rows
        scoped = grep_rows[2]
        assert scoped["summary"]["status"] == "success" and scoped["summary"]["search_id"] == "scoped-grep"
        assert scoped["total"] == 2 and len(scoped["hits"]) == 1 and scoped["truncated"], scoped
        assert scoped["hits"][0]["node_id"] == "folder:" + b"nested/report.txt".hex(), scoped
        assert grep_rows[3]["total"] >= 1 and all(hit["pattern_kind"] == "regex" for hit in grep_rows[3]["hits"])
        assert grep_rows[4]["total"] == 2 and len(grep_rows[4]["items"]) == 2, grep_rows
        run(["--image", str(evidence), "grep", "password"], expected=1)

        # Missing source bytes force offline sessions; path resolution and the
        # recursive find must still come entirely from the published case indexes.
        moved_evidence = root / "temporarily absent evidence"
        evidence.rename(moved_evidence)
        offline_script = root / "offline.txt"
        offline_script.write_text(f'''open "{evidence}"
cd "{evidence.name}"
ls nested
find . --name REPORT
cd nested
pwd
''', encoding="utf-8")
        try:
            result = run(["--case", str(case), "--json", "run", str(offline_script)])
            offline_rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert offline_rows[0]["index_only"], offline_rows
            assert [item["node"]["name"] for item in offline_rows[1]["items"]] == ["report.txt"]
            assert [item["path"] for item in offline_rows[2]["items"]] == ["/nested/report.txt"]
            assert offline_rows[3] == f"/{evidence.name}/nested"
            offline_requests = [
                {"id": "open", "command": "open", "args": {"path": str(evidence)}},
                {"id": "cd", "command": "cd", "args": {"path": "/" + evidence.name}},
                {"id": "stat", "command": "stat", "args": {"path": "nested/report.txt"}},
                {"id": "tree", "command": "tree", "args": {"path": "nested"}},
                {"id": "hex", "command": "hex", "args": {"path": "hello.txt"}},
                {"id": "strings", "command": "strings", "args": {"path": "hello.txt"}},
                {"id": "file", "command": "file", "args": {"path": "hello.txt"}},
                {"id": "pwd", "command": "pwd"},
            ]
            result = run(["--case", str(case), "session"], "\n".join(json.dumps(request) for request in offline_requests), expected=1)
            offline_results = {row["id"]: row for row in map(json.loads, result.stdout.splitlines())}
            assert offline_results["stat"]["result"]["index_only"] and offline_results["stat"]["result"]["name"] == "report.txt"
            assert [item["path"] for item in offline_results["tree"]["result"]["items"]] == ["/report.txt"]
            assert all("error" in offline_results[command] for command in ("hex", "strings", "file"))
            assert offline_results["pwd"]["result"] == "/" + evidence.name
            run(["--case", str(case), "--image", str(evidence), "grep", "password"], expected=1)
        finally:
            moved_evidence.rename(evidence)
        run(["--case", str(case), "--json", "case-active"])
        run(["case-remove", "--dir", str(case)])

        if os.name == "nt":
            process = subprocess.Popen([str(executable), "session"], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                       encoding="utf-8", creationflags=subprocess.CREATE_NEW_PROCESS_GROUP, cwd=root)
            try:
                process.stdin.write('{"id":"ready","command":"image_list"}\n')
                process.stdin.flush()
                assert json.loads(process.stdout.readline())["id"] == "ready"
                visible = []
                callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
                ctypes.windll.user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
                ctypes.windll.user32.IsWindowVisible.argtypes = [ctypes.c_void_p]
                ctypes.windll.user32.GetWindowTextW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
                ctypes.windll.user32.GetClassNameW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
                ctypes.windll.user32.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.RECT)]
                ctypes.windll.user32.GetWindowLongPtrW.argtypes = [ctypes.c_void_p, ctypes.c_int]
                ctypes.windll.user32.GetWindowLongPtrW.restype = ctypes.c_ssize_t

                @callback_type
                def inspect_window(window, _):
                    owner = ctypes.c_ulong()
                    ctypes.windll.user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
                    if owner.value == process.pid and ctypes.windll.user32.IsWindowVisible(window):
                        title = ctypes.create_unicode_buffer(512)
                        class_name = ctypes.create_unicode_buffer(512)
                        ctypes.windll.user32.GetWindowTextW(window, title, len(title))
                        ctypes.windll.user32.GetClassNameW(window, class_name, len(class_name))
                        rectangle = wintypes.RECT()
                        ctypes.windll.user32.GetWindowRect(window, ctypes.byref(rectangle))
                        # Tao deliberately sets WS_VISIBLE on its transparent,
                        # layered message target to receive WM_PAINT. Its source
                        # documents that this is never displayed to the user.
                        extended_style = ctypes.windll.user32.GetWindowLongPtrW(window, -20)
                        message_styles = 0x80000 | 0x20 | 0x8000000 | 0x80
                        if class_name.value == "Tao Thread Event Target" and extended_style & message_styles == message_styles:
                            return True
                        visible.append((window, title.value, class_name.value, tuple(getattr(rectangle, field) for field in ("left", "top", "right", "bottom"))))
                    return True

                ctypes.windll.user32.EnumWindows(inspect_window, None)
                assert not visible, f"CLI created a visible desktop window: {visible}"
                process.send_signal(signal.CTRL_BREAK_EVENT)
                assert process.wait(timeout=15) == 130
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=15)

    print("CLI smoke tests passed: catalog, JSONL, browsing, binary read, hash, preview cleanup, export, errors, carving, search, timeline, case lifecycle, windowless startup and interrupt")


if __name__ == "__main__":
    main()
