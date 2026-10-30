"""Caseless node IDs and image paths through the actual CLI transports."""
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import zipfile


def test_navigation(executable, environment):
    fixture = Path(__file__).resolve().parents[1] / "src-tauri/vendor/ewf/tests/data/ntfs_sample.E01"
    with tempfile.TemporaryDirectory(prefix="lovelycarve-navigation-") as directory:
        def run(*arguments, stdin=None, expected=0):
            result = subprocess.run([str(executable), "--quiet", *map(str, arguments)],
                                    input=stdin, text=True, encoding="utf-8", capture_output=True,
                                    cwd=directory, env=environment, timeout=120)
            assert result.returncode == expected, (arguments, result.stdout, result.stderr)
            return result.stdout

        scope = ("--image", fixture)
        roots = json.loads(run(*scope, "files", "list"))
        volume = next(item for item in roots["items"] if item["id"] == "part:0")
        prefix = "/" + volume["name"]
        children = json.loads(run(*scope, "files", "list", "--parent-id", volume["id"]))
        file = next(item for item in children["items"] if item["name"] == "file1.txt")
        alias = "/" + volume["id"]
        for arguments in [("--node-id", file["id"]), (prefix + "/FILE1.TXT",),
                          (alias + "/FILE1.TXT",)]:
            found = json.loads(run(*scope, "files", "get", *arguments))
            assert found["id"] == file["id"] and found["path"] == prefix + "/file1.txt", found
        tree = json.loads(run(*scope, "files", "tree", "list", "--path", prefix, "--name", "file1"))
        assert tree["items"][0]["path"] == prefix + "/file1.txt", tree
        assert json.loads(run(*scope, "files", "get", tree["items"][0]["path"]))["id"] == file["id"]
        aliased_tree = json.loads(run(*scope, "files", "tree", "list", "--path", alias, "--name", "file1"))
        assert aliased_tree["items"] == tree["items"], aliased_tree
        assert run(*scope, "files", "read", alias + "/file1.txt") == run(*scope, "files", "read", "--node-id", file["id"])
        assert json.loads(run(*scope, "files", "list", "--path", alias)) == children
        run(*scope, "shell", stdin=f'cd {alias}\nfiles get FILE1.TXT\nexit\n')
        run(*scope, "shell", stdin=f'cd --node-id {volume["id"]}\nfiles get FILE1.TXT\nexit\n')
        run(*scope, "context", "set", "--dir", "/", "--node-id", volume["id"], expected=1)
        run(*scope, "context", "set", "--node-id", file["id"], expected=1)
        run(*scope, "files", "get", "--node-id", "ntfs:part:0:99999999", expected=1)
        requests = [
            {"id": "cd", "command": "context set", "args": {"nodeId": volume["id"]}},
            {"id": "get", "command": "files get", "args": {"nodeId": file["id"]}},
            {"id": "relative", "command": "files get", "args": {"path": "FILE1.TXT"}},
            {"id": "parent", "command": "context set", "args": {"dir": ".."}},
            {"id": "alias", "command": "files get", "args": {"path": alias + "/file1.txt"}},
            {"id": "cd-alias", "command": "context set", "args": {"dir": alias}},
        ]
        rows = [json.loads(line) for line in run(*scope, "session", stdin="\n".join(map(json.dumps, requests))).splitlines()]
        assert all("result" in row for row in rows), rows
        assert rows[0]["result"]["nodeId"] == volume["id"]
        assert rows[1]["result"]["id"] == rows[2]["result"]["id"] == file["id"]
        assert rows[3]["result"]["dir"] == "/"
        assert rows[4]["result"]["id"] == file["id"]
        assert rows[5]["result"]["nodeId"] == volume["id"]
        source = Path(directory) / "数据卷 (Archive) · 备份"
        source.mkdir()
        filename = "真实 文件 (A) · B.txt"
        (source / filename).write_text("original evidence bytes", encoding="utf-8")
        folder_scope = ("--image", source)
        root = json.loads(run(*folder_scope, "files", "list"))["items"][0]
        assert root["name"] == "数据卷_Archive_备份", root
        path = "/" + root["name"] + "/" + filename
        node = json.loads(run(*folder_scope, "files", "get", path))
        assert node["name"] == filename and node["path"] == path, node
        assert run(*folder_scope, "files", "read", path) == "original evidence bytes"
        assert (source / filename).is_file()
        test_logical_source_lookup(executable, environment, Path(directory))
        test_links_spellings_and_text_pages(executable, environment, Path(directory))
    print("CLI navigation tests passed: caseless IDs, volume ID paths, shell/JSONL directory IDs, NTFS case fallback and absolute tree paths")


def test_logical_source_lookup(executable, environment, root):
    """Plugin paths start inside the logical source, preserving its navigation frame."""
    source = root / "logical evidence"
    (source / "data/system").mkdir(parents=True)
    content = b"<packages/>"
    (source / "data/system/packages.xml").write_bytes(content)
    archive = root / "logical.tar"
    with tarfile.open(archive, "w") as output:
        output.add(source / "data", arcname="data")
    zipped = root / "logical.zip"
    with zipfile.ZipFile(zipped, "w", zipfile.ZIP_DEFLATED) as output:
        output.write(source / "data/system/packages.xml", "data/system/packages.xml")

    for evidence in [source, archive, zipped]:
        case = root / (evidence.name + "-case")

        def session(requests):
            result = subprocess.run([str(executable), "--quiet", "--case", str(case), "session"],
                                    input="\n".join(map(json.dumps, requests)), text=True,
                                    encoding="utf-8", capture_output=True, cwd=root,
                                    env=environment, timeout=120)
            assert result.returncode == 0, (result.stdout, result.stderr)
            rows = {row["id"]: row for row in map(json.loads, result.stdout.splitlines())}
            assert all("result" in row for row in rows.values()), rows
            return {key: row["result"] for key, row in rows.items()}

        created = subprocess.run([str(executable), "--quiet", "cases", "create", str(case), "--name", "Lookup"],
                                 text=True, encoding="utf-8", capture_output=True,
                                 cwd=root, env=environment, timeout=120)
        assert created.returncode == 0, (created.stdout, created.stderr)
        requests = [
            {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "file", "command": "files lookup", "args": {"rootId": "root", "path": "data/system/packages.xml"}},
            {"id": "directory", "command": "files lookup", "args": {"rootId": "root", "path": "/data/system"}},
            {"id": "root", "command": "files lookup", "args": {"rootId": "root", "path": "/"}},
            {"id": "wait", "command": "tasks wait"},
        ]
        live = session(requests)
        assert [frame["name"] for frame in live["file"]["frames"]][1:] == ["data", "system"], live
        assert live["file"]["fileId"] == live["file"]["nodeId"]
        assert live["directory"]["frames"] == live["file"]["frames"]
        assert live["directory"]["fileId"] is None
        assert live["root"]["frames"] == live["file"]["frames"][:1]
        assert live["root"]["nodeId"] == live["file"]["frames"][0]["id"]
        missing = root / (evidence.name + "-absent")
        evidence.rename(missing)
        try:
            offline = session(requests)
            assert offline["open"]["indexOnly"], offline
            for key in ["file", "directory", "root"]:
                assert offline[key] == live[key], (key, offline[key], live[key])
        finally:
            missing.rename(evidence)


def test_links_spellings_and_text_pages(executable, environment, root):
    """Symbolic links, one spelling for paths and node IDs, text pages, export hints and MSYS paths."""
    def process(*arguments, expected=0):
        result = subprocess.run([str(executable), "--quiet", *map(str, arguments)],
                                text=True, encoding="utf-8", capture_output=True,
                                cwd=root, env=environment, timeout=120)
        assert result.returncode == expected, (arguments, result.returncode, result.stdout, result.stderr)
        return result

    def data(*arguments):
        return json.loads(process(*arguments).stdout)

    def says(stderr, text):
        """Whether stderr says `text`: in an agent's JSON error line, or however a
        framed error block wrapped it."""
        def message(line):
            try:
                record = json.loads(line)
            except ValueError:
                return line
            body = (record.get("error") or record.get("warning")) if isinstance(record, dict) else None
            return body["message"] if isinstance(body, dict) else line
        flat = "".join(map(message, stderr.splitlines()))
        flat = "".join(flat.replace("│", "").replace("┌", "").replace("└", "").split())
        return "".join(text.split()) in flat

    release = b"ID=fixture\n"
    tool = b"#!/bin/sh\necho tool\n"
    line = "2024-06-01 12:00:00 用户登录成功，来源地址 10.0.0.8，账号：测试\n"
    log = line * 200
    encoded = log.encode("gbk")
    archive = root / "links.tar"
    with tarfile.open(archive, "w") as output:
        for name in ["etc", "usr", "usr/lib", "usr/bin", "var", "var/log"]:
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.DIRTYPE
            output.addfile(entry)
        for name, content in [("usr/lib/os-release", release), ("usr/bin/tool", tool), ("var/log/app.log", encoded)]:
            entry = tarfile.TarInfo(name)
            entry.size = len(content)
            output.addfile(entry, io.BytesIO(content))
        for name, target in [("etc/os-release", "../usr/lib/os-release"), ("bin", "usr/bin"),
                             ("etc/absolute", "/usr/lib/os-release"), ("etc/loop", "loop"),
                             ("etc/escape", "../../../../usr/lib/os-release")]:
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.SYMTYPE
            entry.linkname = target
            output.addfile(entry)
    scope = ("--image", archive)
    top = data(*scope, "files", "list")["items"][0]
    prefix = "/" + top["name"]
    for path in ["/etc/os-release", "/etc/absolute", "/etc/escape", "/usr/lib/os-release"]:
        assert process(*scope, "files", "read", prefix + path).stdout == release.decode(), path
    assert process(*scope, "files", "read", prefix + "/bin/tool").stdout == tool.decode()
    link = data(*scope, "files", "get", "--path", prefix + "/etc/os-release")
    assert link["kind"] == "symlink" and link["linkTarget"] == "../usr/lib/os-release", link
    through = data(*scope, "files", "get", prefix + "/bin/tool")
    assert through["path"] == prefix + "/usr/bin/tool", through
    digest = data(*scope, "files", "hash", "--path", prefix + "/etc/os-release", "--algos", "sha256")
    assert digest["bytesHashed"] == len(release), digest
    listed = data(*scope, "files", "list", "--path", prefix + "/bin")
    assert [item["name"] for item in listed["items"]] == ["tool"], listed
    usr = next(item for item in data(*scope, "files", "list", "--path", prefix)["items"] if item["name"] == "usr")
    assert {item["name"] for item in data(*scope, "files", "list", "--node-id", usr["id"])["items"]} == {"lib", "bin"}
    assert says(process(*scope, "files", "read", prefix + "/etc/loop", expected=1).stderr, "成环")
    missing = process(*scope, "files", "get", prefix + "/etc/missing", expected=1).stderr
    assert says(missing, "「missing」") and says(missing, prefix + "/etc"), missing
    hint = process(*scope, "files", "list", prefix, expected=1).stderr
    assert says(hint, f"不接受位置参数 {prefix}：镜像内路径写成 --path {prefix}"), hint

    # Pages of decoded text join to the whole file at character boundaries.
    cut = next(offset for offset in range(4000, len(encoded)) if encoded[offset] >= 0x81
               and len(encoded[:offset].decode("gbk", "ignore").encode("gbk")) == offset)
    first = data(*scope, "files", "text", "get", prefix + "/var/log/app.log", "--max-bytes", cut + 1)
    assert first["encoding"] == "GBK" and not first["hadErrors"], {key: first[key] for key in first if key != "text"}
    assert first["offset"] == 0 and first["nextOffset"] == cut and first["text"] == encoded[:cut].decode("gbk")
    pages, offset = [], 0
    while offset is not None:
        page = data(*scope, "files", "text", "get", prefix + "/var/log/app.log", "--max-bytes", 997,
                    "--offset", offset, "--encoding", "gbk")
        assert page["offset"] == offset and not page["hadErrors"], page["offset"]
        pages.append(page["text"])
        offset = page["nextOffset"]
    assert "".join(pages) == log
    detected = data(*scope, "files", "text", "get", prefix + "/var/log/app.log", "--max-bytes", 997, "--offset", 997)
    assert detected["encoding"] == "GBK", detected["encoding"]
    process(*scope, "files", "text", "get", prefix + "/var/log/app.log", "--encoding", "no-such", expected=1)

    # Exports never overwrite and point a directory to the directory export.
    destination = root / "exported-release"
    process(*scope, "files", "export", prefix + "/etc/os-release", "--destination", destination)
    assert destination.read_bytes() == release
    again = process(*scope, "files", "export", prefix + "/etc/os-release", "--destination", destination, expected=1)
    assert says(again.stderr, "导出从不覆盖已有文件") and destination.read_bytes() == release, again.stderr
    directory = process(*scope, "files", "export", prefix + "/usr", "--destination", root / "usr-export", expected=1)
    assert says(directory.stderr, f'files directories export "{prefix}/usr" --mode tree'), directory.stderr
    assert not (root / "usr-export").exists()

    # A path Git Bash rewrote below an MSYS installation is turned back.
    if os.name == "nt":
        msys = root / "FakeGit"
        (msys / "usr" / "bin").mkdir(parents=True)
        (msys / "usr" / "bin" / "msys-2.0.dll").write_bytes(b"")
        rewritten = str(msys).replace("\\", "/") + prefix + "/etc/os-release"
        assert process(*scope, "files", "read", rewritten).stdout == release.decode()
        elsewhere = str(root).replace("\\", "/") + prefix + "/etc/os-release"
        assert says(process(*scope, "files", "read", elsewhere, expected=1).stderr, "MSYS2_ARG_CONV_EXCL")
