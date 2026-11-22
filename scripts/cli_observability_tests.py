"""Progress beside redirected results, without a terminal: plain lines on
stderr, nothing with --quiet, JSONL with --events, the same through the case
backend, and raw stdout that stays byte-exact."""
import json

from cli_support import request, says


def events(text):
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def test_observability(cli, root):
    evidence = root / "evidence"
    evidence.mkdir()
    content = b"forensic\x00\xff\n"
    (evidence / "report.bin").write_bytes(content)
    report = "/evidence/folder:/report.bin"

    plain = cli.run("evidence", "add", evidence)
    assert "/evidence" in plain.stdout and "folder" in plain.stdout, plain.stdout
    assert "[evidence add]" in plain.stderr and "完成" in plain.stderr, plain.stderr
    assert "\x1b" not in plain.stderr, "redirected progress must be plain text"
    assert cli.run("--quiet", "evidence", "add", evidence).stderr == ""
    # Progress for people stays off with --json: programs read --events.
    machine = cli.run("--json", "evidence", "add", evidence)
    assert json.loads(machine.stdout)["items"][0]["sourceFormat"] == "folder" and machine.stderr == "", machine.stderr
    assert cli.run("evidence", "add", evidence, env=dict(cli.environment, LOVELYCARVE_QUIET="1")).stderr == ""
    observed = cli.run("--quiet", "--events", "--json", "evidence", "add", evidence)
    assert json.loads(observed.stdout)["items"][0]["sourceFormat"] == "folder"
    records = events(observed.stderr)
    assert records[0]["event"] == "cli-command" and records[0]["payload"]["phase"] == "started", records[0]
    assert any(row["event"] == "image-load-progress" for row in records), [row["event"] for row in records]
    assert any(row["event"] == "cli-command" and row["payload"]["phase"] == "completed" for row in records)

    # Evidence given with --image to a command that takes none is opened too.
    raid = cli.run("--events", "--json", "--image", evidence, "raid", "detect")
    assert len(json.loads(raid.stdout)["images"]) == 1
    phases = [row["payload"] for row in events(raid.stderr) if row["event"] == "raid-detect-progress"]
    assert phases[0] == {"stage": "lvmMetadata", "current": 0, "total": 1}, phases
    assert phases[-1]["stage"] == "complete"
    assert any(row["stage"] == "heuristic" and row["total"] is None for row in phases), phases

    # Raw stdout stays byte-for-byte identical with diagnostics on.
    raw = cli.run("--quiet", "--events", "--image", evidence, "cat", report, binary=True)
    assert raw.stdout == content
    assert any(row["event"] == "image-load-progress" for row in events(raw.stderr.decode("utf-8")))

    # A reference to derived evidence that no longer exists says how to get it back.
    for reference in ("assembled://missing-array", "ASSEMBLED://missing-array", "assembled:\\missing-array"):
        failure = cli.run("--quiet", "evidence", "add", reference, expected=1)
        assert "os error 123" not in failure.stderr
        assert says(failure.stderr, "组装卷只存在于组装它的进程") and says(failure.stderr, "RAID 检测"), failure.stderr
    _, rows = cli.batch([request("bad", "evidence", "add", "assembled://missing")], "--quiet", expected=1)
    error = rows[0]["error"]
    assert error["code"] == "notFound" and error["details"]["context"]["referenceType"] == "assembled", error
    assert "raid detect" in error["details"]["context"]["recovery"], error
    failure = cli.run("--quiet", "--events", "evidence", "add", "assembled://missing", expected=1)
    failures = [row["payload"] for row in events(failure.stderr) if row["event"] == "cli-error"]
    assert failures[-1]["code"] == "notFound" and failures[-1]["operation"] == "image_load", failures

    # Commands sent to a case backend report the backend's progress the same way.
    case = root / "case"
    cli.run("case", "new", case, "--name", "Observed")
    cli.json("-C", case, "case", "open")
    try:
        remote = cli.run("--quiet", "--events", "--json", "-C", case, "evidence", "add", evidence)
        assert json.loads(remote.stdout)["items"][0]["sourceFormat"] == "folder"
        assert any(row["event"] == "image-load-progress" for row in events(remote.stderr)), remote.stderr
        remote_raid = cli.run("--quiet", "--events", "--json", "-C", case, "raid", "detect")
        assert any(row["event"] == "raid-detect-progress" for row in events(remote_raid.stderr)), remote_raid.stderr
        raw = cli.run("--quiet", "--events", "-C", case, "cat", report, binary=True)
        assert raw.stdout == content
        assert events(raw.stderr.decode("utf-8"))[-1]["payload"]["phase"] == "completed"
    finally:
        cli.run("--quiet", "-C", case, "case", "close")
    print("CLI observability tests passed: redirected progress, quiet controls, events in-process and from the backend, "
          "RAID progress, byte-exact stdout and recoverable reference errors")
