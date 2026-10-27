---
name: lovelycarve-cli
description: Use lovelycarve-cli for forensic analysis of disk images, folders, archives, backups, physical disks and RAID members. Browse, search, recover, unlock, hash and export evidence through persistent sessions and structured results. Applies to CLI analysis, not LovelyCarve source development or desktop UI operation.
---

# lovelycarve-cli

Analyze evidence read-only with `lovelycarve-cli [global options] <collection>… <verb> [subject] [options]`. Results go to stdout as JSON (compact for agents), or raw bytes for content commands; progress and errors go to stderr.

Resolve the skill root from this `SKILL.md`'s absolute location. Prefer its `bin/lovelycarve-cli.exe` on Windows (`bin/lovelycarve-cli` on Unix, when supplied); every bare `lovelycarve-cli` example below means that resolved executable, not a requirement to add it to `PATH`. Keep the adjacent DLLs, plugin loader, `plugins/` and any `resources/` together. Do not change the working directory to `bin` just to run it. If no bundled executable exists, use the standalone CLI package root or `PATH`; in a source checkout, `just stage-cli-skill` stages an existing release and `just run-cli` runs a development build. The staged Windows x64 runtime is platform-specific; `system get` reports its version and optional features.

Quote executable paths; PowerShell needs the call operator `&`. Git Bash / MSYS2 rewrites arguments that start with `/` into host paths even when quoted: the CLI turns rewritten evidence paths back, but any other such argument (a search pattern like `/etc/passwd`) needs `MSYS2_ARG_CONV_EXCL='*'` on the call. See [shell invocation](references/workflows.md#shell-invocation-and-argument-files) for JSON files and shell-specific examples.

## Choose the workflow

Read only the relevant sections of [workflows.md](references/workflows.md). Search [commands.md](references/commands.md) by collection or keyword (`hash` / `哈希`, `unlock` / `解锁`, `export` / `导出`) for exact commands and options; do not load the whole catalogue by default.

| Task | Start here |
| --- | --- |
| Open evidence, select a case, reuse a process | [Sessions and cases](references/workflows.md#sessions-and-cases) |
| Find a file, read text or inspect a native format | [Find and inspect files](references/workflows.md#find-and-inspect-files), [paths and node IDs](references/workflows.md#paths-and-node-ids) |
| Search file contents across a subtree or evidence | [Content search](references/workflows.md#content-search-and-global-search) |
| Assemble disks or unlock a volume | [RAID and LVM](references/workflows.md#raid-lvm-and-assembled-volumes), [BitLocker](references/workflows.md#bitlocker), [LUKS](references/workflows.md#luks), [VeraCrypt and FileVault](references/workflows.md#luks-veracrypt-and-filevault) |
| Recover deleted records or carve signatures | [File recovery](references/workflows.md#file-recovery) |
| Run analyzers or query events | [Plugin analysis](references/workflows.md#plugin-analysis), [timelines](references/workflows.md#timelines) |
| Hash or save evidence bytes | [Hashes and exports](references/workflows.md#hashes-exports-and-read-faults) |
| Batch requests, handle large results or wait for work | [JSONL](references/workflows.md#batch-with-jsonl), [pagination](references/workflows.md#pagination-and-projection), [background tasks](references/workflows.md#background-tasks) |

Every command that resolves an evidence path takes it as `--path <path>` and a returned node ID as `--node-id <id>`; where the synopsis shows a positional `<path>` it may be written positionally instead, but `list` commands take no positional arguments. Use `<command> --help` for option defaults and `schema <command>` for top-level camelCase request keys. Nested JSON objects keep their own contract: recovery options and timeline queries include snake_case keys. `schema` names those types but does not expand their fields; use the workflow examples or the matching source type rather than guessing.

## Start once, inspect, then act

For a new investigation, create a case before loading evidence, then build and verify its volume indexes before filename or content searches. Use a unique session name; replace paths and IDs with actual values:

```text
lovelycarve-cli -q sessions open exam
lovelycarve-cli -q --session exam cases create "<new-absolute-case-dir>" --name "<case-name>"
lovelycarve-cli -q --session exam images open "<absolute-evidence-path>"
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes list
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam files list --path / --page-size 50 --fields name,id,kind,size
```

Reuse a user-selected existing case with `--case "<absolute-case-dir>"` when opening the session. Evidence registration builds indexes automatically; inspect `indexes list` for the selected image/volume (`indexed: true`, `stale: false`, `truncated: false`). If a required index is missing, get its volume key from `indexes volumes list`, run `indexes create --volume-key <key>`, wait, and check again. A listed volume alone does not prove its index is ready. Keep unlocked volumes, assembled arrays and recovery scans in the same session. Close a session you created only after required work finishes; leave a user's reused session open unless asked to close it. If the process exited, `sessionExited` clears its record: reopen the session and rebuild in-memory state.

Paths inside evidence use `/`; `/` lists volumes. Symbolic links are followed as a shell follows them: always in the middle of a path, and at its end when reading, hashing, exporting, decoding or listing; `files get` reports the link itself with `linkTarget`. Copy returned node IDs for duplicate names, deleted entries and special streams; do not construct IDs. Relative host paths resolve against the session's starting directory, so use absolute host paths. Verify the selected image before acting: defaults may come from `LOVELYCARVE_IMAGE`, `LOVELYCARVE_CASE`, `LOVELYCARVE_SESSION` or local dotenv files.

## Bound output without losing evidence

Start with small pages and `--fields` containing identity plus the needed fields. For directory pages, follow `nextCursor` with `--cursor` until null; decoded text follows `nextOffset` with `--offset`; other collections use `skip`/`limit` or `offset`. A page, a bounded tree walk or a tool-truncated response cannot establish that no other matches exist. Prefer native filtering and decoded `files text get` or a format-specific parser over dumping raw bytes.

For filename patterns use `indexes files list --filter` with `nameGlob`, scoped by `pathPrefix` if needed; it filters before paging. See [filename glob search](references/workflows.md#filename-glob-search). `files search` searches contents, and `jq` only filters results already returned.

## Check the outcome

| Exit | Meaning |
| --- | --- |
| `0` | Command succeeded; inspect task outcomes and result coverage too |
| `1` | Failure, including not found or a failed background task |
| `2` | Incomplete result; inspect the reported fields and warnings |
| `130` | Interrupted |

- Under a coding agent, a failure is one JSON line on stderr, `{"error":{"code":…,"message":…}}` (with `details` and `ipcCommand` when known), and an incomplete result adds `{"warning":{…}}` after the stdout result. Branch on `code`: `notFound`, `alreadyExists`, `isDirectory`, `usage`, `sessionExited`, `readFaultsDetected`, ….
- JSONL reports each request through `result`, `error` and optional `warning`; a failed request does not stop subsequent lines. Inspect the relevant responses before submitting dependent work.
- `readFaultsDetected`, nonempty `badRegions` or nonzero `syntheticBytes` mean synthesized zeroes contributed to a hash or export. Report the audit; those digests do not authenticate the missing source bytes. Check logical lengths and byte counts. A failed raw read may already have emitted partial stdout: discard it.
- Preserve timestamp role and provenance. Display conversion does not establish an evidence time zone; unknown-offset times stay unconverted. See [time zones](references/workflows.md#file-times-and-time-zones).
- Exports never overwrite: an existing destination fails with `alreadyExists` before anything is written, so a rerun needs new destinations (a fresh directory per batch run). Export a directory with `files directories export`; several chosen files with one `files export` request each in a JSONL session. For an authorized destructive command, inspect `--dry-run` before using `--force` (JSONL `"force": true`). Keep secrets out of arguments with `--password @<file>` or stdin; do not echo credential files or JSONL requests containing secrets.
- Report the evidence and file/volume identity, examined scope, findings, relevant times, output paths and any incomplete or unsupported coverage. Missing source bytes, failed indexes and unsupported formats cannot establish absence of evidence. Format limits live under `docs/formats/` in the [LovelyCarve repository](https://github.com/LovelyTeams/LovelyCarve).
