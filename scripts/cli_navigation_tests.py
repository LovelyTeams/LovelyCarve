"""Caseless node IDs and image paths through the actual CLI transports."""
import json
from pathlib import Path
import subprocess
import tempfile


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
    print("CLI navigation tests passed: caseless IDs, volume ID paths, shell/JSONL directory IDs, NTFS case fallback and absolute tree paths")
