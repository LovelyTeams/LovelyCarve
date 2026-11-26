"""Case backends across processes: every command for a case shares one
backend's evidence, unlocked volumes and assembled arrays; byte results
stream intact; cases stay apart; the transport authenticates; a backend
that died is reported once, then cleared; an interrupted client ends with
130 while the backend cancels the request it left behind, down to the read
of a single file; and a close that waits for busy work leaves the backend
answering."""
import ctypes
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import threading
import time

from cli_support import REPOSITORY, Cli, says

ENCRYPTED = REPOSITORY / "src-tauri/tests/data/encrypted"
FSCTL_SET_SPARSE = 0x000900C4


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


def attempt(cli, *arguments, timeout=60):
    """Run a command whatever it exits with."""
    return subprocess.run([str(cli.executable), *map(str, arguments)], capture_output=True, text=True,
                          encoding="utf-8", cwd=cli.cwd, env=cli.environment, timeout=timeout)


def error_code(stderr):
    """The code of the JSON error line a command wrote."""
    for line in reversed(stderr.splitlines()):
        if line.startswith("{") and "error" in (record := json.loads(line)):
            return record["error"]["code"]
    return None


def slow_unlock(cli, scope, password, **options):
    """Start an unlock in the backend of `scope` that runs alone far longer than
    any test: the largest PIM keeps the key derivation of a wrong `password`
    busy. Returns the client once the unlock runs, and the unlock's task ID."""
    process = subprocess.Popen(
        [str(cli.executable), "--events", "-q", *map(str, scope), "unlock", "/vc", "--scheme", "veracrypt",
         "--password", "@" + str(password), "--pim", "2147468", "--prf", "sha512", "--header", "normal"],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, encoding="utf-8", cwd=cli.cwd,
        env=cli.environment, **options)
    started = False
    for line in process.stderr:
        record = json.loads(line)
        payload = record.get("payload") or {}
        started |= record["event"] == "cli-command" and payload.get("phase") == "started"
        entry = payload.get("entry") or {}
        # The backend relays its task center: the unlock runs there now.
        if started and record["event"] == "task-entry" and entry.get("kind") == "encryptedVolumeUnlock" \
                and entry.get("status") == "running":
            return process, entry["id"]
    raise AssertionError(f"the unlock never ran in the backend: exit {process.wait(timeout=30)}")


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
    test_close_while_busy(cli, root)
    assert not list((root / "backends").glob("*.json"))
    print("CLI backend tests passed: shared evidence across clients, shell and batch, streamed reads, unlock and RAID reuse, "
          "isolation, authentication, close and crash recovery" + (", interrupted clients" if interrupted else "")
          + ", closing while busy")


def sparse_file(path, size):
    """A file of `size` zero bytes that takes no disk space, so that hashing or
    exporting all of it outlasts this test in any build."""
    import msvcrt
    from ctypes import wintypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.DeviceIoControl.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD,
                                         wintypes.LPVOID, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD),
                                         wintypes.LPVOID]
    kernel32.DeviceIoControl.restype = wintypes.BOOL
    with open(path, "wb") as file:
        returned = wintypes.DWORD()
        if not kernel32.DeviceIoControl(msvcrt.get_osfhandle(file.fileno()), FSCTL_SET_SPARSE, None, 0, None, 0,
                                        ctypes.byref(returned), None):
            raise ctypes.WinError(ctypes.get_last_error())
        file.truncate(size)


def interrupt_running(cli, scope, kind, *argv):
    """Run a command, press Ctrl+Break once its task of `kind` runs, and check
    that the client ends with 130. Returns the task's ID and the seconds from
    the interrupt to the exit."""
    process = subprocess.Popen(
        [str(cli.executable), "--events", "-q", *map(str, scope), *map(str, argv)],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, encoding="utf-8",
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP, cwd=cli.cwd, env=cli.environment)
    try:
        started, task = False, None
        # Heartbeats keep the events coming while the command runs.
        deadline = time.monotonic() + 120
        for line in process.stderr:
            record = json.loads(line)
            payload = record.get("payload") or {}
            started |= record["event"] == "cli-command" and payload.get("phase") == "started"
            snapshot = payload.get("task") or {}
            # A backend relays the lifecycle of its tasks: the work runs there now.
            if started and record["event"] == "task-lifecycle" and payload.get("phase") == "running" \
                    and snapshot.get("kind") == kind:
                task = snapshot["id"]
                break
            assert time.monotonic() < deadline, f"no {kind} task ran for {argv}"
        else:
            raise AssertionError(f"no {kind} task ran for {argv}: exit {process.wait(timeout=30)}")
        process.send_signal(signal.CTRL_BREAK_EVENT)
        interrupted = time.monotonic()
        # Keep reading: the events of the cancellation would fill the pipe and
        # stall a client that has not exited yet.
        _, rest = process.communicate(timeout=30)
        exited = time.monotonic() - interrupted
        assert process.returncode == 130, (process.returncode, rest)
        records = [json.loads(line) for line in rest.splitlines() if line.startswith("{")]
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=30)
    # The client reported the cancellation itself instead of being cut off.
    phases = [record["payload"]["phase"] for record in records if record["event"] == "cli-command"]
    assert phases[-1:] == ["cancelled"], records
    return task, exited


def test_interrupted_client(cli, root):
    """On Windows, Ctrl+Break ends a forwarded command with 130 instead of the
    console's default termination, and the backend cancels the request instead
    of finishing it for nobody, so the next client is served at once."""
    if os.name != "nt":
        return False
    large = root / "large"
    large.mkdir()
    sparse_file(large / "hash.bin", 64 << 30)
    sparse_file(large / "export.bin", 16 << 30)
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
        task, _ = interrupt_running(cli, scope, "encryptedVolumeUnlock", "unlock", "/vc", "--scheme", "veracrypt",
                                    "--password", "@" + str(wrong), "--pim", "2147468", "--prf", "sha512",
                                    "--header", "normal")
        # The next client is served at once, and the unlock was cancelled rather
        # than left running for nobody.
        assert cli.json(*scope, "case", "status", timeout=60)["process"] == {"pid": backend, "mode": "Backend"}
        tasks = {item["id"]: item["status"] for item in cli.json(*scope, "tasks", "ls", timeout=60)["items"]}
        assert tasks.get(task) == "cancel", (task, tasks)
        assert [item["name"] for item in cli.json(*scope, "evidence", "ls", timeout=60)["items"]] == ["vc"]

        # Reading one file is a task of its request too: a hash or a single-file
        # export stops at its next chunk instead of reading on for nobody.
        cli.run(*scope, "evidence", "add", large, "--as", "large", "--wait", timeout=120)
        output = root / "interrupted export"
        output.mkdir()
        for kind, argv in (("fileHash", ("hash", "/large/folder:/hash.bin", "--algo", "sm3")),
                           ("fileExport", ("export", "/large/folder:/export.bin", "-o", output / "export.bin"))):
            interrupt_running(cli, scope, kind, *argv)
            # `tasks wait` returns once no task runs, and the read's worker holds
            # its task until the read stops.
            waiting = time.monotonic()
            cli.run(*scope, "tasks", "wait", timeout=60)
            assert time.monotonic() - waiting < 15, f"the backend read on after {argv[0]} was interrupted"
        # The export published nothing, and its staging file is gone.
        assert list(output.iterdir()) == []
    finally:
        try:
            cli.run(*scope, "case", "close", timeout=60)
        except BaseException:
            # A backend that no longer answers must not outlive the test.
            subprocess.run(["taskkill", "/F", "/PID", str(backend)], capture_output=True)
            raise
    # Without a backend, Ctrl+Break cancels the read as well instead of waiting
    # for the runtime's 10 s shutdown.
    _, exited = interrupt_running(cli, ("--image", large), "fileHash", "hash", "/large/folder:/hash.bin",
                                  "--algo", "sm3")
    assert exited < 5, f"the interrupted one-shot hash took {exited:.1f} s to exit"
    return True


def test_close_while_busy(cli, root):
    """A close waits for the command that runs alone, and the backend keeps
    answering meanwhile: `case open` and other commands fail at once with
    caseBackendClosing instead of waiting for the backend to exit."""
    case = root / "busy case"
    cli.run("case", "new", case, "--name", "Busy close")
    scope = ("-C", case)
    backend = cli.json(*scope, "case", "open")["pid"]
    unlock = closer = None
    try:
        cli.run(*scope, "evidence", "add", ENCRYPTED / "veracrypt.hc", "--as", "vc")
        wrong = root / "wrong password for the busy case.txt"
        wrong.write_text("not the password", encoding="utf-8")
        unlock, _ = slow_unlock(cli, scope, wrong)
        # Its progress keeps coming; read it so that it never stops reading.
        threading.Thread(target=unlock.stderr.read, daemon=True).start()
        closer = subprocess.Popen([str(cli.executable), "--json", *map(str, scope), "case", "close"],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
                                  cwd=cli.cwd, env=cli.environment)
        deadline = time.monotonic() + 60
        while True:
            asked = time.monotonic()
            opened = attempt(cli, "--json", *scope, "case", "open", timeout=60)
            # Answered at once, before and after the close arrived.
            assert time.monotonic() - asked < 15, ("case open waited for the busy backend", opened)
            if opened.returncode:
                break
            assert time.monotonic() < deadline, "the close never reached the backend"
            time.sleep(0.1)
        assert error_code(opened.stderr) == "caseBackendClosing", opened
        # A command is refused rather than queued behind the close.
        status = attempt(cli, "--json", *scope, "case", "status", timeout=60)
        assert status.returncode == 1 and error_code(status.stderr) == "caseBackendClosing", status
        assert closer.poll() is None, "the close did not wait for the unlock"
        # Once the unlock's client is gone, the unlock is cancelled and the close completes.
        unlock.kill()
        unlock.wait(timeout=30)
        out, err = closer.communicate(timeout=120)
        assert closer.returncode == 0 and json.loads(out) == {"caseDir": str(case), "closed": True}, (out, err)
        deadline = time.monotonic() + 30
        while alive(backend) and time.monotonic() < deadline:
            time.sleep(0.05)
        assert not alive(backend), "backend did not stop"
        assert cli.json(*scope, "case", "status")["process"]["mode"] == "OneShot"
    finally:
        for process in (unlock, closer):
            if process is not None and process.poll() is None:
                process.kill()
        if alive(backend):
            # A backend that no longer answers must not outlive the test.
            if os.name == "nt":
                subprocess.run(["taskkill", "/F", "/PID", str(backend)], capture_output=True)
            else:
                os.kill(backend, signal.SIGKILL)
