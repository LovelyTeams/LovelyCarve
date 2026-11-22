"""Timestamp display preserves the evidence and holds across every transport:
evidence configuration, TZif files and links, --tz per command, case and
evidence settings, batches, the shell and the case backend."""
from datetime import datetime
import io
import json
import os
from pathlib import PurePosixPath
import struct
import tarfile

from cli_support import Cli, request

PATH = "/nas/folder:/report.txt"


def check(node, offset, hour):
    timestamp = node["timestamps"]["modified"]
    assert timestamp.endswith(offset) and f"T{hour}:00:00" in timestamp, node
    original = node.get("timestampsOriginal", node["timestamps"])["modified"]
    assert datetime.fromisoformat(timestamp) == datetime.fromisoformat(original)
    assert not node["timestampDisplay"]["unconvertedFields"], node
    return node


def json_documents(text):
    """The JSON documents a script printed one after another."""
    decoder, documents, remaining = json.JSONDecoder(), [], text.strip()
    while remaining:
        value, end = decoder.raw_decode(remaining)
        documents.append(value)
        remaining = remaining[end:].lstrip()
    return documents


def config_archive(root, name, links, files):
    """A tar with `files` and the symbolic `links`, as Linux and Android keep them."""
    target = root / name
    with tarfile.open(target, "w") as output:
        directories = set()
        for file_name in files:
            directories.update(str(parent) for parent in PurePosixPath(file_name).parents if str(parent) != ".")
        for directory in sorted(directories):
            entry = tarfile.TarInfo(directory)
            entry.type = tarfile.DIRTYPE
            output.addfile(entry)
        for file_name, body in files.items():
            entry = tarfile.TarInfo(file_name)
            entry.size = len(body)
            entry.mtime = 1719792000
            output.addfile(entry, io.BytesIO(body))
        for link_name, destination in links.items():
            entry = tarfile.TarInfo(link_name)
            entry.type = tarfile.SYMTYPE
            entry.linkname = destination
            output.addfile(entry)
    return target


def test_timezones(cli, root):
    # The examiner's own zone must never show up in the results.
    cli = Cli(cli.executable, cli.cwd, dict(cli.environment, TZ="Pacific/Honolulu"))
    evidence = root / "nas"
    (evidence / "etc").mkdir(parents=True)
    (evidence / "etc" / "timezone").write_text("Asia/Shanghai\n", encoding="utf-8")
    report = evidence / "report.txt"
    report.write_bytes(b"original forensic bytes\n")
    os.utime(report, (1719792000, 1719792000))  # 2024-07-01T00:00:00Z

    def get(*arguments):
        return cli.json("--image", evidence, *arguments, "stat", PATH)

    auto = check(get(), "+08:00", "08")
    assert auto["timestampDisplay"]["timeZone"]["source"] == "evidenceConfiguration", auto
    check(get("--tz", "UTC"), "+00:00", "00")
    check(get("--tz", "America/New_York"), "-04:00", "20")
    check(get("--tz", "-03:00"), "-03:00", "21")
    assert "T05:45:00" in get("--tz", "+05:45")["timestamps"]["modified"]
    for arguments in (["ls", "/nas/folder:"], ["tree", "/nas", "--depth", "3"]):
        listed = cli.json("--image", evidence, *arguments)
        assert listed["timestampDisplay"]["timeZones"][0]["zone"] == "Asia/Shanghai", listed
        assert all("timestampDisplay" not in item for item in listed["items"]), listed
        node = next(item for item in listed["items"] if item["name"] == "report.txt")
        # Opening a live folder may update host access times between processes;
        # its fixed modified time stays identical.
        assert node["timestamps"]["modified"] == auto["timestamps"]["modified"]
        assert node.get("timestampsOriginal", node["timestamps"])["modified"] == auto["timestampsOriginal"]["modified"]
    assert cli.run("--tz", "UTC", "--image", evidence, "cat", PATH).stdout == report.read_text()
    failure = cli.run("--tz", "Not/AZone", "--image", root / "missing.E01", "stat", PATH, expected=1)
    assert "无效时区" in failure.stderr and "image_load" not in failure.stderr, failure.stderr

    # --tz on one request never changes another; parallel readers keep their own.
    rows = cli.results([
        request("open", "evidence", "add", evidence),
        request("utc", "--tz", "UTC", "stat", PATH),
        request("default", "stat", PATH),
        request("evidence", "--tz", "evidence", "stat", PATH),
        request("effective", "tz", "/nas"),
    ], jobs=4)
    check(rows["utc"], "+00:00", "00")
    check(rows["default"], "+08:00", "08")
    check(rows["evidence"], "+08:00", "08")
    assert rows["effective"]["effective"]["zone"] == "Asia/Shanghai", rows["effective"]
    # The shell's --tz applies to every line; a line's own --tz wins.
    shell = cli.run("--json", "--tz", "UTC", "--image", evidence, "shell",
                    stdin=f"stat {PATH} --tz Asia/Shanghai\nstat {PATH}\nexit\n")
    nodes = json_documents(shell.stdout)
    check(nodes[-2], "+08:00", "08")
    check(nodes[-1], "+00:00", "00")
    # Saved settings need a case.
    assert "-C" in cli.run("--image", evidence, "tz", "set", "UTC", expected=1).stderr

    # Settings live in the case and hold across processes and the backend.
    case = root / "case"
    cli.run("case", "new", case, "--name", "Timezone display test")
    in_case = ("-C", case)
    cli.run(*in_case, "evidence", "add", evidence)
    assert cli.json(*in_case, "tz", "set", "UTC") == {"zone": "UTC", "scope": "case", "saved": True}
    check(cli.json(*in_case, "stat", PATH), "+00:00", "00")
    assert cli.json(*in_case, "tz", "set", "Asia/Tokyo", "/nas")["scope"] == "evidence"
    check(cli.json(*in_case, "stat", PATH), "+09:00", "09")
    check(cli.json(*in_case, "--tz", "case", "stat", PATH), "+00:00", "00")
    check(cli.json(*in_case, "--tz", "evidence", "stat", PATH), "+08:00", "08")
    cli.run(*in_case, "tz", "set", "auto", "/nas")
    check(cli.json(*in_case, "stat", PATH), "+00:00", "00")
    cli.run(*in_case, "tz", "set", "evidence", expected=1)
    cli.json(*in_case, "case", "open")
    try:
        check(cli.json(*in_case, "stat", PATH), "+00:00", "00")
        check(cli.json(*in_case, "--tz", "evidence", "stat", PATH), "+08:00", "08")
        check(cli.json(*in_case, "stat", PATH), "+00:00", "00")
    finally:
        cli.run(*in_case, "case", "close")
    cli.run("case", "rm", case, "-y")

    # A copied /etc/localtime is a TZif binary with no IANA name. This minimal
    # invented v1 file has a single +09:00 standard offset.
    tzif = b"TZif\0" + bytes(15) + struct.pack(">6I", 0, 0, 0, 0, 1, 4) + struct.pack(">lBB", 32400, 0, 0) + b"JST\0"
    (evidence / "etc" / "localtime").write_bytes(tzif)
    binary = check(get(), "+09:00", "09")
    assert binary["timestampDisplay"]["timeZone"]["zone"].startswith("TZif:sha256:")
    (evidence / "etc" / "localtime").write_bytes(b"TZif-broken")
    assert check(get(), "+08:00", "08")["timestampDisplay"]["timeZone"]["warnings"]
    (evidence / "etc" / "localtime").unlink()
    (evidence / "etc" / "timezone").write_text("No/SuchZone")
    assert check(get(), "+00:00", "00")["timestampDisplay"]["timeZone"]["source"] == "fallback"
    cli.run("--image", evidence, "--tz", "evidence", "stat", PATH, expected=1)
    (evidence / "etc" / "timezone").unlink()
    (evidence / "etc" / "sysconfig").mkdir()
    clock = evidence / "etc" / "sysconfig" / "clock"
    clock.write_text('ZONE="Asia/Tokyo"\n', encoding="utf-8")
    check(get(), "+09:00", "09")
    (evidence / "etc" / "timezone").write_text("Asia/Shanghai\n", encoding="utf-8")
    assert check(get(), "+00:00", "00")["timestampDisplay"]["timeZone"]["warnings"]
    clock.unlink()
    (evidence / "etc" / "timezone").write_text("x" * 4097)
    oversized = check(get(), "+00:00", "00")
    assert any("上限" in warning for warning in oversized["timestampDisplay"]["timeZone"]["warnings"])

    # Tar keeps symbolic links on Windows without host privileges.
    archive = root / "symlink.tar"
    with tarfile.open(archive, "w") as output:
        for name in ["etc", "usr", "usr/share", "usr/share/zoneinfo"]:
            entry = tarfile.TarInfo(name)
            entry.type = tarfile.DIRTYPE
            output.addfile(entry)
        link = tarfile.TarInfo("etc/localtime")
        link.type = tarfile.SYMTYPE
        link.linkname = "../usr/share/zoneinfo/Asia/Taipei"
        output.addfile(link)
        entry = tarfile.TarInfo("report.txt")
        entry.mtime = 1719792000
        entry.size = 3
        output.addfile(entry, io.BytesIO(b"abc"))
    detected = cli.json("--image", archive, "tz")
    assert detected["effective"]["zone"] == "Asia/Taipei" and detected["effective"]["source"] == "evidenceLocaltimeLink", detected
    # The evidence's own rules win even where its TZif differs from the bundled
    # database for that name; old tzdb rules are never silently rewritten.
    with tarfile.open(archive, "a") as output:
        entry = tarfile.TarInfo("usr/share/zoneinfo/Asia/Taipei")
        entry.size = len(tzif)
        output.addfile(entry, io.BytesIO(tzif))
    assert cli.json("--image", archive, "tz")["effective"]["source"] == "evidenceTzif"
    check(cli.json("--image", archive, "stat", "/symlink/tar:/report.txt"), "+09:00", "09")

    # Android-style /etc and a relocated /usr are read in the evidence's own
    # namespace, through bounded relative, absolute and chained directory links.
    for name, links, files, source in [
        ("relative-config.tar", {"etc": "system/etc"}, {"system/etc/timezone": b"Asia/Tokyo\n"}, "evidenceConfiguration"),
        ("chained-config.tar", {"etc": "/alias", "alias": "system/config/../etc"},
         {"system/config/keep": b"", "system/etc/timezone": b"Asia/Tokyo\n"}, "evidenceConfiguration"),
        ("relocated-tzif.tar", {"etc": "/system/etc", "usr": "system/usr", "system/etc/localtime": "../../usr/share/zoneinfo/Asia/Taipei"},
         {"system/etc/keep": b"", "system/usr/share/zoneinfo/Asia/Taipei": tzif}, "evidenceTzif"),
    ]:
        linked = config_archive(root, name, links, files)
        detected = cli.json("--image", linked, "tz")
        assert detected["effective"]["source"] == source and not detected["effective"]["warnings"], detected
        assert detected["effective"]["zone"] == ("Asia/Taipei" if source == "evidenceTzif" else "Asia/Tokyo"), detected
        if source != "evidenceTzif":
            # Reading follows the same directory links detection used.
            alias = name.split(".")[0]
            assert cli.json("--image", linked, "text", f"/{alias}/tar:/etc/timezone")["text"] == "Asia/Tokyo\n"

    long_chain = {"etc": "link0", **{f"link{index}": f"link{index + 1}" for index in range(41)}}
    long_chain["link41"] = "system/etc"
    for name, links, expected_warning in [
        ("cycle-config.tar", {"etc": "alias", "alias": "etc"}, "跳转"),
        ("deep-config.tar", long_chain, "跳转"),
        ("dangling-config.tar", {"etc": "missing"}, "目标不存在"),
        ("escape-relative-config.tar", {"etc": "../host-config"}, "越过"),
        ("escape-absolute-config.tar", {"etc": "/../host-config"}, "越过"),
        ("unc-config.tar", {"etc": "\\\\server\\share\\etc"}, "不是受支持"),
    ]:
        linked = config_archive(root, name, links, {"system/etc/timezone": b"Asia/Tokyo\n"})
        detected = cli.json("--image", linked, "tz")
        assert detected["effective"]["source"] == "fallback", detected
        assert any(expected_warning in warning for warning in detected["effective"]["warnings"]), detected
        cli.run("--image", linked, "--tz", "evidence", "ls", "/" + name.split(".")[0], expected=1)

    # A configuration directory link never reads the examiner's host files.
    outside = root / "host-config"
    outside.mkdir()
    (outside / "timezone").write_text("Asia/Tokyo\n")
    escaped = root / "escape.tar"
    with tarfile.open(escaped, "w") as output:
        entry = tarfile.TarInfo("etc")
        entry.type = tarfile.SYMTYPE
        entry.linkname = str(outside)
        output.addfile(entry)
    escaped_zone = cli.json("--image", escaped, "tz")
    assert escaped_zone["effective"]["source"] == "fallback" and escaped_zone["effective"]["warnings"], escaped_zone
    print("CLI timezone tests passed: evidence configs, TZif and links, DST, preserved instants, --tz per command, "
          "case and evidence settings, batch, shell and backend")
