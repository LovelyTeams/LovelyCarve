"""Process-level resident-session checks, called by the CLI smoke recipe."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time


def test_resident_sessions(executable, base_environment):
    with tempfile.TemporaryDirectory(prefix="lovelycarve-resident-") as directory:
        root = Path(directory)
        environment = {key: value for key, value in base_environment.items()
                       if key not in {"LOVELYCARVE_IMAGE", "LOVELYCARVE_CASE", "LOVELYCARVE_SESSION"}}
        environment["LOVELYCARVE_SESSION_DIR"] = str(root / "sessions")
        evidence = root / "evidence"
        evidence.mkdir()
        (evidence / "sub").mkdir()
        content = b"resident evidence\x00\xff\n"
        (evidence / "sub" / "report.bin").write_bytes(content)
        # Split a multi-byte UTF-8 character at the transport's 8 MiB boundary.
        large = b"x" * (8 * 1024 * 1024 - 1) + "中".encode() + b"z" * (9 * 1024 * 1024)
        (evidence / "large.txt").write_bytes(large)
        active = set()

        def run(*arguments, stdin=None, expected=0, env=None, binary=False):
            result = subprocess.run([str(executable), "--quiet", *map(str, arguments)],
                                    input=stdin, capture_output=True, cwd=root,
                                    env=environment if env is None else env, timeout=120,
                                    **({} if binary else {"text": True, "encoding": "utf-8"}))
            assert result.returncode == expected, (arguments, result.returncode, result.stdout[:1000], result.stderr)
            return result

        def value(*arguments, **options):
            return json.loads(run(*arguments, **options).stdout)

        try:
            initial = value("--image", evidence, "sessions", "open", "exam")
            active.add("exam")
            image_id = initial["context"]["image"]["id"]
            assert image_id and initial["imageCount"] == 1, initial
            pid = initial["pid"]
            assert pid != os.getpid() and initial["workingDirectory"] == str(root), initial
            assert value("sessions", "open", "exam")["pid"] == pid
            run("--image", evidence, "sessions", "open", "exam", expected=1)

            # Twenty-six separate clients keep the same actual runtime image.
            for _ in range(26):
                context = value("--session", "exam", "context", "get")
                assert context["image"]["id"] == image_id, context
            assert value("sessions", "get", "exam")["pid"] == pid
            roots = value("--session", "exam", "files", "list")
            assert roots["total"] >= 1, roots
            root_id = roots["items"][0]["id"]
            run("sessions", "attach", "exam", stdin=f"cd --node-id {root_id}\nfiles get sub\nexit\n")
            assert value("--session", "exam", "context", "get")["nodeId"] == root_id
            run("--session", "exam", "context", "set", "--dir", f"/{evidence.name}/sub")
            assert run("--session", "exam", "files", "read", "report.bin", binary=True).stdout == content
            run("sessions", "attach", "exam", stdin="pwd\ncd ..\nfiles get large.txt\nexit\n")
            assert value("--session", "exam", "context", "get")["dir"] == f"/{evidence.name}"
            assert value("sessions", "get", "exam")["pid"] == pid
            assert run("--session", "exam", "files", "read", "large.txt", binary=True).stdout == large
            assert run("--session", "exam", "files", "read", "large.txt", "--text").stdout == large.decode()
            assert run("--session", "exam", "files", "read", "large.txt", "--offset", "8388606", "--length", "6", binary=True).stdout == large[8388606:8388612]
            run("--session", "exam", "files", "get", "missing", expected=1)
            run("--session", "exam", "cases", "delete", root / "never-delete", expected=1)
            assert value("sessions", "get", "exam")["pid"] == pid

            implicit = dict(environment, LOVELYCARVE_SESSION="exam")
            assert value("context", "get", env=implicit)["image"]["id"] == image_id
            run("--session", "exam", "--case", root / "unused", "context", "get", expected=1)
            run("--session", "absent", "files", "list", expected=1)
            run("sessions", "open", "../escape", expected=1)
            run("sessions", "attach", "exam", stdin="sessions open nested\n", expected=1)

            # A second namespace has independent evidence and directory context.
            second = value("sessions", "open", "other")
            active.add("other")
            assert second["context"]["image"] is None and second["pid"] != pid
            assert value("--session", "other", "context", "get", env=implicit)["image"] is None
            assert value("sessions", "get", "exam")["context"]["image"]["id"] == image_id

            # Real assembled evidence survives multiple clients, including use by
            # UUID and assembled:// reference, without another assembly command.
            fixture = Path(__file__).resolve().parents[1] / "src-tauri/tests/data/encrypted/plain.img"
            member = value("--session", "other", "images", "open", fixture)
            params = {"member_image_ids": [member["id"]], "level": 1, "layout": 0,
                      "chunk_sectors": 128, "data_offset_sectors": 0}
            assembled = value("--session", "other", "raid", "assemble", "--params", json.dumps(params))
            assembled_id = assembled["imageId"]
            run("--session", "other", "context", "use", assembled_id)
            assert value("--session", "other", "context", "get")["image"]["id"] == assembled_id
            images = value("--session", "other", "images", "list")
            assembled_image = next(image for image in images if image["id"] == assembled_id)
            assert value("--session", "other", "files", "list")["total"] >= 1
            assert value("--session", "other", "images", "open", assembled_image["path"])["id"] == assembled_id

            # Public status must not reveal the private transport credential.
            public = run("sessions", "list").stdout
            assert '"token"' not in public and len(json.loads(public)["sessions"]) == 2, public
            descriptor = json.loads((root / "sessions/session-exam.json").read_text())
            with socket.create_connection(("127.0.0.1", descriptor["port"]), timeout=5) as connection:
                connection.sendall(b'{"token":"wrong","command":"@close"}\n')
                response = json.loads(connection.makefile("rb").readline())
                assert response["kind"] == "error" and response["error"]["code"] == "permissionDenied", response
            assert descriptor["token"] not in public
            assert value("sessions", "get", "exam")["pid"] == pid

            # Closing releases the case lock so an ordinary fresh process can
            # open it, and leaves the other resident session available.
            case = root / "case"
            value("--session", "other", "cases", "create", case, "--name", "Resident test")
            value("sessions", "close", "other")
            active.remove("other")
            assert not (root / "sessions/session-other.json").exists()
            value("--case", case, "cases", "get")
            assert value("sessions", "get", "exam")["pid"] == pid

            # A crash leaves a stale record; clients fail explicitly, and opening
            # the same name recovers without trusting a possibly reused PID.
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                result = subprocess.run([str(executable), "--quiet", "--session", "exam", "context", "get"],
                                        capture_output=True, env=environment, cwd=root, timeout=15)
                if result.returncode != 0:
                    break
                time.sleep(0.05)
            else:
                raise AssertionError("resident process did not stop")
            run("--session", "exam", "files", "list", expected=1)
            recovered = value("sessions", "open", "exam")
            assert recovered["pid"] != pid and recovered["context"]["image"] is None, recovered
        finally:
            for name in active:
                run("sessions", "close", name)
        assert value("sessions", "list")["sessions"] == []
    print("Resident CLI tests passed: 26 clients, attach, shared context, RAID reuse, large binary/text reads, isolation, authentication, case release and crash recovery")
