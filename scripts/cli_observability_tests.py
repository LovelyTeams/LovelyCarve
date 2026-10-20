"""Verify progress alongside redirected results, without a desktop terminal."""
import json
from pathlib import Path
import subprocess
import tempfile


def test_observability(executable, base_environment):
    with tempfile.TemporaryDirectory(prefix="lovelycarve-observability-") as directory:
        root = Path(directory)
        environment = {key: value for key, value in base_environment.items()
                       if key not in {"LOVELYCARVE_IMAGE", "LOVELYCARVE_CASE", "LOVELYCARVE_SESSION", "LOVELYCARVE_QUIET"}}
        environment["LOVELYCARVE_SESSION_DIR"] = str(root / "sessions")
        evidence = root / "evidence"
        evidence.mkdir()
        content = b"forensic\x00\xff\n"
        (evidence / "report.bin").write_bytes(content)

        def run(*arguments, expected=0, binary=False, stdin=None, env=None):
            result = subprocess.run([str(executable), *map(str, arguments)], capture_output=True,
                                    timeout=120, cwd=root, env=environment if env is None else env,
                                    input=stdin, **({} if binary else {"text": True, "encoding": "utf-8"}))
            assert result.returncode == expected, (arguments, result.returncode, result.stdout[:1000], result.stderr)
            return result

        def events(result):
            text = result.stderr.decode("utf-8") if isinstance(result.stderr, bytes) else result.stderr
            return [json.loads(line) for line in text.splitlines() if line.strip()]

        plain = run("images", "open", evidence)
        assert json.loads(plain.stdout)["sourceFormat"] == "folder"
        assert "[images open]" in plain.stderr and "完成" in plain.stderr
        assert "\x1b" not in plain.stderr, "redirected progress must be plain text"
        assert run("--quiet", "images", "open", evidence).stderr == ""
        assert run("images", "open", evidence, env=dict(environment, LOVELYCARVE_QUIET="1")).stderr == ""
        observed = run("--quiet", "--events", "images", "open", evidence)
        assert json.loads(observed.stdout)["sourceFormat"] == "folder"
        records = events(observed)
        assert records[0]["event"] == "cli-command" and records[0]["payload"]["phase"] == "started", records
        assert any(row["event"] == "image-load-progress" for row in records), records
        assert any(row["event"] == "cli-command" and row["payload"]["phase"] == "completed" for row in records), records

        # Initial evidence on a command without imageId must be opened too.
        raid = run("--events", "--image", evidence, "raid", "detect")
        assert len(json.loads(raid.stdout)["images"]) == 1
        phases = [row["payload"] for row in events(raid) if row["event"] == "raid-detect-progress"]
        assert phases[0] == {"stage": "lvmMetadata", "current": 0, "total": 1}, phases
        assert phases[-1]["stage"] == "complete"
        assert any(row["stage"] == "heuristic" and row["total"] is None for row in phases), phases

        # Raw stdout must remain byte-for-byte identical with diagnostics on.
        raw = run("--quiet", "--events", "--image", evidence, "files", "read", f"/{evidence.name}/report.bin", binary=True)
        assert raw.stdout == content
        assert any(row["event"] == "image-load-progress" for row in events(raw))
        for reference in ("assembled://missing-array", "ASSEMBLED://missing-array", r"assembled:\missing-array"):
            failure = run("--quiet", "images", "open", reference, expected=1)
            assert "os error 123" not in failure.stderr
            assert all(step in failure.stderr for step in ("image ID", "context use", "sessions open", "raid detect")), failure.stderr
        jsonl = run("--quiet", "session", stdin=json.dumps({"id": "bad", "command": "images open", "args": {"path": "assembled://missing"}}), expected=1)
        error = json.loads(jsonl.stdout)["error"]
        assert error["code"] == "notFound" and error["details"]["context"]["referenceType"] == "assembled", error
        assert "context use <imageId>" in error["details"]["context"]["recovery"]
        failure = run("--quiet", "--events", "images", "open", "assembled://missing", expected=1)
        failures = [row["payload"] for row in events(failure) if row["event"] == "cli-error"]
        assert failures[-1]["code"] == "notFound" and failures[-1]["ipcCommand"] == "image_load", failures

        run("--quiet", "sessions", "open", "watch")
        try:
            remote = run("--quiet", "--events", "--session", "watch", "images", "open", evidence)
            assert json.loads(remote.stdout)["sourceFormat"] == "folder"
            assert any(row["event"] == "image-load-progress" for row in events(remote)), remote.stderr
            remote_raid = run("--quiet", "--events", "--session", "watch", "raid", "detect")
            assert any(row["event"] == "raid-detect-progress" for row in events(remote_raid)), remote_raid.stderr
            raw = run("--quiet", "--events", "--session", "watch", "files", "read", f"/{evidence.name}/report.bin", binary=True)
            assert raw.stdout == content
            assert events(raw)[-1]["payload"]["phase"] == "completed"
        finally:
            run("--quiet", "sessions", "close", "watch")
    print("CLI observability tests passed: redirected progress, quiet controls, ordinary/remote events, RAID scope and actionable reference errors")
