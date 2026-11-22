"""Addresses through the actual CLI: root IDs and names, node IDs, NTFS case
fallback, links, text pages, exports and paths Git Bash rewrote."""
import io
import json
import os
import tarfile
import zipfile

from cli_support import REPOSITORY, node, request, says


def test_navigation(cli, root):
    fixture = REPOSITORY / "src-tauri/vendor/ewf/tests/data/ntfs_sample.E01"
    scope = ("--image", fixture)
    volumes = cli.json(*scope, "ls", "/ntfs_sample")
    volume = next(item for item in volumes["items"] if item["id"] == "part:0")
    assert volume["path"] == "/ntfs_sample/part:0", volume
    named = "/ntfs_sample/" + volume["name"]
    children = cli.json(*scope, "ls", volume["path"])
    file = next(item for item in children["items"] if item["name"] == "file1.txt")
    assert file["path"] == volume["path"] + "/file1.txt", file
    # One file, three spellings: its node ID, the volume's name with another
    # letter case below it, and the volume's ID.
    for address in [node("ntfs_sample", file["id"]), named + "/FILE1.TXT", volume["path"] + "/FILE1.TXT"]:
        found = cli.json(*scope, "stat", address)
        assert found["id"] == file["id"] and found["path"] == file["path"], (address, found)
    tree = cli.json(*scope, "tree", volume["path"], "--name", "file1")
    assert tree["items"][0]["path"] == file["path"], tree
    assert cli.json(*scope, "tree", named, "--name", "file1")["items"] == tree["items"]
    assert cli.json(*scope, "tree", "/ntfs_sample", "--depth", "2", "--name", "file1")["items"][0]["path"] == file["path"]
    assert cli.run(*scope, "cat", file["path"], binary=True).stdout == cli.run(*scope, "cat", node("ntfs_sample", file["id"]), binary=True).stdout
    assert cli.json(*scope, "ls", named) == children
    shell = cli.run(*scope, "shell", stdin=f"cd {volume['path']}\nstat FILE1.TXT\ncd ..\npwd\nexit\n")
    assert "file1.txt" in shell.stdout and shell.stdout.rstrip().endswith("/ntfs_sample"), shell.stdout
    assert says(cli.run(*scope, "shell", stdin=f"cd {file['path']}\n", expected=1).stderr, "不是目录")
    cli.run(*scope, "stat", node("ntfs_sample", "ntfs:part:0:99999999"), expected=1)
    # Relative addresses belong to the shell.
    _, rows = cli.batch([request("relative", "stat", "FILE1.TXT")], *scope, expected=1)
    assert rows[0]["error"]["code"] == "usage", rows

    # Names with spaces, CJK and punctuation stay exact in addresses.
    source = root / "数据卷 (Archive) · 备份"
    source.mkdir()
    filename = "真实 文件 (A) · B.txt"
    (source / filename).write_text("original evidence bytes", encoding="utf-8")
    folder_scope = ("--image", source)
    evidence = cli.json(*folder_scope, "ls", "/")["items"][0]
    folder_root = cli.json(*folder_scope, "ls", evidence["path"])["items"][0]
    assert folder_root["name"] == "数据卷_Archive_备份" and folder_root["id"] == "folder:", folder_root
    path = folder_root["path"] + "/" + filename
    found = cli.json(*folder_scope, "stat", path)
    assert found["name"] == filename and found["path"] == path, found
    assert cli.json(*folder_scope, "stat", evidence["path"] + "/" + folder_root["name"] + "/" + filename)["id"] == found["id"]
    assert cli.run(*folder_scope, "cat", path).stdout == "original evidence bytes"
    assert (source / filename).is_file()
    test_logical_source_lookup(cli, root)
    test_links_spellings_and_text_pages(cli, root)
    print("CLI navigation tests passed: root IDs and names, node IDs, NTFS case fallback, shell directories, "
          "logical source lookup, links, text pages, exports and MSYS paths")


def test_logical_source_lookup(cli, root):
    """Plugin paths start inside the logical source, live and from the case indexes."""
    source = root / "logical evidence"
    (source / "data/system").mkdir(parents=True)
    (source / "data/system/packages.xml").write_bytes(b"<packages/>")
    archive = root / "logical.tar"
    with tarfile.open(archive, "w") as output:
        output.add(source / "data", arcname="data")
    zipped = root / "logical.zip"
    with zipfile.ZipFile(zipped, "w", zipfile.ZIP_DEFLATED) as output:
        output.write(source / "data/system/packages.xml", "data/system/packages.xml")

    def locate(identifier, path):
        body = json.dumps({"imageId": "src", "rootId": "root", "path": path})
        return request(identifier, "rpc", "fs_source_locate", "--body", body)

    for evidence in [source, archive, zipped]:
        case = root / (evidence.name + "-case")
        cli.run("case", "new", case, "--name", "Lookup")
        requests = [
            request("open", "evidence", "add", evidence, "--as", "src"),
            locate("file", "data/system/packages.xml"),
            locate("directory", "/data/system"),
            locate("root", "/"),
            request("wait", "tasks", "wait"),
        ]
        live = cli.results(requests, "-C", case)
        assert [frame["name"] for frame in live["file"]["frames"]][1:] == ["data", "system"], live
        assert live["file"]["fileId"] == live["file"]["nodeId"]
        assert live["directory"]["frames"] == live["file"]["frames"] and live["directory"]["fileId"] is None
        assert live["root"]["frames"] == live["file"]["frames"][:1]
        assert live["root"]["nodeId"] == live["file"]["frames"][0]["id"]
        missing = root / (evidence.name + "-absent")
        evidence.rename(missing)
        try:
            offline = cli.results(requests, "-C", case)
            assert offline["open"]["items"][0]["indexOnly"], offline
            for key in ["file", "directory", "root"]:
                assert offline[key] == live[key], (key, offline[key], live[key])
        finally:
            missing.rename(evidence)


def test_links_spellings_and_text_pages(cli, root):
    """Symbolic links, text pages, export rules and Git Bash's rewriting."""
    release = b"ID=fixture\n"
    tool = b"#!/bin/sh\necho tool\n"
    log = "2024-06-01 12:00:00 用户登录成功，来源地址 10.0.0.8，账号：测试\n" * 200
    encoded = log.encode("gbk")
    archive = root / "links.tar"
    with tarfile.open(archive, "w") as output:
        for name in ["etc", "usr", "usr/lib", "usr/bin", "var", "var/log"]:
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.DIRTYPE
            output.addfile(entry)
        for name, data in [("usr/lib/os-release", release), ("usr/bin/tool", tool), ("var/log/app.log", encoded)]:
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            output.addfile(entry, io.BytesIO(data))
        for name, target in [("etc/os-release", "../usr/lib/os-release"), ("bin", "usr/bin"),
                             ("etc/absolute", "/usr/lib/os-release"), ("etc/loop", "loop"),
                             ("etc/escape", "../../../../usr/lib/os-release")]:
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.SYMTYPE
            entry.linkname = target
            output.addfile(entry)
    scope = ("--image", archive)
    top = cli.json(*scope, "ls", "/links")["items"][0]
    prefix = top["path"]
    assert prefix == "/links/tar:", top
    for path in ["/etc/os-release", "/etc/absolute", "/etc/escape", "/usr/lib/os-release"]:
        assert cli.run(*scope, "cat", prefix + path, binary=True).stdout == release, path
    assert cli.run(*scope, "cat", prefix + "/bin/tool", binary=True).stdout == tool
    # The last link is described, not followed; links on the way are.
    link = cli.json(*scope, "stat", prefix + "/etc/os-release")
    assert link["kind"] == "symlink" and link["linkTarget"] == "../usr/lib/os-release", link
    assert cli.json(*scope, "stat", prefix + "/bin/tool")["path"] == prefix + "/usr/bin/tool"
    digest = cli.json(*scope, "hash", prefix + "/etc/os-release", "--algo", "sha256")["items"][0]
    assert digest["bytesHashed"] == len(release), digest
    assert [item["name"] for item in cli.json(*scope, "ls", prefix + "/bin")["items"]] == ["tool"]
    usr = next(item for item in cli.json(*scope, "ls", prefix)["items"] if item["name"] == "usr")
    assert {item["name"] for item in cli.json(*scope, "ls", node("links", usr["id"]))["items"]} == {"lib", "bin"}
    assert says(cli.run(*scope, "cat", prefix + "/etc/loop", expected=1).stderr, "成环")
    missing = cli.run(*scope, "stat", prefix + "/etc/missing", expected=1).stderr
    assert says(missing, "「missing」") and says(missing, prefix + "/etc"), missing

    # Pages of decoded text join to the whole file at character boundaries.
    cut = next(offset for offset in range(4000, len(encoded)) if encoded[offset] >= 0x81
               and len(encoded[:offset].decode("gbk", "ignore").encode("gbk")) == offset)
    first = cli.json(*scope, "text", prefix + "/var/log/app.log", "--max-bytes", cut + 1)
    assert first["encoding"] == "GBK" and not first["hadErrors"], {key: first[key] for key in first if key != "text"}
    assert first["offset"] == 0 and first["nextOffset"] == cut and first["text"] == encoded[:cut].decode("gbk")
    pages, offset = [], 0
    while offset is not None:
        page = cli.json(*scope, "text", prefix + "/var/log/app.log", "--max-bytes", 997, "--offset", offset, "--encoding", "gbk")
        assert page["offset"] == offset and not page["hadErrors"], page["offset"]
        pages.append(page["text"])
        offset = page["nextOffset"]
    assert "".join(pages) == log
    assert cli.json(*scope, "text", prefix + "/var/log/app.log", "--max-bytes", 997, "--offset", 997)["encoding"] == "GBK"
    cli.run(*scope, "text", prefix + "/var/log/app.log", "--encoding", "no-such", expected=1)

    # Exports never overwrite; a directory exports as a tree.
    destination = root / "exported-release"
    cli.run(*scope, "export", prefix + "/etc/os-release", "-o", destination)
    assert destination.read_bytes() == release
    again = cli.run(*scope, "export", prefix + "/etc/os-release", "-o", destination, expected=1)
    assert says(again.stderr, "导出从不覆盖已有文件") and destination.read_bytes() == release, again.stderr
    cli.run(*scope, "export", prefix + "/usr", "-o", root / "usr-export")
    assert (root / "usr-export/lib/os-release").read_bytes() == release
    assert (root / "usr-export/bin/tool").read_bytes() == tool

    # A path Git Bash rewrote below an MSYS installation is turned back, in its
    # plain form and in the path-list form that `:/` causes.
    if os.name == "nt":
        msys = root / "FakeGit"
        (msys / "usr" / "bin").mkdir(parents=True)
        (msys / "usr" / "bin" / "msys-2.0.dll").write_bytes(b"")
        plain = str(msys).replace("\\", "/") + "/links"
        assert cli.json(*scope, "ls", plain) == cli.json(*scope, "ls", "/links")
        listed = f"{msys}\\links\\tar;{msys}\\etc\\os-release"
        assert cli.run(*scope, "cat", listed, binary=True).stdout == release
        elsewhere = str(root).replace("\\", "/") + "/links/tar:/etc/os-release"
        assert says(cli.run(*scope, "cat", elsewhere, expected=1).stderr, "MSYS2_ARG_CONV_EXCL")
