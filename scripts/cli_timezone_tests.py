"""Timestamp display must preserve evidence and survive all CLI transports."""
from datetime import datetime
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile


def test_timezones(executable, base_environment):
    with tempfile.TemporaryDirectory(prefix="lovelycarve-timezones-") as directory:
        root = Path(directory)
        environment = {key: value for key, value in base_environment.items()
                       if key not in {"LOVELYCARVE_IMAGE", "LOVELYCARVE_CASE", "LOVELYCARVE_SESSION"}}
        environment.update(LOVELYCARVE_SESSION_DIR=str(root / "sessions"), TZ="Pacific/Honolulu")
        evidence = root / "nas"
        (evidence / "etc").mkdir(parents=True)
        (evidence / "etc" / "timezone").write_text("Asia/Shanghai\n", encoding="utf-8")
        report = evidence / "report.txt"
        report.write_bytes(b"original forensic bytes\n")
        os.utime(report, (1719792000, 1719792000))  # 2024-07-01T00:00:00Z
        path = "/nas/report.txt"

        def run(*arguments, expected=0, stdin=None):
            result = subprocess.run([str(executable), "--quiet", *map(str, arguments)],
                                    capture_output=True, timeout=120, cwd=root, env=environment,
                                    input=stdin, text=True, encoding="utf-8")
            assert result.returncode == expected, (arguments, result.returncode, result.stdout[:1000], result.stderr)
            return result

        def data(*arguments, **kwargs):
            return json.loads(run(*arguments, **kwargs).stdout)

        def get(*arguments):
            return data("--image", evidence, *arguments, "files", "get", path)

        def check(node, offset, hour):
            timestamp = node["timestamps"]["modified"]
            assert timestamp.endswith(offset) and f"T{hour}:00:00" in timestamp, node
            assert datetime.fromisoformat(timestamp) == datetime.fromisoformat(node["timestampsOriginal"]["modified"])
            assert not node["timestampDisplay"]["unconvertedFields"], node
            return node

        auto = check(get(), "+08:00", "08")
        assert auto["timestampDisplay"]["timeZone"]["source"] == "evidenceConfiguration"
        check(get("--timezone", "UTC"), "+00:00", "00")
        check(get("--timezone", "America/New_York"), "-04:00", "20")
        check(get("--timezone", "-03:00"), "-03:00", "21")
        assert "T05:45:00" in get("--timezone", "+05:45")["timestamps"]["modified"]
        for command, arguments in [("list", ["--path", "/nas"]), ("tree", ["list", "--path", "/nas"])]:
            listed = data("--image", evidence, "files", command, *arguments)
            items = [item.get("node", item) for item in listed["items"]]
            node = next(item for item in items if item["name"] == "report.txt")
            # Opening a live folder may update host access times between
            # processes; its fixed modified time must remain identical.
            assert node["timestamps"]["modified"] == auto["timestamps"]["modified"]
            assert node["timestampsOriginal"]["modified"] == auto["timestampsOriginal"]["modified"]
        assert run("--timezone", "UTC", "--image", evidence, "files", "read", path).stdout == report.read_text()
        failure = run("--timezone", "Not/AZone", "--image", root / "missing.E01", "files", "get", path, expected=1)
        assert "无效时区" in failure.stderr and "image_load" not in failure.stderr

        # A per-request timezone does not alter a long-lived context; parallel
        # readers keep independent policies and barriers save the default.
        requests = [
            {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "set", "command": "timezones set", "args": {"zone": "America/New_York"}},
            {"id": "utc", "command": "files get", "args": {"path": path}, "timezone": "UTC"},
            {"id": "default", "command": "files get", "args": {"path": path}},
            {"id": "evidence", "command": "files get", "args": {"path": path}, "timezone": "evidence"},
            {"id": "after", "command": "timezones get"},
        ]
        rows = [json.loads(line) for line in run("session", "--jobs", "4", stdin="\n".join(map(json.dumps, requests))).stdout.splitlines()]
        results = {row["id"]: row["result"] for row in rows}
        check(results["utc"], "+00:00", "00")
        check(results["default"], "-04:00", "20")
        check(results["evidence"], "+08:00", "08")
        assert results["after"]["sessionDefault"] == "America/New_York"
        shell = run("--image", evidence, "shell", stdin=f"timezones set UTC\nfiles get {path} --timezone Asia/Shanghai\nfiles get {path}\nexit\n")
        nodes = []
        remaining = shell.stdout.strip()
        while remaining:
            node, end = json.JSONDecoder().raw_decode(remaining)
            nodes.append(node)
            remaining = remaining[end:].lstrip()
        check(nodes[-2], "+08:00", "08")
        check(nodes[-1], "+00:00", "00")

        # Settings live in the case ledger and survive separate processes.
        case = root / "case"
        run("cases", "create", case, "--name", "Timezone display test")
        run("--case", case, "timezones", "set", "UTC", "--scope", "case")
        check(get("--case", case), "+00:00", "00")
        run("--case", case, "--image", evidence, "timezones", "set", "Asia/Tokyo", "--scope", "image")
        check(get("--case", case), "+09:00", "09")
        check(get("--case", case, "--timezone", "case"), "+00:00", "00")
        check(get("--case", case, "--timezone", "evidence"), "+08:00", "08")
        run("--case", case, "--image", evidence, "timezones", "set", "auto", "--scope", "image")
        check(get("--case", case), "+00:00", "00")

        run("--image", evidence, "--timezone", "UTC", "sessions", "open", "tz")
        try:
            check(data("--session", "tz", "files", "get", path), "+00:00", "00")
            check(data("--session", "tz", "--timezone", "evidence", "files", "get", path), "+08:00", "08")
            check(data("--session", "tz", "files", "get", path), "+00:00", "00")
        finally:
            run("sessions", "close", "tz")

        # Copied /etc/localtime is a TZif binary, with no IANA name available.
        # This minimal invented v1 file has a single +09:00 standard offset.
        tzif = b"TZif\0" + bytes(15) + struct.pack(">6I", 0, 0, 0, 0, 1, 4) + struct.pack(">lBB", 32400, 0, 0) + b"JST\0"
        (evidence / "etc" / "localtime").write_bytes(tzif)
        binary = check(get(), "+09:00", "09")
        assert binary["timestampDisplay"]["timeZone"]["zone"].startswith("TZif:sha256:")
        (evidence / "etc" / "localtime").write_bytes(b"TZif-broken")
        fallback = check(get(), "+08:00", "08")
        assert fallback["timestampDisplay"]["timeZone"]["warnings"]
        (evidence / "etc" / "localtime").unlink()
        (evidence / "etc" / "timezone").write_text("No/SuchZone")
        unknown = check(get(), "+00:00", "00")
        assert unknown["timestampDisplay"]["timeZone"]["source"] == "fallback"
        run("--image", evidence, "--timezone", "evidence", "files", "get", path, expected=1)
        (evidence / "etc" / "timezone").unlink()
        (evidence / "etc" / "sysconfig").mkdir()
        clock = evidence / "etc" / "sysconfig" / "clock"
        clock.write_text('ZONE="Asia/Tokyo"\n', encoding="utf-8")
        check(get(), "+09:00", "09")
        (evidence / "etc" / "timezone").write_text("Asia/Shanghai\n", encoding="utf-8")
        conflict = check(get(), "+00:00", "00")
        assert conflict["timestampDisplay"]["timeZone"]["warnings"]
        clock.unlink()
        (evidence / "etc" / "timezone").write_text("x" * 4097)
        oversized = check(get(), "+00:00", "00")
        assert any("上限" in warning for warning in oversized["timestampDisplay"]["timeZone"]["warnings"])

        # Tar preserves symlinks on Windows without requiring host privileges.
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
        detected = data("--image", archive, "timezones", "get")
        assert detected["effective"]["zone"] == "Asia/Taipei", detected
        assert detected["effective"]["source"] == "evidenceLocaltimeLink"
        # Use the evidence's own rules even when its TZif differs from the
        # bundled database for the name. Do not silently rewrite old tzdb rules.
        with tarfile.open(archive, "a") as output:
            entry = tarfile.TarInfo("usr/share/zoneinfo/Asia/Taipei")
            entry.size = len(tzif)
            output.addfile(entry, io.BytesIO(tzif))
        original = data("--image", archive, "timezones", "get")
        assert original["effective"]["source"] == "evidenceTzif", original
        roots = data("--image", archive, "files", "list")["items"]
        node = data("--image", archive, "files", "get", f'/{roots[0]["name"]}/report.txt')
        check(node, "+09:00", "09")

        # A configuration directory link must not read examiner host files.
        outside = root / "host-config"
        outside.mkdir()
        (outside / "timezone").write_text("Asia/Tokyo\n")
        escaped = root / "escape.tar"
        with tarfile.open(escaped, "w") as output:
            entry = tarfile.TarInfo("etc")
            entry.type = tarfile.SYMTYPE
            entry.linkname = str(outside)
            output.addfile(entry)
        escaped_zone = data("--image", escaped, "timezones", "get")
        assert escaped_zone["effective"]["source"] == "fallback", escaped_zone
        assert escaped_zone["effective"]["warnings"]
        run("cases", "delete", case, "--force")
    print("CLI timezone tests passed: evidence configs/TZif/symlinks, DST, preserved instants, case/image preferences and session isolation")
