---
name: lovelycarve-cli
description: Use lovelycarve-cli for forensic analysis of disk images, folders, archives, backups, physical disks and RAID members. Browse, search, recover, unlock, hash and export evidence through persistent sessions and structured results. Applies to CLI analysis, not LovelyCarve source development or desktop UI operation.
---

# lovelycarve-cli

Analyze evidence read-only with `lovelycarve-cli [global options] <collection>… <verb> [subject] [options]`. Results go to stdout as JSON (compact for agents), or raw bytes for content commands; progress and errors go to stderr.

Set `LOVELYCARVE_AGENT=1` in the CLI child process environment for explicit machine output, including under DSH: compact JSON, JSON errors/warnings, bounded default directory pages and no interactive prompts or terminal colors. `system get` reports `output.agentMode`. Use `-q` to suppress progress. False-like values (`0`, `false`, `no`, `off`, empty) disable agent detection even when a harness marker is inherited.

When DSH supplies the `lovelycarve` tool, prefer it for structured CLI calls. Give the full command in `command`, API parameter names in the `args` object, and the resident name in `session`. For lifecycle commands such as `sessions open`, omit `session` and pass `args.name`. Inspect `exitCode`, `error`, `warning` and `outputTruncated` before dependent requests; tool truncation is not a usable partial result. The tool preserves pagination/provenance, enables agent mode and reuses the DSH executor's policy and cancellation. Use `files text get` for text and `files export` for exact bytes.

Resolve the skill root from this `SKILL.md`'s absolute location. Prefer its `bin/lovelycarve-cli.exe` on Windows (`bin/lovelycarve-cli` on Unix, when supplied); every bare `lovelycarve-cli` example below means that resolved executable, not a requirement to add it to `PATH`. Keep the adjacent DLLs, plugin loader, `plugins/` and any `resources/` together. Do not change the working directory to `bin` just to run it. If no bundled executable exists, use the standalone CLI package root or `PATH`; in a source checkout, `just stage-cli-skill` stages an existing release and `just run-cli` runs a development build. The staged Windows x64 runtime is platform-specific; `system get` reports its version and optional features.

Quote executable paths; PowerShell needs the call operator `&`. Git Bash / MSYS2 rewrites arguments that start with `/` into host paths even when quoted: the CLI turns rewritten evidence paths back, but any other such argument (a search pattern like `/etc/passwd`) needs `MSYS2_ARG_CONV_EXCL='*'` on the call. See [shell invocation](references/workflows.md#shell-invocation-and-argument-files) for JSON files and shell-specific examples.

Discover DSH requests without a session using `{"command":"schema files list"}` or `{"command":"files list","help":true}`. Keep `command` free of `--help` and positional values. A complete listing request is `{"command":"files list","session":"exam","args":{"path":"/","pageSize":20},"fields":"id,name,kind,size"}`. `fields` belongs at the top level. `indexes list` is case-wide and takes no `imageId`; inspect its returned rows to select an image. Close with `{"command":"sessions close","args":{"name":"exam"}}`, without `session`. On `usage`, inspect `error.details.allowedFields`, `schemaRequest` and any `retryRequest`; review and explicitly retry the correction. The tool never executes a correction automatically.

DSH returns the CLI's resolved `source` with evidence path, image ID, node ID and available volume/path identity. Cite that source rather than the session's later selection. SQLite `recordKeys` identify database records within the source table, not filesystem nodes; never construct a file node ID from a database primary key. For ordinary CLI JSON commands, add `--with-source` to opt into `{"source":…, "result":…}`; JSONL uses top-level `"withSource":true`. Raw byte commands do not support this envelope.

## Choose the workflow

Read only the relevant sections of [workflows.md](references/workflows.md). Search [commands.md](references/commands.md) by collection or keyword (`hash` / `哈希`, `unlock` / `解锁`, `export` / `导出`) for exact commands and options; do not load the whole catalogue by default.

| Task | Start here |
| --- | --- |
| Solve forensic questions with forensic-board MCP, save answers and reuse clues | [Questions, answers and clues](references/forensic-board.md) |
| Open evidence, select a case, reuse a process | [Sessions and cases](references/workflows.md#sessions-and-cases) |
| Find a file, read text or inspect a native format | [Find and inspect files](references/workflows.md#find-and-inspect-files), [paths and node IDs](references/workflows.md#paths-and-node-ids) |
| Search file contents across a subtree or evidence | [Content search](references/workflows.md#content-search-and-global-search) |
| Identify Linux/PVE software or kernel versions | [Linux and PVE versions](references/workflows.md#linux-and-pve-versions) |
| Assemble disks or unlock a volume | [RAID and LVM](references/workflows.md#raid-lvm-and-assembled-volumes), [BitLocker](references/workflows.md#bitlocker), [LUKS](references/workflows.md#luks), [VeraCrypt and FileVault](references/workflows.md#luks-veracrypt-and-filevault) |
| Recover deleted records or carve signatures | [File recovery](references/workflows.md#file-recovery) |
| Run analyzers or query events | [Plugin analysis](references/workflows.md#plugin-analysis), [timelines](references/workflows.md#timelines) |
| Hash or save evidence bytes | [Hashes and exports](references/workflows.md#hashes-exports-and-read-faults) |
| Batch requests, handle large results or wait for work | [JSONL](references/workflows.md#batch-with-jsonl), [pagination](references/workflows.md#pagination-and-projection), [background tasks](references/workflows.md#background-tasks) |

Every command that resolves an evidence path takes it as `--path <path>` and a returned node ID as `--node-id <id>`; where the synopsis shows a positional `<path>` it may be written positionally instead, but `list` commands take no positional arguments. Use `<command> --help` for option defaults and `schema <command>` for top-level camelCase request keys. Nested JSON objects keep their own contract: recovery options and timeline queries include snake_case keys. `schema` expands common file filters and directory queries. For other nested types, use the workflow examples or matching source type rather than guessing.

## Questions and shared clues

When solving questions with forensic-board MCP, read [forensic-board.md](references/forensic-board.md) first. Use the host's existing connection: the board owns questions, candidate answers and shared clues; the CLI owns evidence access and analysis. Start or resume from the board's current question and clues, then verify the CLI session, evidence and index state. Keep board question IDs separate from CLI image, volume and node IDs.

After each evidence result, check whether it answers any authorized question. Submit each supported answer immediately, including answers found for other questions: refresh that question, deduplicate, submit or update, then read back the answer ID, value, reasoning and evidence before continuing analysis. Save useful clues with their original value and reproducible source as they are found. A submission timeout requires reading back before retrying; a candidate stored on the board is distinct from a confirmed correct answer or a submission to a competition platform. Apply this workflow when the board is part of the task; ordinary CLI analysis does not require it.

## Start once, inspect, then act

Start with the smallest evidence set that can answer the question. Record supplied identity metadata; compute hashes only when requested or needed for verification, and honor an explicit request to skip them. Every investigation starts by creating a case or opening the user-selected existing case before loading evidence. Wait for its tasks and verify the relevant volume indexes before browsing, reading or analysis, including known-path reads. Record unsupported, encrypted or unindexable volumes as coverage limitations. Use a unique session name; replace paths and IDs with actual values:

```text
lovelycarve-cli -q sessions open exam
lovelycarve-cli -q --session exam cases create "<new-absolute-case-dir>" --name "<case-name>"
lovelycarve-cli -q --session exam images open "<absolute-evidence-path>"
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes list
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam files list --path / --page-size 50 --fields name,id,kind,size
```

Reuse a user-selected existing case with `--case "<absolute-case-dir>"` when opening a new session. With an already running `--session`, switch explicitly using `cases open`; a global `--case` is rejected. Never open one case concurrently in multiple sessions or the desktop app. Evidence registration builds indexes automatically; inspect `indexes list` for the selected image/volume (`indexed: true`, `stale: false`, `truncated: false`). If a required index is missing, get its volume key from `indexes volumes list`, run `indexes create --volume-key <key>`, wait, and check again. A listed volume alone does not prove its index is ready. Keep unlocked volumes, assembled arrays and recovery scans in the same session. Close a session you created only after required work finishes; leave a user's reused session open unless asked to close it. If the process exited, `sessionExited` clears its record: reopen the session and rebuild in-memory state.

Paths inside evidence use `/`; `/` lists volumes. Symbolic links are followed as a shell follows them: always in the middle of a path, and at its end when reading, hashing, exporting, decoding or listing; `files get` reports the link itself with `linkTarget`. Copy returned node IDs for duplicate names, deleted entries and special streams; do not construct IDs. Relative host paths resolve against the session's starting directory, so use absolute host paths. Verify the selected image before acting: defaults may come from `LOVELYCARVE_IMAGE`, `LOVELYCARVE_CASE`, `LOVELYCARVE_SESSION` or local dotenv files.

## Bound output without losing evidence

Start with small pages and `--fields` containing identity plus the needed fields. `--fields` applies only to `list` / `search`, not `get`, text reads or mutations. For directory pages, follow `nextCursor` with `--cursor` until null; decoded text follows `nextOffset` with `--offset`; other collections use `skip`/`limit` or `offset`. A page, a bounded tree walk or a tool-truncated response cannot establish that no other matches exist. After tool output overflow, use `recovery.retryRequest` to reread the current page with its original cursor/offset; the discarded page’s next cursor must not skip those entries. Prefer native filtering and decoded `files text get` or a format-specific parser over dumping raw bytes.

For filename patterns use `indexes files list --filter` with `nameGlob`, scoped by `pathPrefix` if needed; it filters before paging. See [filename glob search](references/workflows.md#filename-glob-search). `files search` searches contents, and `jq` only filters results already returned.

## Check the outcome

| Exit | Meaning |
| --- | --- |
| `0` | Command succeeded; inspect task outcomes and result coverage too |
| `1` | Failure, including not found or a failed background task |
| `2` | Incomplete result; inspect the reported fields and warnings |
| `130` | Interrupted |

- Under a coding agent, a failure is one JSON line on stderr, `{"error":{"code":…,"message":…}}` (with `details` and `ipcCommand` when known), and an incomplete result adds `{"warning":{…}}` after the stdout result. Branch on `code`: `notFound`, `alreadyExists`, `isDirectory`, `usage`, `sessionExited`, `readFaultsDetected`, ….
- In PowerShell, capture native stdout, inspect `$LASTEXITCODE` immediately, then decode JSON or run dependent commands. Exit 1/130 stops the dependent chain; exit 2 requires inspecting the incomplete result. Do not let `Out-Null`, a pipeline or a later successful command hide an earlier error.
- JSONL reports each request through `result`, `error` and optional `warning`; a failed request does not stop subsequent lines. Inspect the relevant responses before submitting dependent work.
- `readFaultsDetected`, nonempty `badRegions` or nonzero `syntheticBytes` mean synthesized zeroes contributed to a hash or export. Report the audit; those digests do not authenticate the missing source bytes. Check logical lengths and byte counts. A failed raw read may already have emitted partial stdout: discard it.
- Preserve timestamp role and provenance. Display conversion does not establish an evidence time zone; unknown-offset times stay unconverted. See [time zones](references/workflows.md#file-times-and-time-zones).
- Exports never overwrite: an existing destination fails with `alreadyExists` before anything is written, so a rerun needs new destinations (a fresh directory per batch run). Export a directory with `files directories export`; several chosen files with one `files export` request each in a JSONL session. For an authorized destructive command, inspect `--dry-run` before using `--force` (JSONL `"force": true`). Keep secrets out of arguments with `--password @<file>` or stdin; do not echo credential files or JSONL requests containing secrets.
- Report the evidence and file/volume identity, examined scope, findings, relevant times, output paths and any incomplete or unsupported coverage. Missing source bytes, failed indexes and unsupported formats cannot establish absence of evidence. Format limits live under `docs/formats/` in the [LovelyCarve repository](https://github.com/LovelyTeams/LovelyCarve).
