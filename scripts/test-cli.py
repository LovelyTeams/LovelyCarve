"""End-to-end tests against the standalone CLI process (no desktop runtime, window or web server).

python scripts/test-cli.py <executable> [section ...]   runs every section, or the ones named.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import sqlite3
import struct
import subprocess
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit
import zipfile
import zlib

from cli_backend_tests import test_backend
from cli_container_tests import test_containers, test_encryption
from cli_format_tests import test_formats
from cli_integrity_tests import test_integrity
from cli_navigation_tests import test_navigation
from cli_observability_tests import test_observability
from cli_optical_tests import iso_fixture, test_optical_discs
from cli_output_tests import test_output
from cli_support import Cli, isolated, node, path_id, request, says
from cli_timezone_tests import test_timezones

CONTENT = b"hello forensic world\npassword test\n"


def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


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


def test_discovery(cli, root):
    """What the CLI says about itself needs no evidence: its build, commands,
    help, completion, usage errors and dry runs."""
    system = cli.json("system")
    assert system["standalone"], "the CLI must be built without the desktop runtime"
    assert system["batch"]["maxJobs"] == 64 and system["exitCodes"]["incompleteResult"] == 2, system
    assert system["rpcOperations"] >= 140, system["rpcOperations"]
    catalogue = cli.json("schema")
    commands = [entry["command"] for entry in catalogue["commands"]]
    for expected in ["ls", "cat", "evidence add", "sqlite query", "case open", "unlock", "rpc", "batch"]:
        assert expected in commands, expected
    assert "case serve" not in commands and "__complete" not in commands
    add = cli.json("schema", "evidence", "add")
    password = next(option for option in add["options"] if option["flag"] == "--qcow-password")
    assert password["valueName"] == "@文件" and password["optionalValue"], password
    assert cli.json("schema", "cat")["output"] == "bytes"
    assert cli.json("schema", "case", "rm")["confirm"]
    assert any(item["name"] == "fs_size_tree" for item in cli.json("rpc", "--list")["items"])
    candidates = [line.split("\t")[0] for line in cli.run("__complete", "s").stdout.splitlines()]
    assert {"search", "sqlite", "strings", "stat", "streams"} <= set(candidates), candidates
    assert "serve" not in [line.split("\t")[0] for line in cli.run("__complete", "case", "").stdout.splitlines()]
    for shell in ("bash", "zsh", "fish", "powershell"):
        assert "__complete" in cli.run("completion", shell).stdout, shell
    assert "浏览与读取" in cli.run("--help").stdout and "--offset" in cli.run("cat", "--help").stdout
    assert "schema hash" in cli.run("hash", "--help").stdout
    # Usage errors name the problem and where help is; --json makes them one JSON line.
    assert says(cli.run("lss", "/", expected=1).stderr, "未知命令：lss")
    error = json.loads(cli.run("--json", "ls", "/x", "--limt", "5", expected=1).stderr)["error"]
    assert error["code"] == "usage" and "--limit" in error["message"], error
    cli.run("--unknown", expected=1)
    for jobs in ("0", "65"):
        cli.run("batch", "--jobs", jobs, expected=1)
    # The previous command names are gone, not aliased.
    for removed in (["files", "list"], ["images", "open", "x"], ["sessions", "list"], ["image_load"]):
        assert says(cli.run(*removed, expected=1).stderr, "未知命令"), removed
    # A relative path outside the shell says how to write an address.
    assert says(cli.run("ls", "ev/part:0", expected=1).stderr, "检材内路径须以 / 开头")
    # --dry-run shows the parsed request without touching evidence or reading secrets.
    missing = root / "missing.E01"
    (root / "password.txt").write_text("hunter2", encoding="utf-8")
    preview = cli.json("--dry-run", "evidence", "add", missing, "--qcow-password", "@" + str(root / "password.txt"))
    assert preview["command"] == "evidence add" and preview["executed"] is False, preview
    assert "hunter2" not in json.dumps(preview) and not missing.exists()
    preview = cli.json("--dry-run", "hash", "/x/part:0/a", "--algo", "md5,sha256")
    assert preview["request"]["hash"]["algo"] == ["md5", "sha256"], preview
    # Secrets never come from argv.
    assert says(cli.run("evidence", "add", missing, "--qcow-password", "hunter2", expected=1).stderr, "@文件")
    # Destructive commands are refused where nobody can be asked.
    refused = cli.run("case", "rm", root / "no such case", expected=1)
    assert "Aborted." in refused.stderr, refused.stderr
    # Looking up or cancelling a task that does not exist fails instead of passing for a success.
    missing_task = cli.run("--json", "tasks", "show", "no-such-task", expected=1)
    assert missing_task.stdout == "" and json.loads(missing_task.stderr)["error"]["code"] == "notFound", missing_task
    assert says(cli.run("tasks", "cancel", "no-such-task", expected=1).stderr, "找不到运行中的任务")
    # What lives only as long as a process is refused where it would end at once.
    assert says(cli.run("--image", root, "carve", "start", expected=1).stderr, "case open")
    print("CLI discovery tests passed: system, schema, rpc list, completion, help, usage errors, dry runs and refusals")


def test_batches(cli, root):
    """Batch requests are command lines; queries overlap, everything else is a
    barrier, references wait for what they refer to, and one bad line does
    not stop the others."""
    evidence = root / "evidence"
    evidence.mkdir()
    (evidence / "hello.txt").write_bytes(CONTENT)
    hello = "/ev/folder:/hello.txt"
    requests = [
        request("open", "evidence", "add", evidence, "--as", "ev"),
        request("read", "cat", hello, "--length", "8"),
        request("digest", "hash", hello, "--algo", "sha256"),
        request("stat", "stat", hello),
        request("dependent", "cat", {"$ref": "stat#/path"}, "--length", "8"),
        request("by-id", "cat", node("ev", path_id("folder:", "hello.txt")), "--length", "8"),
        request("bad", "ls", "/ev", "--typo"),
        request("relative", "ls", "ev"),
        {"id": "previous", "command": "images open", "args": {"path": str(evidence)}},
        request("missing-task", "tasks", "show", "no-such-task"),
        request("wait", "tasks", "wait"),
        request("close", "evidence", "close", "ev"),
        request("remaining", "evidence", "ls"),
    ]
    _, serial = cli.batch(requests, expected=1)
    _, parallel = cli.batch(requests, expected=1, jobs=4)
    # A serial batch answers in request order.
    assert [row["id"] for row in serial] == [item["id"] for item in requests], serial
    rows = {row["id"]: row for row in parallel}
    assert len(rows) == len(requests), parallel
    eight = {"encoding": "hex", "data": CONTENT[:8].hex()}
    assert rows["read"]["result"] == rows["dependent"]["result"] == rows["by-id"]["result"] == eight, rows
    assert rows["digest"]["result"]["items"][0]["sha256"] == hashlib.sha256(CONTENT).hexdigest()
    assert rows["remaining"]["result"]["items"] == []
    errors = {"bad", "relative", "previous", "missing-task"}
    assert all("result" in row for key, row in rows.items() if key not in errors), rows
    assert all(rows[key]["error"]["code"] and rows[key]["error"]["message"] for key in errors), rows
    assert rows["bad"]["error"]["code"] == "usage" and rows["missing-task"]["error"]["code"] == "notFound", rows
    assert "argv" in rows["previous"]["error"]["message"], rows["previous"]
    assert isinstance(rows["wait"]["result"]["tasks"], list), rows["wait"]
    serial_rows = {row["id"]: row for row in serial}
    assert all(serial_rows[key] == rows[key] for key in ["read", "digest", "dependent", "remaining"])
    # Results can be saved to new files instead of returned, and never overwrite.
    saved = root / "saved.bin"
    rows = cli.results([request("open", "evidence", "add", evidence, "--as", "ev"),
                        request("save", "cat", hello, output=saved)])
    assert rows["save"] == {"output": str(saved)} and saved.read_bytes() == CONTENT
    _, rows = cli.batch([request("open", "evidence", "add", evidence, "--as", "ev"),
                         request("again", "cat", hello, output=saved)], expected=1)
    assert "error" in rows[1] and saved.read_bytes() == CONTENT, rows
    # Process options belong to the batch process, not to its requests.
    _, rows = cli.batch([request("scoped", "--image", evidence, "ls", "/")], expected=1)
    assert rows[0]["error"]["code"] == "usage", rows

    # Agents keep stdin open and wait for a response before sending more.
    process = subprocess.Popen([str(cli.executable), "--image", str(evidence), "batch", "-j", "4"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, encoding="utf-8", cwd=root, env=cli.environment)
    responses = queue.Queue()
    reader = threading.Thread(target=lambda: responses.put(process.stdout.readline()), daemon=True)
    try:
        process.stdin.write(json.dumps(request("live", "cat", "/evidence/folder:/hello.txt", "--length", "8")) + "\n")
        process.stdin.flush()
        reader.start()
        live = json.loads(responses.get(timeout=60))
        assert live == {"id": "live", "result": eight}, live
        assert process.poll() is None, "batch exited before stdin EOF"
        process.stdin.close()
        process.wait(timeout=60)
        assert process.returncode == 0, process.stderr.read()
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for stream in [process.stdin, process.stdout, process.stderr]:
            stream.close()
        reader.join(timeout=5)

    # Optional private evidence stays outside the repository. Compare actual
    # file system queries and bounded reads between serial and parallel runs.
    real_image = os.environ.get("LOVELYCARVE_CLI_TEST_IMAGE")
    if real_image:
        def real_queries(jobs):
            process = subprocess.Popen([str(cli.executable), "--image", real_image, "batch", "--jobs", str(jobs)],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                       text=True, encoding="utf-8", cwd=root, env=cli.environment)
            replies = queue.Queue()

            def collect():
                for line in process.stdout:
                    replies.put(line)
                replies.put(None)
            thread = threading.Thread(target=collect, daemon=True)
            thread.start()

            def ask(items):
                for item in items:
                    process.stdin.write(json.dumps(item) + "\n")
                process.stdin.flush()
                pending, results = {item["id"] for item in items}, {}
                while pending:
                    line = replies.get(timeout=300)
                    assert line is not None, "real evidence batch ended early"
                    row = json.loads(line)
                    assert row["id"] in pending and "result" in row, row
                    pending.remove(row["id"])
                    results[row["id"]] = row["result"]
                return results

            def listings(paths, prefix):
                return ask([request(f"{prefix}-{index}", "ls", path) for index, path in enumerate(paths)])
            try:
                alias = ask([request("evidence", "evidence", "ls")])["evidence"]["items"][0]["path"]
                volumes = [item["path"] for item in listings([alias], "roots")["roots-0"]["items"] if item["kind"] == "dir"][:4]
                assert volumes, "real evidence has no browsable volumes"
                directories = listings(volumes, "dir")

                def files_in(pages):
                    return sorted((entry for page in pages.values() for entry in page["items"]
                                   if entry["kind"] == "file" and entry["size"] > 0 and not entry["deleted"]),
                                  key=lambda entry: entry["path"])[:4]
                files = files_in(directories)
                nested = sorted(entry["path"] for page in directories.values() for entry in page["items"]
                                if entry["kind"] == "dir" and not entry["deleted"])[:8]
                if not files and nested:
                    nested_pages = listings(nested, "nested")
                    directories.update(nested_pages)
                    files = files_in(nested_pages)
                assert files, "real evidence has no file for bounded read verification"
                reads = ask([request(f"bytes-{index}", "cat", node(alias.strip("/"), entry["id"]), "--length",
                                     min(4096, entry["size"])) for index, entry in enumerate(files)])
                process.stdin.close()
                process.wait(timeout=300)
                assert process.returncode == 0
                return directories, reads
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                thread.join(timeout=5)
                process.stdout.close()
        serial_directories, serial_reads = real_queries(1)
        parallel_directories, parallel_reads = real_queries(4)
        assert parallel_directories == serial_directories, "real evidence directory queries differ under concurrency"
        assert parallel_reads == serial_reads, "real evidence range reads differ under concurrency"
        print(f"Real evidence concurrency passed: {len(serial_directories)} directories, {len(serial_reads)} bounded reads")
    print("CLI batch tests passed: argv requests, references, serial and parallel answers, saved results, open stdin")


def test_browsing(cli, root):
    """Browsing a folder: pages, cursors, trees, properties, ranged reads,
    strings, content types and text, in batches, one-shot and in the shell."""
    evidence = root / "evidence with spaces"
    evidence.mkdir()
    (evidence / "hello.txt").write_bytes(CONTENT)
    python_content = b"#!/usr/bin/env python3\nimport sys\nprint('LovelyCarve forensic CLI')\n" * 20
    (evidence / "signature.bin").write_bytes(python_content)
    unicode_text = "中文取证字符串"
    (evidence / "unicode.bin").write_bytes(b"\0" + unicode_text.encode("utf-8") + b"\0")
    (evidence / "wide.bin").write_bytes(("\0" + unicode_text + "\0").encode("utf-16le"))
    (evidence / "latin1.bin").write_bytes(b"caf\xe9")
    alias = "evidence-with-spaces"
    folder = f"/{alias}/folder:"
    hello = folder + "/hello.txt"
    rows = cli.results([
        request("open", "evidence", "add", evidence),
        request("root", "ls", "/"),
        request("wait", "tasks", "wait"),
        request("page", "ls", folder, "--limit", "2"),
        request("page-by-id", "ls", node(alias, "folder:"), "--limit", "2"),
        request("next", "ls", folder, "--cursor", {"$ref": "page#/nextCursor"}, "--limit", "1000"),
        request("all", "ls", folder, "--all"),
        request("tree", "tree", f"/{alias}", "--depth", "2"),
        request("stat", "stat", hello),
        request("hex", "cat", hello, "--offset", "6", "--length", "8"),
        request("strings", "strings", hello),
        request("utf8", "strings", folder + "/unicode.bin", "--encoding", "utf-8"),
        request("utf16", "strings", folder + "/wide.bin", "--encoding", "utf-16le"),
        request("limited", "strings", hello, "--limit", "1"),
        request("file", "identify", folder + "/signature.bin"),
        request("text", "text", hello),
    ])
    assert rows["open"]["items"][0]["alias"] == alias and rows["root"]["items"][0]["path"] == f"/{alias}", rows["root"]
    assert rows["page"] == rows["page-by-id"], (rows["page"], rows["page-by-id"])
    assert rows["page"]["nextCursor"] and rows["next"]["nextCursor"] is None
    assert rows["page"]["items"] + rows["next"]["items"] == rows["all"]["items"]
    assert all(item["path"].startswith(folder + "/") and "parentId" in item for item in rows["all"]["items"])
    # The same page from a one-shot process; reading a live folder may update
    # host access times in between, so those are left out.
    one_shot = cli.json("--image", evidence, "ls", folder, "--limit", "2")
    for page in (one_shot, rows["page"]):
        for item in page["items"]:
            item["timestamps"].pop("accessed", None)
            item.get("timestampsOriginal", {}).pop("accessed", None)
    assert one_shot == rows["page"], (one_shot, rows["page"])
    assert hello in [item["path"] for item in rows["tree"]["items"]], rows["tree"]
    stat = rows["stat"]
    assert stat["size"] == len(CONTENT) and stat["nodeId"] == path_id("folder:", "hello.txt"), stat
    assert stat["path"] == hello and stat["parent"] == folder and not stat["indexOnly"] and "timestamps" in stat
    assert rows["hex"] == {"encoding": "hex", "data": CONTENT[6:14].hex()}
    assert rows["strings"]["strings"] == [
        {"offset": 0, "text": "hello forensic world", "truncated": False},
        {"offset": CONTENT.index(b"password"), "text": "password test", "truncated": False},
    ], rows["strings"]
    assert rows["utf8"]["strings"] == [{"offset": 1, "text": unicode_text, "truncated": False}]
    assert rows["utf16"]["strings"] == [{"offset": 2, "text": unicode_text, "truncated": False}]
    assert len(rows["limited"]["strings"]) == 1 and rows["limited"]["truncated"]
    assert rows["file"]["label"] == "python" and rows["file"]["inferred"], rows["file"]
    assert rows["file"]["fileLen"] == len(python_content)
    assert rows["text"]["text"] == CONTENT.decode() and rows["text"]["nextOffset"] is None, rows["text"]
    # One-shot reads write the bytes themselves to stdout; text is plain text.
    image = ("--image", evidence)
    assert cli.run(*image, "cat", hello, "--offset", "6", "--length", "8", binary=True).stdout == b"forensic"
    assert cli.run(*image, "cat", folder + "/latin1.bin", binary=True).stdout == b"caf\xe9"
    assert cli.run(*image, "text", hello).stdout == CONTENT.decode()
    assert cli.json(*image, "identify", folder + "/signature.bin")["label"] == "python"
    listing = cli.run(*image, "ls", folder).stdout
    assert "hello.txt" in listing and "{" not in listing, listing
    assert "SHA256 (" + hello + ") = " + hashlib.sha256(CONTENT).hexdigest() in cli.run(*image, "hash", hello).stdout

    # A shell script runs one command per line, from the shell's directory,
    # and stops at the first failure.
    exported = root / "export.txt"
    script = f'''evidence add "{evidence}" --as ev
cd /ev/folder:
pwd
ls
cat hello.txt --length 5
hash hello.txt --algo sha256
export hello.txt -o "{exported}"
'''
    outputs = documents(cli.run("--json", "shell", stdin=script).stdout)
    assert outputs[0]["items"][0]["alias"] == "ev" and outputs[1] == "/ev/folder:", outputs
    assert "hello.txt" in [item["name"] for item in outputs[2]["items"]], outputs
    assert outputs[3] == {"encoding": "hex", "data": b"hello".hex()}, outputs
    assert outputs[4]["items"][0]["sha256"] == hashlib.sha256(CONTENT).hexdigest(), outputs
    assert exported.read_bytes() == CONTENT
    refused = cli.run("shell", stdin=script, expected=1)
    assert says(refused.stderr, "已存在"), refused.stderr
    assert exported.read_bytes() == CONTENT

    # A preview file a command makes for another program is gone when the process ends.
    rows = cli.results([
        request("open", "evidence", "add", evidence, "--as", "ev"),
        request("preview", "rpc", "fs_media_temp", "--body",
                json.dumps({"imageId": "ev", "nodeId": path_id("folder:", "hello.txt"), "ext": "txt", "taskId": "preview-1"})),
        request("wait", "tasks", "wait"),
    ])
    assert isinstance(rows["preview"], str) and rows["preview"].endswith(".txt"), rows["preview"]
    assert not Path(rows["preview"]).exists(), "preview file leaked after the process ended"
    print("CLI browsing tests passed: pages, cursors, tree, stat, ranges, strings, identify, text, shell scripts, preview cleanup")


def test_analysis(cli, root):
    """SQLite with its WAL, and BCP packages published as analysis results."""
    # The path and node ID forms both discover the database's WAL.
    sqlite_evidence = root / "sqlite-evidence"
    sqlite_evidence.mkdir()
    source = sqlite3.connect(root / "chat.db")
    try:
        source.executescript("""
            PRAGMA journal_mode = WAL;
            PRAGMA wal_autocheckpoint = 0;
            CREATE TABLE messages(id INTEGER PRIMARY KEY, chat_id INTEGER);
            INSERT INTO messages VALUES (1, 3), (2, 3), (3, 5);
        """)
        for name in ("chat.db", "chat.db-wal"):
            (sqlite_evidence / name).write_bytes((root / name).read_bytes())
    finally:
        source.close()
    sql = "SELECT chat_id, COUNT(*) FROM messages WHERE id > ?1 GROUP BY chat_id ORDER BY chat_id"
    rows = cli.results([
        request("open", "evidence", "add", sqlite_evidence, "--as", "db"),
        request("first", "sqlite", "query", "/db/folder:/chat.db", sql, "--param", "0", "--limit", "1"),
        request("next", "sqlite", "query", node("db", path_id("folder:", "chat.db")), sql, "--param", "0",
                "--limit", "1", "--offset", "1"),
        request("tables", "sqlite", "tables", "/db/folder:/chat.db"),
        request("rows", "sqlite", "rows", "/db/folder:/chat.db", "messages", "--limit", "2"),
    ])
    for key, values, next_offset in [("first", ("3", "2"), 1), ("next", ("5", "1"), None)]:
        page = rows[key]
        assert page["rows"] == [[{"kind": "integer", "value": value} for value in values]], page
        assert page["sidecars"] == ["chat.db-wal"] and page["nextOffset"] == next_offset, page
        assert page["path"] == "/db/folder:/chat.db", page
    assert "messages" in json.dumps(rows["tables"]), rows["tables"]
    assert len(rows["rows"]["rows"]) == 2, rows["rows"]
    # Writes are refused before they reach the database.
    _, refused = cli.batch([request("open", "evidence", "add", sqlite_evidence, "--as", "db"),
                            request("write", "sqlite", "query", "/db/folder:/chat.db", "DELETE FROM messages")], expected=1)
    assert "error" in refused[1], refused
    iso = root / "sqlite.iso"
    iso.write_bytes(iso_fixture((root / "chat.db").read_bytes())[0])
    counted = cli.json("--image", iso, "sqlite", "query", "/sqlite/part:0/PAYLOAD.BIN", "SELECT COUNT(*) FROM messages")
    assert counted["rows"] == [[{"kind": "integer", "value": "3"}]], counted

    # KeePass: a renamed database is recognized by its signature, its header
    # needs no key, and a password file with a host keyfile decrypts it.
    fixtures = Path(__file__).resolve().parents[1] / "src-tauri" / "vendor" / "kdbx" / "tests" / "data" / "pykeepass"
    vault = root / "vault"
    vault.mkdir()
    (vault / "renamed.bin").write_bytes((fixtures / "test3.kdbx").read_bytes())
    keyfile = root / "test3.key"
    keyfile.write_bytes((fixtures / "test3.key").read_bytes())
    password = root / "kdbx-password.txt"
    password.write_text("password", encoding="utf-8")
    address = "/vault/folder:/renamed.bin"
    header = cli.json("--image", vault, "inspect", address)
    assert (header["format"], header["version"], header["cipher"], header["kdf"]["rounds"]) == ("kdbx", "3.1", "AES-256", 100), header
    opened = cli.json("--image", vault, "inspect", address, "--password", f"@{password}", "--keyfile", keyfile)
    assert opened["counts"] == {"groups": 7, "entries": 15, "history": 18, "attachments": 6}, opened["counts"]
    assert (opened["headerHash"], opened["key"]["keyfile"]) == ("matches", "XML 1.0"), opened
    fields = [{field["key"]: field["value"] for field in entry["fields"]} for entry in opened["root"]["entries"]]
    assert next(entry for entry in fields if entry["Title"] == "root_entry")["Password"] == "passw0rd", fields
    wrong = cli.run("--json", "--image", vault, "inspect", address, "--password", f"@{password}", expected=1)
    assert json.loads(wrong.stderr)["error"]["code"] == "invalidInput", wrong.stderr

    # BCP packages, as a ZIP and as a directory, publish once and reuse the run.
    bcp_directory = root / "bcp export"
    bcp_directory.mkdir()
    bcp_zip = bcp_directory / "evidence.zip"
    last_record = "群组\t结尾\t1030036\towner\t1\n"
    with zipfile.ZipFile(bcp_zip, "w", zipfile.ZIP_DEFLATED) as archive:
        shards = []

        def add_shard(name, code, shard_contents, count, fields):
            archive.writestr(name, shard_contents)
            items = "".join(f'<ITEM key="{index}" eng="{field}" chn="{label}"/>' for index, (field, label) in enumerate(fields, 1))
            shards.append(
                f'<DATA><ITEM key="A010004" val="WA_MFORENSICS_{code}"/><ITEM key="I010039" val="UTF-8无BOM"/>'
                f'<DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="{name}"/>'
                f'<ITEM key="I010034" val="{count}"/></DATA></DATASET>'
                f'<DATASET name="WA_COMMON_010015"><DATA>{items}</DATA></DATASET></DATA>')
        account_fields = [("CONTACT_ACCOUNT_TYPE", "应用类型"), ("ACCOUNT_ID", "账号")]
        group_fields = [("GROUP_NAME", "群组名称"), ("CONTENT", "内容"), *account_fields, ("DELETE_STATUS", "删除状态")]
        add_shard("one.bcp", "020600", "群组\t结尾\t1030063\towner\t0\n" * 100, 100, group_fields)
        add_shard("two.bcp", "020600", last_record, 1, group_fields)
        add_shard("accounts.bcp", "020100",
                  "1039999\tprofile-owner\thttps://i9.taou.com/maimai/photo.png\n"
                  "1039999\tunconfirmed\thttps://example.org/i9.taou.com/maimai/photo.png\n",
                  2, [*account_fields, ("USER_PHOTO", "头像")])
        add_shard("direct.bcp", "020500", "1039999\tprofile-owner\t消息一\n1039999\tunconfirmed\t消息二\n",
                  2, [*account_fields, ("CONTENT", "内容")])
        archive.writestr("GAB_ZIP_INDEX.xml", '<MESSAGE><DATASET name="WA_COMMON_010013">' + "".join(shards) + "</DATASET></MESSAGE>")
    case = root / "bcp case"
    cli.run("case", "new", case, "--name", "BCP import")
    for alias, package in (("bcp-zip", bcp_zip), ("bcp-dir", bcp_directory)):
        scope = ("-C", case)
        cli.run(*scope, "evidence", "add", package, "--as", alias)
        imported = cli.json(*scope, "results", "import", alias)
        assert imported["recordCount"] == 105 and not imported["reused"], imported
        assert cli.json(*scope, "results", "import", alias) == {**imported, "reused": True}
        tree = cli.json(*scope, "results", "ls")
        level = [leaf for item in tree for category in item["children"] for analyzer in category["children"]
                 if analyzer["runId"] == imported["runId"] for leaf in analyzer["children"]]
        by_type = {leaf["artifactType"]: leaf for leaf in level}
        assert {kind: leaf["childCount"] for kind, leaf in by_type.items()} == {
            "bcp.group_messages": 101, "bcp.messaging_accounts": 2, "bcp.direct_messages": 2}, by_type
        for kind in ("bcp.messaging_accounts", "bcp.direct_messages"):
            assert [(group["application"], group["account"], group.get("identifiedApplication"))
                    for group in by_type[kind]["bcpGroups"]] == [
                ("1039999", "profile-owner", "1070109"), ("1039999", "unconfirmed", None)]
        assert [(group["application"], group["account"], group["total"], group["deleted"])
                for group in by_type["bcp.group_messages"]["bcpGroups"]] == [
            ("1030036", "owner", 1, 1), ("1030063", "owner", 100, 0)]
        rows_args = [*scope, "results", "rows", imported["runId"], "bcp.group_messages", "--evidence", alias]
        assert cli.json(*rows_args)["total"] == 101
        page = cli.json(*rows_args, "--keyword", "结尾", "--bcp-app", "1030036", "--bcp-account", "owner")
        assert page["total"] == 1, page
        record = page["items"][0]
        assert record["values"] == {"group_name": "群组", "content": "结尾", "contact_account_type": "1030036",
                                    "account_id": "owner", "delete_status": "1"}, record
        origin = record["sources"][0]
        if package == bcp_zip:
            assert origin == {"root": "root", "path": "two.bcp", "offset": 0, "length": len(last_record.encode())}, origin
        else:
            assert origin["path"] == bcp_zip.name and "two.bcp" in origin["note"], origin
        assert cli.json(*scope, "results", "show", record["recordId"], "--evidence", alias)["record"] == record
    missing = cli.run("--json", "-C", case, "results", "show", "no-such-record", "--evidence", "bcp-zip", expected=1)
    assert json.loads(missing.stderr)["error"]["code"] == "notFound", missing.stderr
    print("CLI analysis tests passed: SQLite WAL by path and node ID, read-only queries, ISO members, KeePass KDBX, BCP import and results")


def test_carving(cli, root):
    """Signature carving and file system recovery run as scans in the
    background of a batch, and export what they found."""
    png = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
    png += png_chunk(b"IDAT", zlib.compress(b"\0\xff\0\0")) + png_chunk(b"IEND", b"")
    disk = root / "disk.img"
    disk.write_bytes((b"\0" * 512 + png).ljust(16384, b"\0"))
    recovered = root / "recovered"
    rows = cli.results([
        request("open", "evidence", "add", disk, "--as", "disk"),
        request("scan", "carve", "start", "--types", "png"),
        request("wait", "tasks", "wait"),
        request("status", "carve", "status", {"$ref": "scan#/scanId"}),
        request("results", "carve", "results", {"$ref": "scan#/scanId"}),
        request("recover", "carve", "export", {"$ref": "scan#/scanId"}, "-o", recovered),
        request("tasks", "tasks", "ls"),
        request("scans", "carve", "ls"),
    ])
    assert rows["status"]["status"] == "completed" and rows["status"]["artifactsFound"] == 1, rows["status"]
    # tasks wait reports the scan that kept running after its command returned.
    assert [task["status"] for task in rows["wait"]["tasks"] if task["kind"] == "carveScan"] == ["success"], rows["wait"]
    assert rows["recover"]["recovered"] == 1, rows["recover"]
    assert next(recovered.glob("*.png")).read_bytes() == png
    entries = {entry.get("kind"): entry for entry in rows["tasks"]["items"]}
    scan = entries["carveScan"]
    assert scan["source"] == "builtin" and scan["status"] == "success" and scan["scanId"] == rows["scan"]["scanId"], scan
    assert scan["view"]["counts"]["found"] == 1, scan
    recovery = entries["carveRecovery"]
    assert recovery["status"] == "success" and recovery["view"]["subject"] == [str(recovered)], recovery
    assert recovery["view"]["counts"]["files"] == 1, recovery
    assert any(item["scanId"] == rows["scan"]["scanId"] for item in rows["scans"]["items"]), rows["scans"]
    artifact = rows["results"]["items"][0]["id"]
    cut = cli.results([request("open", "evidence", "add", disk, "--as", "disk"),
                       request("scan", "carve", "start", "--types", "png"),
                       request("wait", "tasks", "wait"),
                       request("cat", "carve", "cat", {"$ref": "scan#/scanId"}, artifact)])
    assert cut["cat"] == {"encoding": "hex", "data": png.hex()}, cut["cat"]

    # File system recovery shares the scan: a deleted FAT12 entry is listed and
    # exported by its candidate number.
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
    rows = cli.results([
        request("open", "evidence", "add", deleted_image, "--as", "fat"),
        request("scan", "carve", "start", "--no-signatures", "--fs", "fat"),
        request("wait", "tasks", "wait"),
        request("status", "carve", "status", {"$ref": "scan#/scanId"}),
        request("files", "carve", "results", {"$ref": "scan#/scanId"}, "--fs"),
        request("recover", "carve", "export", {"$ref": "scan#/scanId"}, "-o", restored, "--no-artifacts", "--files", "0"),
    ])
    assert rows["status"]["status"] == "completed" and rows["status"]["filesystem"]["files"] == 1, rows["status"]
    candidate = rows["files"]["candidates"][0]
    assert candidate["node"]["deleted"] and candidate["readable"], rows["files"]
    exported = rows["recover"]["files"]
    assert rows["recover"]["recovered"] == 0 and exported["recovered"] == 1, rows["recover"]
    # Recovered files keep their folders under one folder per volume.
    export_root = Path(exported["directory"])
    outputs = sorted(path for path in export_root.rglob("*") if path.is_file() and path.name != "recovery-report.jsonl")
    assert [path.read_bytes() for path in outputs] == [b"hello"], outputs
    relative = outputs[0].relative_to(export_root).parts
    assert len(relative) == 2 and relative[0].startswith("v0-"), relative
    report = [json.loads(line) for line in Path(exported["reportPath"]).read_text(encoding="utf-8").splitlines()]
    assert report[1]["relative_output"] == "/".join(relative), report
    print("CLI carving tests passed: PNG carving, scan tasks, carve cat, FAT record recovery and export layout")


def link_fields(link):
    """The parameters of a lovelycarve://open link."""
    parts = urlsplit(link)
    assert (parts.scheme, parts.netloc, parts.path) == ("lovelycarve", "open", ""), link
    return {key: values[0] for key, values in parse_qs(parts.query, strict_parsing=True).items()}


def test_cases(cli, root):
    """A case outlives its processes: indexes, searches and timelines persist,
    and evidence that went missing stays browsable from the indexes."""
    evidence = root / "evidence"
    nested = evidence / "nested"
    nested.mkdir(parents=True)
    (evidence / "hello.txt").write_bytes(CONTENT)
    (nested / "report.txt").write_text("Password nested\npassword second\n", encoding="utf-8")
    case = root / "case"
    created = cli.json("case", "new", case, "--name", "CLI smoke test")
    assert created["name"] == "CLI smoke test", created
    scope = ("-C", case)
    hello = "/ev/folder:/hello.txt"
    rows = cli.results([
        request("image", "evidence", "add", evidence, "--as", "ev", "--wait"),
        request("search", "grep", "password", "/ev"),
        request("timeline", "timeline", "build"),
        request("plugins", "plugin", "ls"),
        request("volumes", "volumes"),
    ], *scope)
    assert rows["image"]["indexes"] and all(index["state"] == "ready" for index in rows["image"]["indexes"]), rows["image"]
    assert rows["search"]["total"] >= 1, rows["search"]
    assert rows["volumes"]["items"][0]["index"] == "ready", rows["volumes"]
    # grep searches through the global search, saved in the case, with scopes,
    # case matching, regular expressions and a display limit apart from storage.
    rows = cli.results([
        request("wait", "tasks", "wait"),
        request("case-sensitive", "grep", "Password", hello, "--encoding", "UTF-8"),
        request("scoped", "grep", "password", "/ev/folder:/nested", "-i", "--encoding", "UTF-8", "--limit", "1"),
        request("regex", "grep", "pass(word)?", hello, "-E", "--encoding", "UTF-8"),
        request("hits", "search", "hits", {"$ref": "scoped#/items/0/searchId"}, "--limit", "10"),
        request("find", "find", "*.TXT", "--under", "/ev/folder:/nested"),
        request("saved", "search", "ls"),
    ], *scope)
    assert rows["case-sensitive"]["total"] == 0, rows["case-sensitive"]
    scoped = rows["scoped"]
    assert scoped["searches"][0]["status"] == "success", scoped
    assert scoped["total"] == 2 and len(scoped["items"]) == 1 and scoped["truncated"], scoped
    assert scoped["items"][0]["path"] == "/ev/folder:/nested/report.txt", scoped
    assert rows["regex"]["total"] >= 1, rows["regex"]
    assert rows["hits"]["total"] == 2 and len(rows["hits"]["items"]) == 2, rows["hits"]
    assert [item["path"] for item in rows["find"]["items"]] == ["/ev/folder:/nested/report.txt"], rows["find"]
    assert len(rows["saved"]["items"]) >= 4, rows["saved"]
    # grep needs a case to keep its results in.
    assert says(cli.run("--image", evidence, "grep", "password", expected=1).stderr, "案件")

    # Links name the case by its uid and nodes by ID, with the bytes to highlight.
    rows = cli.results([
        request("case", "case", "link"),
        request("root", "case", "link", "/ev"),
        request("file", "case", "link", hello, "--offset", "6", "--length", "8"),
        request("stat", "stat", hello),
        request("evidence", "evidence", "ls"),
    ], *scope)
    image_id = rows["evidence"]["items"][0]["imageId"]
    assert link_fields(rows["case"]["link"]) == {"case": created["id"]}, rows["case"]
    assert link_fields(rows["root"]["link"]) == {"case": created["id"], "evidence": image_id}, rows["root"]
    assert link_fields(rows["file"]["link"]) == {
        "case": created["id"], "evidence": image_id, "node": rows["stat"]["nodeId"], "offset": "6", "length": "8",
    }, rows["file"]
    assert rows["file"]["path"] == hello and rows["file"]["length"] == 8, rows["file"]
    assert cli.run(*scope, "case", "link", hello).stdout.strip() == rows["file"]["link"].split("&offset=")[0]
    assert says(cli.run("--image", evidence, "case", "link", hello, expected=1).stderr, "案件")
    assert says(cli.run(*scope, "case", "link", "/ev", "--offset", "1", expected=1).stderr, "--offset")

    # Evidence that went missing stays browsable from the case's indexes; its
    # bytes fail explicitly.
    moved = root / "temporarily absent evidence"
    evidence.rename(moved)
    try:
        rows = cli.results([
            request("list", "ls", "/ev/folder:/nested"),
            request("tree", "tree", "/ev/folder:", "--depth", "128", "--name", "REPORT"),
            request("stat", "stat", "/ev/folder:/nested/report.txt"),
            request("evidence", "evidence", "ls"),
        ], *scope)
        listed = rows["evidence"]["items"][0]
        assert listed["loaded"] and listed["indexOnly"], listed
        assert [item["name"] for item in rows["list"]["items"]] == ["report.txt"]
        assert [item["path"] for item in rows["tree"]["items"]] == ["/ev/folder:/nested/report.txt"], rows["tree"]
        assert rows["stat"]["indexOnly"] and rows["stat"]["name"] == "report.txt"
        _, failed = cli.batch([request("bytes", "cat", hello), request("strings", "strings", hello),
                               request("identify", "identify", hello), request("grep", "grep", "password", "/ev")],
                              *scope, expected=1)
        assert all("error" in row for row in failed), failed
        shell = cli.run(*scope, "shell", stdin="cd /ev/folder:/nested\npwd\nls\n")
        assert shell.stdout.splitlines()[0] == "/ev/folder:/nested" and "report.txt" in shell.stdout, shell.stdout
    finally:
        moved.rename(evidence)
    status = cli.json(*scope, "case")
    assert status["case"]["name"] == "CLI smoke test" and status["evidence"][0]["alias"] == "ev", status
    assert any(item["dir"] == str(case) for item in cli.json("case", "ls")["items"])
    cli.run("case", "rm", case, "-y")
    assert not (case / "case.db").exists()
    print("CLI case tests passed: indexes, grep scopes, saved searches, find, timeline, offline browsing, status and removal")


def test_process(cli, root):
    """Optional QR/OCR, and on Windows: no window, no elevation, and Ctrl+Break ends with 130."""
    system = cli.json("system")
    if system["qrcode"] or system["ocr"]:
        evidence = root / "images"
        evidence.mkdir()
        blank = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 256, 64, 8, 2, 0, 0, 0))
        blank += png_chunk(b"IDAT", zlib.compress((b"\0" + b"\xff" * (256 * 3)) * 64)) + png_chunk(b"IEND", b"")
        (evidence / "blank.png").write_bytes(blank)
        requests = [request("open", "evidence", "add", evidence, "--as", "ev")]
        if system["qrcode"]:
            requests.append(request("qr", "qr", "/ev/folder:/blank.png", "--expect", "1"))
        if system["ocr"]:
            requests.append(request("ocr", "ocr", "/ev/folder:/blank.png"))
        cli.results(requests)
    if os.name != "nt":
        print("CLI process tests passed: optional image recognition")
        return
    binary = cli.executable.read_bytes()
    assert b"WebView2Loader" not in binary, "the CLI links the desktop WebView"
    assert b"requireAdministrator" not in binary, "the CLI requests elevation"
    process = subprocess.Popen([str(cli.executable), "batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, encoding="utf-8",
                               creationflags=subprocess.CREATE_NEW_PROCESS_GROUP, cwd=root, env=cli.environment)
    try:
        process.stdin.write(json.dumps(request("ready", "evidence", "ls")) + "\n")
        process.stdin.flush()
        assert json.loads(process.stdout.readline())["id"] == "ready"
        windows = []
        callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
        user32 = ctypes.windll.user32
        user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
        user32.GetWindowTextW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
        user32.GetClassNameW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
        user32.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.RECT)]

        @callback_type
        def inspect_window(window, _):
            owner = ctypes.c_ulong()
            user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
            if owner.value == process.pid:
                title = ctypes.create_unicode_buffer(512)
                class_name = ctypes.create_unicode_buffer(512)
                user32.GetWindowTextW(window, title, len(title))
                user32.GetClassNameW(window, class_name, len(class_name))
                windows.append((title.value, class_name.value))
            return True

        user32.EnumWindows(inspect_window, None)
        assert not windows, f"CLI created a window: {windows}"
        process.send_signal(signal.CTRL_BREAK_EVENT)
        assert process.wait(timeout=30) == 130
    finally:
        if process.poll() is None:
            process.kill()
        process.communicate(timeout=30)
    print("CLI process tests passed: optional image recognition, windowless startup, no elevation, interrupt status")


SECTIONS = {
    "discovery": test_discovery,
    "formats": test_formats,
    "containers": test_containers,
    "encryption": test_encryption,
    "batch": test_batches,
    "browsing": test_browsing,
    "analysis": test_analysis,
    "carving": test_carving,
    "cases": test_cases,
    "backend": test_backend,
    "navigation": test_navigation,
    "output": test_output,
    "observability": test_observability,
    "timezones": test_timezones,
    "integrity": test_integrity,
    "optical": test_optical_discs,
    "process": test_process,
}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("executable", type=Path)
    parser.add_argument("sections", nargs="*", metavar="section", help="any of: " + ", ".join(SECTIONS))
    options = parser.parse_args()
    unknown = [name for name in options.sections if name not in SECTIONS]
    if unknown:
        parser.error("unknown sections: " + ", ".join(unknown))
    executable = options.executable.resolve()
    for name in options.sections or SECTIONS:
        with tempfile.TemporaryDirectory(prefix=f"lovelycarve-cli-{name}-") as directory:
            root = Path(directory)
            SECTIONS[name](Cli(executable, root, isolated(os.environ, root)), root)
    print("CLI end-to-end tests passed: " + ", ".join(options.sections or SECTIONS))


if __name__ == "__main__":
    main()
