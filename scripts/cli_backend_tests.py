"""Case backends across processes: every command for a case shares one
backend's evidence, unlocked volumes and assembled arrays; byte results
stream intact; cases stay apart; the transport authenticates; a backend
that died is reported once, then cleared; and an interrupted client ends
with 130 while the backend cancels the request it left behind."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

from cli_support import REPOSITORY, Cli, says

ENCRYPTED = REPOSITORY / "src-tauri/tests/data/encrypted"


def descriptor(root, case):
    """The record of the backend that serves `case`."""
    for record in (root / "backends").glob("*.json"):
        value = json.loads(record.read_text(encoding="utf-8"))
        if Path(value["caseDir"]) == Path(case):
            return record, value
    raise AssertionError(f"no backend record for {case}")


def alive(pid):
    if os.name == "nt":
        listed = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH"], capture_output=True, text=True)
        return str(pid) in listed.stdout
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def test_backend(cli, root):
    evidence = root / "evidence"
    (evidence / "sub").mkdir(parents=True)
    content = b"resident evidence\x00\xff\n"
    (evidence / "sub" / "report.bin").write_bytes(content)
    # A multi-byte UTF-8 character across the transport's 8 MiB chunk boundary.
    large = b"x" * (8 * 1024 * 1024 - 1) + "中".encode() + b"z" * (9 * 1024 * 1024)
    (evidence / "large.txt").write_bytes(large)
    password = root / "password.txt"
    password.write_text("lovelycarve-test", encoding="utf-8")
    case, other = root / "case", root / "other case"
    for directory, name in ((case, "Backend test"), (other, "Other backend")):
        cli.run("case", "new", directory, "--name", name)
    scope = ("-C", case)
    opened = cli.json(*scope, "case", "open")
    pid = opened["pid"]
    try:
        assert pid != os.getpid() and opened["running"] and Path(opened["caseDir"]) == case, opened
        assert cli.json(*scope, "case", "open") == {**opened, "alreadyRunning": True}
        cli.run(*scope, "evidence", "add", evidence, "--as", "ev", "--wait")

        # Separate client processes share the backend's open evidence.
        for _ in range(20):
            status = cli.json(*scope, "case", "status")
            assert status["process"] == {"pid": pid, "mode": "Backend"}, status["process"]
            assert [(item["alias"], item["loaded"]) for item in status["evidence"]] == [("ev", True)], status
        # So do the shell and a batch, and a command run from inside the case
        # directory or with LOVELYCARVE_CASE.
        shell = cli.run(*scope, "shell", stdin="cd /ev/folder:/sub\npwd\nstat report.bin\nexit\n")
        assert shell.stdout.startswith("/ev/folder:/sub\n") and "report.bin" in shell.stdout, shell.stdout
        rows = cli.results([{"id": "status", "argv": ["case", "status"]}], *scope)
        assert rows["status"]["process"]["pid"] == pid, rows
        inside = Cli(cli.executable, case, cli.environment)
        assert inside.json("case", "status")["process"]["pid"] == pid
        implicit = dict(cli.environment, LOVELYCARVE_CASE=str(case))
        assert cli.json("case", "status", env=implicit)["process"]["pid"] == pid

        # Byte results stream through the backend unchanged.
        assert cli.run(*scope, "cat", "/ev/folder:/sub/report.bin", binary=True).stdout == content
        assert cli.run(*scope, "cat", "/ev/folder:/large.txt", binary=True).stdout == large
        assert cli.run(*scope, "cat", "/ev/folder:/large.txt", "--offset", "8388606", "--length", "6",
                       binary=True).stdout == large[8388606:8388612]
        page = cli.json(*scope, "text", "/ev/folder:/large.txt", "--offset", "8388600", "--max-bytes", "64")
        assert page["text"] == large[8388600:8388664].decode(), page["text"]
        # A failed command or a refused one leaves the backend serving.
        cli.run(*scope, "stat", "/ev/folder:/missing", expected=1)
        assert "Aborted." in cli.run(*scope, "evidence", "rm", "ev", expected=1).stderr
        assert [item["name"] for item in cli.json(*scope, "evidence", "ls")["items"]] == ["ev"]

        # What lives only as long as a process lives in the backend: a volume
        # unlocked by one client is readable by the next.
        cli.run(*scope, "evidence", "add", ENCRYPTED / "luks1.img", "--as", "lk")
        unlocked = cli.json(*scope, "unlock", "/lk/unsupported:0", "--password", "@" + str(password))
        assert unlocked["path"] == "/lk/part:0", unlocked
        hello = b"LovelyCarve encrypted evidence fixture.\n"
        assert cli.run(*scope, "cat", "/lk/part:0/HELLO.TXT", binary=True).stdout == hello
        assert "lovelycarve-test" not in json.dumps(unlocked)
        # So is an array assembled from member evidence.
        cli.run(*scope, "evidence", "add", ENCRYPTED / "plain.img", "--as", "member")
        assembled = cli.json(*scope, "raid", "assemble", "--members", "member", "--level", "1", "--as", "md")
        assert assembled["path"] == "/md", assembled
        assert cli.json(*scope, "ls", "/md")["total"] >= 1
        assert "HELLO.TXT" in cli.run(*scope, "ls", "/md/part:0").stdout
        # Detection is scoped to the members named, never to derived evidence.
        scoped = cli.json(*scope, "raid", "detect", "--member", "member")
        member_id = next(item["imageId"] for item in cli.json(*scope, "evidence", "ls")["items"] if item["name"] == "member")
        assert [image["imageId"] for image in scoped["images"]] == [member_id], scoped
        cli.run(*scope, "raid", "detect", "--member", "not-loaded", expected=1)
        cli.run(*scope, "raid", "detect", "--member", "md", expected=1)

        # Another case has its own backend and its own evidence.
        second = cli.json("-C", other, "case", "open")
        assert second["pid"] != pid
        assert cli.json("-C", other, "evidence", "ls")["items"] == []
        assert {Path(item["dir"]): item["backend"] for item in cli.json("case", "ls")["items"]
                if Path(item["dir"]) in (case, other)} == {case: True, other: True}
        # A case with a backend is not removed under it.
        assert says(cli.run("case", "rm", other, "-y", expected=1).stderr, "case close")

        # Status never shows the transport credential, and a wrong one is refused.
        record, value = descriptor(root, case)
        assert value["token"] not in cli.run(*scope, "--json", "case", "status").stdout
        with socket.create_connection(("127.0.0.1", value["port"]), timeout=15) as connection:
            connection.sendall(b'{"token":"wrong","work":{"kind":"close"}}\n')
            response = json.loads(connection.makefile("rb").readline())
        assert response["kind"] == "error" and response["error"]["code"] == "permissionDenied", response
        assert cli.json(*scope, "case", "status")["process"]["pid"] == pid

        # Closing stops the backend and releases the case to ordinary processes.
        assert cli.json("-C", other, "case", "close") == {"caseDir": str(other), "closed": True}
        assert cli.json("-C", other, "case", "status")["process"]["mode"] == "OneShot"
        assert cli.json("-C", other, "case", "close")["running"] is False
        cli.run("case", "rm", other, "-y")

        # A backend that died leaves its record; the next client says so once and
        # clears it, then commands run in their own processes until it is opened again.
        if os.name == "nt":
            subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True, check=True)
        else:
            os.kill(pid, signal.SIGKILL)
        deadline = time.monotonic() + 30
        while alive(pid) and time.monotonic() < deadline:
            time.sleep(0.05)
        assert not alive(pid), "backend did not stop"
        failure = cli.run(*scope, "case", "status", expected=1)
        assert says(failure.stderr, "后台进程已退出") and says(failure.stderr, "case open"), failure.stderr
        assert not record.exists()
        assert cli.json(*scope, "case", "status")["process"]["mode"] == "OneShot"
        recovered = cli.json(*scope, "case", "open")
        assert recovered["pid"] != pid and recovered["openEvidence"] == 0, recovered
        # Case evidence is still there; process state (the unlock, the array) is not.
        assert {item["name"] for item in cli.json(*scope, "evidence", "ls")["items"]} == {"ev", "lk", "member"}
        cli.run(*scope, "cat", "/lk/part:0/HELLO.TXT", expected=1)
    finally:
        for directory in (case, other):
            if (directory / "case.db").exists():
                cli.run("-C", directory, "case", "close")
    interrupted = test_interrupted_client(cli, root)
    assert not list((root / "backends").glob("*.json"))
    print("CLI backend tests passed: shared evidence across clients, shell and batch, streamed reads, unlock and RAID reuse, "
          "isolation, authentication, close and crash recovery" + (", interrupted clients" if interrupted else ""))


def test_interrupted_client(cli, root):
    """On Windows, Ctrl+Break ends a forwarded command with 130 instead of the
    console's default termination, and the backend cancels the request instead
    of finishing it for nobody, so the next client is served at once."""
    if os.name != "nt":
        return False
    case = root / "interrupted case"
    cli.run("case", "new", case, "--name", "Interrupted client")
    scope = ("-C", case)
    backend = cli.json(*scope, "case", "open")["pid"]
    try:
        cli.run(*scope, "evidence", "add", ENCRYPTED / "veracrypt.hc", "--as", "vc")
        wrong = root / "wrong password.txt"
        wrong.write_text("not the password", encoding="utf-8")
        # The largest PIM keeps the key derivation busy far longer than this
        # test in any build, and an unlock runs alone: abandoned, it would hold
        # the backend for every other client.
        process = subprocess.Popen(
            [str(cli.executable), "--events", "-q", *map(str, scope), "unlock", "/vc", "--scheme", "veracrypt",
             "--password", "@" + str(wrong), "--pim", "2147468", "--prf", "sha512", "--header", "normal"],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, encoding="utf-8",
            creationflags=subprocess.CREATE_NEW_PROCESS_GROUP, cwd=cli.cwd, env=cli.environment)
        try:
            started, task = False, None
            for line in process.stderr:
                record = json.loads(line)
                payload = record.get("payload") or {}
                started |= record["event"] == "cli-command" and payload.get("phase") == "started"
                entry = payload.get("entry") or {}
                # The backend relays its task center: the unlock runs there now.
                if started and record["event"] == "task-entry" and entry.get("kind") == "encryptedVolumeUnlock" \
                        and entry.get("status") == "running":
                    task = entry["id"]
                    break
            else:
                raise AssertionError(f"the unlock never ran in the backend: exit {process.wait(timeout=30)}")
            process.send_signal(signal.CTRL_BREAK_EVENT)
            assert process.wait(timeout=30) == 130
            records = [json.loads(line) for line in process.stderr.read().splitlines() if line.startswith("{")]
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=30)
        # The client reported the cancellation itself instead of being cut off.
        phases = [record["payload"]["phase"] for record in records if record["event"] == "cli-command"]
        assert phases[-1:] == ["cancelled"], records
        # The next client is served at once, and the unlock was cancelled rather
        # than left running for nobody.
        assert cli.json(*scope, "case", "status", timeout=60)["process"] == {"pid": backend, "mode": "Backend"}
        tasks = {item["id"]: item["status"] for item in cli.json(*scope, "tasks", "ls", timeout=60)["items"]}
        assert tasks.get(task) == "cancel", (task, tasks)
        assert [item["name"] for item in cli.json(*scope, "evidence", "ls", timeout=60)["items"]] == ["vc"]
    finally:
        try:
            cli.run(*scope, "case", "close", timeout=60)
        except BaseException:
            # A backend that no longer answers must not outlive the test.
            subprocess.run(["taskkill", "/F", "/PID", str(backend)], capture_output=True)
            raise
    return True
