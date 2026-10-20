"""Bound agent output across direct, resident and JSONL transports."""
import json
import os
from pathlib import Path
import subprocess
import tempfile


def test_agent_output(executable, base_environment):
    with tempfile.TemporaryDirectory(prefix="lovelycarve-agent-") as directory:
        root = Path(directory)
        evidence = root / "System32"
        evidence.mkdir()
        for index in range(520):
            path = evidence / f"system-library-{index:04}.dll"
            path.write_bytes(b"fixture\n")
            os.utime(path, (1719792000, 1719792000))
        environment = dict(base_environment, CLAUDECODE="1", LOVELYCARVE_SESSION_DIR=str(root / "sessions"))
        environment.pop("RUST_LOG", None)

        def run(*args, stdin=None, expected=0, env=None):
            result = subprocess.run([str(executable), "-q", *map(str, args)],
                                    input=stdin, text=True, encoding="utf-8", env=environment if env is None else env,
                                    cwd=root, capture_output=True, timeout=120)
            assert result.returncode == expected, (args, result.stdout[:1000], result.stderr)
            return result

        capabilities = json.loads(run("system", "get").stdout)
        assert capabilities["output"]["agentFilesListDefaultPageSize"] == 50
        assert "fields" in capabilities["session"]["request"]
        fields = "name,id,kind,size,timestamps.modified"
        args = ["--image", evidence, "--timezone", "UTC", "files", "list", "--path", "/System32"]
        page = run(*args)
        data = json.loads(page.stdout)
        assert len(data["items"]) == 50 and data["nextCursor"], data
        assert page.stdout.count("\n") == 1 and not page.stderr
        assert page.stdout.index('"nextCursor"') < page.stdout.index('"items"')
        assert all("timestampDisplay" not in item and "timestampsOriginal" not in item for item in data["items"])
        projected = run(*args, "--fields", fields)
        small = json.loads(projected.stdout)
        assert set(small["items"][0]) == {"name", "id", "kind", "size", "timestamps"}
        assert set(small["items"][0]["timestamps"]) == {"modified"}
        assert small["timestampDisplay"] == data["timestampDisplay"]
        assert len(projected.stdout) < 12_000, len(projected.stdout)
        following = json.loads(run(*args, "--fields", fields, "--cursor", small["nextCursor"]).stdout)
        assert len(following["items"]) == 50
        assert not ({item["id"] for item in small["items"]} & {item["id"] for item in following["items"]})
        assert len(json.loads(run(*args, "--page-size", "500", "--fields", fields).stdout)["items"]) == 500
        agent_variables = {"OPENCODE", "QWEN_CODE_SESSION_ID", "PI_CODING_AGENT", "AI_AGENT", "CURSOR_AGENT", "CLAUDECODE",
                           "CODEX_THREAD_ID", "AMP_CURRENT_THREAD_ID", "GEMINI_CLI", "AUGMENT_AGENT", "CRUSH", "COPILOT_AGENT", "OZ_RUN_ID"}
        human_environment = {key:value for key,value in environment.items() if key not in agent_variables}
        assert len(json.loads(run(*args, env=human_environment).stdout)["items"]) == 500
        for invalid in ("name,,id", "timestamps..modified"):
            run(*args, "--fields", invalid, expected=1)
        # Raw output and mutations must reject projection before producing host output.
        run("files", "export", "--path", "/missing", "--destination", root / "unexpected.bin", "--fields", "id", expected=1)
        assert not (root / "unexpected.bin").exists()
        failure = run("files", "lsit", expected=1)
        assert len(failure.stderr) < 1000 and "AGENT COMMAND DISCOVERY" not in failure.stderr
        assert "files list" in failure.stderr
        if "files lookup" in failure.stderr:
            assert failure.stderr.index("files list") < failure.stderr.index("files lookup")
        assert "AGENT COMMAND DISCOVERY" in run("files", "--help").stdout
        assert "AGENT COMMAND DISCOVERY" not in run("files", "list", "--help").stdout
        for intent, expected in [("list partitions", "images get"), ("change directory", "context set"),
                                 ("show file timestamps", "files get"), ("open evidence", "images open")]:
            assert json.loads(run("cli", "search", intent).stdout)[0]["command"] == "lovelycarve-cli " + expected

        requests = [
            {"id": "open", "command": "images open", "args": {"path": str(evidence)}},
            {"id": "list", "command": "files list", "args": {"path": "/System32", "pageSize": 3}, "fields": fields, "timezone": "UTC"},
            {"id": "default", "command": "files list", "args": {"path": "/System32"}, "fields": fields},
            {"id": "text", "command": "files text get", "args": {"path": "/System32/system-library-0000.dll"}},
        ]
        results = [json.loads(line) for line in run("session", stdin="\n".join(map(json.dumps, requests))).stdout.splitlines()]
        assert len(results[1]["result"]["items"]) == 3
        assert set(results[1]["result"]["items"][0]) == set(small["items"][0])
        assert len(results[2]["result"]["items"]) == 50
        assert "fixture" in json.dumps(results[3]["result"])
        run("sessions", "open", "agent", "--image", evidence)
        try:
            listed = json.loads(run("sessions", "list", "--fields", "name").stdout)
            assert listed["total"] == 1 and listed["sessions"] == [{"name":"agent"}], listed
            remote = json.loads(run("--session", "agent", "--timezone", "UTC", "files", "list",
                                    "--path", "/System32", "--fields", fields).stdout)
            assert len(remote["items"]) == 50 and set(remote["items"][0]) == set(small["items"][0])
            human_remote = json.loads(run("--session", "agent", "files", "list", "--path", "/System32", env=human_environment).stdout)
            assert len(human_remote["items"]) == 500, "the caller determines defaults, not the resident server's environment"
        finally:
            run("sessions", "close", "agent")
    print("CLI agent tests passed: bounded projection, cursors, discovery and all transports")
