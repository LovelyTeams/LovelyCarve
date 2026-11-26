"""Output for people and for programs: text by default, --json for machines,
--json=fields to keep only some fields of list entries, cursors to page, one
JSON error line with --json, and the same results through batch and the
case backend."""
import json
import os
import sqlite3

from cli_support import request, says

FIELDS = "name,id,kind,size,timestamps.modified"


def test_output(cli, root):
    evidence = root / "System32"
    evidence.mkdir()
    for index in range(520):
        path = evidence / f"system-library-{index:04}.dll"
        path.write_bytes(b"fixture\n")
        os.utime(path, (1719792000, 1719792000))
    with sqlite3.connect(evidence / "zz-config.db") as database:
        database.execute("CREATE TABLE tree (inode INTEGER PRIMARY KEY, name TEXT, data BLOB)")
        database.execute("INSERT INTO tree VALUES (?, ?, ?)", (1423, "pve-ssl.pem", b"synthetic record"))
    database.close()
    scope = ("--image", evidence, "--tz", "UTC")
    folder = "/System32/folder:"

    # People get tables and a footer that says how to read on.
    text = cli.run(*scope, "ls", folder, "--limit", "5").stdout
    assert "system-library-0000.dll" in text and "{" not in text, text
    assert "--cursor" in text, text
    # Programs get one compact JSON line per result, cursors before the items.
    page = cli.run(*scope, "--json", "ls", folder, "--limit", "50")
    data = json.loads(page.stdout)
    assert len(data["items"]) == 50 and data["nextCursor"], data
    assert page.stdout.count("\n") == 1 and not page.stderr, page.stderr
    assert page.stdout.index('"nextCursor"') < page.stdout.index('"items"')
    assert all("timestampDisplay" not in item and "timestampsOriginal" not in item for item in data["items"])
    projected = cli.run(*scope, f"--json={FIELDS}", "ls", folder, "--limit", "50")
    small = json.loads(projected.stdout)
    assert set(small["items"][0]) == {"name", "id", "kind", "size", "timestamps"}, small["items"][0]
    assert set(small["items"][0]["timestamps"]) == {"modified"}
    assert small["timestampDisplay"] == data["timestampDisplay"] and small["nextCursor"] == data["nextCursor"]
    assert len(projected.stdout) < 12_000, len(projected.stdout)
    filtered = cli.run(*scope, "--json=name", "ls", folder, "--limit", "3",
                       "--jq", "{nextCursor, items: [.items[] | select(.size == null)]}")
    selection = json.loads(filtered.stdout)
    assert selection["nextCursor"] and [set(item) for item in selection["items"]] == [{"name"}] * 3, selection
    names = cli.run(*scope, "--jq=.items[].name", "ls", folder, "--limit", "3")
    assert names.stdout.splitlines() == [item["name"] for item in selection["items"]] and not names.stderr
    assert cli.run(*scope, "ls", folder, "--jq", ".items[] | select(.size > 200)").stdout == ""
    assert cli.run("system", "--jq", ".exitCodes.incompleteResult").stdout == "2\n"
    assert cli.run("--jq", '.command', "--dry-run", "ls", folder).stdout == "ls\n"
    following = cli.json(*scope, "ls", folder, "--limit", "50", "--cursor", small["nextCursor"])
    assert len(following["items"]) == 50
    assert not {item["id"] for item in small["items"]} & {item["id"] for item in following["items"]}
    assert len(cli.json(*scope, "ls", folder)["items"]) == 200
    assert len(cli.json(*scope, "ls", folder, "--limit", "500")["items"]) == 500
    everything = cli.json(*scope, "ls", folder, "--all")
    assert len(everything["items"]) == 521 and everything["nextCursor"] is None
    for invalid in ("name,,id", "timestamps..modified"):
        cli.run(*scope, f"--json={invalid}", "ls", folder, expected=1)
    # Raw content is JSON too with --json; fields belong to lists only, and are
    # refused before anything is read.
    dll = folder + "/system-library-0000.dll"
    assert cli.json(*scope, "cat", dll) == {"encoding": "hex", "data": b"fixture\n".hex()}
    assert cli.run(*scope, "cat", dll, "--jq", ".data").stdout == b"fixture\n".hex() + "\n"
    refused = cli.run(*scope, "--json=id", "cat", dll, expected=1)
    assert refused.stdout == "" and json.loads(refused.stderr)["error"]["code"] == "usage", refused

    # Failures: one JSON line with --json, a framed block for people.
    failure = cli.run("--json", "lss", expected=1)
    error = json.loads(failure.stderr)["error"]
    assert error["code"] == "usage" and len(failure.stderr) < 1000, failure.stderr
    invalid_case = root / "invalid-filter-case"
    for expression in (".[", "unknown_filter", 'import "external" as $data; $data', "halt", '"note" | stderr'):
        failure = cli.run("case", "new", invalid_case, "--jq", expression, expected=1)
        assert failure.stdout == "" and json.loads(failure.stderr)["error"]["code"] == "usage", failure
        assert not invalid_case.exists()
    evaluation = cli.run("system", "--jq", 'error("query failed")', expected=1)
    assert json.loads(evaluation.stderr)["error"]["code"] == "invalidInput", evaluation
    for command in ("shell", "batch"):
        failure = cli.run(command, "--jq", ".", expected=1)
        assert failure.stdout == "" and json.loads(failure.stderr)["error"]["code"] == "usage", failure
    missing = json.loads(cli.run(*scope, "--json", "stat", folder + "/missing", expected=1).stderr)["error"]
    assert missing["code"] == "notFound" and "「missing」" in missing["message"], missing
    assert "┌" in cli.run("lss", expected=1).stderr
    assert says(cli.run(*scope, "stat", folder + "/missing", expected=1).stderr, "「missing」")
    rows = cli.json(*scope, "sqlite", "rows", folder + "/zz-config.db", "tree")
    assert rows["rows"][0][:2] == [{"kind": "integer", "value": 1423}, {"kind": "text", "value": "pve-ssl.pem"}], rows
    large = cli.run(*scope, "sqlite", "query", folder + "/zz-config.db", "SELECT 9223372036854775807",
                    "--jq", ".rows[0][0].value | tonumber | [., . + 1, 18446744073709551616]")
    assert json.loads(large.stdout) == [9223372036854775807, 9223372036854775808, 18446744073709551616], large

    # Batch requests take the same options, per request.
    rows = cli.results([
        request("open", "evidence", "add", evidence),
        request("list", f"--json={FIELDS}", "--tz", "UTC", "ls", folder, "--limit", "3"),
        request("default", "ls", folder),
        request("text", "text", dll),
    ])
    assert len(rows["list"]["items"]) == 3 and set(rows["list"]["items"][0]) == set(small["items"][0]), rows["list"]
    assert len(rows["default"]["items"]) == 200
    assert rows["text"]["text"] == "fixture\n", rows["text"]
    _, rejected = cli.batch([request("filter", "system", "--jq", ".")], expected=1)
    assert rejected[0]["error"]["code"] == "usage", rejected

    # A case backend answers exactly as the process would have; file names are
    # found in the volume index, filtered before paging.
    case = root / "case"
    cli.run("case", "new", case, "--name", "Filename glob")
    cli.run("-C", case, "evidence", "add", evidence, "--wait")
    glob = "SYSTEM-LIBRARY-00[12]?.DLL"
    local = cli.json("-C", case, "find", glob, "--limit", "3", "--offset", "2")
    assert [item["name"] for item in local["items"]] == [f"system-library-{index:04}.dll" for index in range(12, 15)], local
    assert local["items"][0]["path"] == folder + "/system-library-0012.dll", local["items"][0]
    cli.json("-C", case, "case", "open")
    try:
        remote = json.loads(cli.run("-C", case, "--tz", "UTC", f"--json={FIELDS}", "ls", folder, "--limit", "50").stdout)
        assert remote == small, (remote, small)
        assert cli.run("-C", case, "ls", folder, "--limit", "3", "--jq", ".items[].name").stdout == names.stdout
        assert cli.json("-C", case, "find", glob, "--limit", "3", "--offset", "2") == local
        later = cli.json("-C", case, "find", glob, "--limit", "3", "--offset", "5")
        assert [item["name"] for item in later["items"]] == [f"system-library-{index:04}.dll" for index in range(15, 18)]
        assert cli.json("-C", case, "cat", dll) == {"encoding": "hex", "data": b"fixture\n".hex()}
        assert cli.run("-C", case, "cat", dll, "--jq", ".data").stdout == b"fixture\n".hex() + "\n"
        assert cli.run("-C", case, "cat", dll, binary=True).stdout == b"fixture\n"
    finally:
        cli.run("-C", case, "case", "close")
    # Without a case there is no volume index to search, and find says so.
    assert says(cli.run(*scope, "find", glob, expected=1).stderr, "卷索引")
    print("CLI output tests passed: text and JSON, projection, jq, cursors, raw content as JSON, error lines, batch options, "
          "backend parity and indexed name globs")
