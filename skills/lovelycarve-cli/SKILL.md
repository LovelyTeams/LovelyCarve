---
name: lovelycarve-cli
description: Analyze forensic evidence with lovelycarve-cli (disk images such as E01, VMDK, VHD/VHDX, QCOW and raw, folders, archives, AD1, iOS backups, optical discs, physical disks and RAID members) by browsing and reading files, hashing, searching contents, recovering deleted files, unlocking BitLocker, LUKS, VeraCrypt and FileVault volumes, building timelines, running plugins and exporting, with small structured results and persistent sessions. Use whenever a task involves lovelycarve-cli or command-line LovelyCarve analysis.
---

# lovelycarve-cli

`lovelycarve-cli [global options] <collection>… <verb> [subject] [options]` runs one command. Results go to stdout as JSON, compact when run by an agent or redirected, or as raw bytes for content commands; progress, prompts and errors go to stderr. The exit status is 0 on success, 1 on failure and 130 when interrupted. If `lovelycarve-cli` is not on `PATH`, use the executable at the root of the CLI package, which ships this skill in `skills/lovelycarve-cli`.

## Find the command

[references/commands.md](references/commands.md) lists every command by collection with its arguments, options and description. Search it for an English keyword (`hash`, `unlock`, `carve`) or a Chinese term from the descriptions (`哈希`, `解锁`, `导出`), or read one collection's section, instead of exploring with chained `--help` calls.

- `lovelycarve-cli <command> --help` shows one command's option descriptions and defaults.
- `lovelycarve-cli schema <command>` shows its JSON request: the camelCase parameter names that `--body` and JSONL use.
- JSON-valued options (`--options`, `--query`, `--body`) take inline JSON or `@<file>`. From PowerShell, prefer a file, because PowerShell can alter quotes in arguments to native programs.
- [references/workflows.md](references/workflows.md) covers multi-step tasks: paths and node IDs, time zones, content search, RAID and LVM, encrypted volumes, file recovery, plugins, hashes and exports, background tasks and special evidence.

## Choose evidence and case

`--image` (`-i`) takes a loaded image ID or an evidence path, opened on first use. Without it the CLI uses `LOVELYCARVE_IMAGE` (also read from `.env.local` or `.env` in the working directory) or the session's current image; `--case <dir>` (or `LOVELYCARVE_CASE`) opens a case first. With a case, `images open` registers the evidence and builds volume indexes in the background; without one, the indexes last only as long as the process. Content search, global search, timelines and plugin results need an open case. Do not open the same case in the desktop app and the CLI at the same time.

## Work in a session

Open the evidence once and reuse it across calls:

```text
lovelycarve-cli -q sessions open exam --image "<evidence-path>"
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam files list --path / --page-size 50 --fields name,id,kind,size,timestamps.modified
lovelycarve-cli -q --session exam context set --node-id "<directory-node-id>"
lovelycarve-cli -q sessions close exam
```

Use a unique session name and close it when finished. A session keeps the case, the images, unlocked and assembled volumes, the current directory and background tasks; a one-shot command loses all of them when it exits. Relative host paths resolve against the directory where the session started, so pass absolute paths. `-q` suppresses progress; failures still exit nonzero.

## Keep results small

Request small pages and only the fields you need: `--fields` takes comma-separated entry paths, and dot paths keep nesting. Follow `nextCursor` with `--cursor` until it is null; never treat a truncated tool result or `truncated: true` as complete. Include `timestampsOriginal` in `--fields` to see the original times; it is absent when they are identical. Time zone provenance is reported once per response (`timestampDisplay`), and times with an unknown offset stay unconverted.

Filter a projected page with `jq` before it reaches your context. Pipe a `files list` response to:

```sh
jq -c '.items |= map(select(.kind == "file" and .size > 1048576))'
```

This keeps the pagination and time zone metadata and selects files larger than 1 MiB. Filtering applies only to the current page: keep following `nextCursor` even when a filtered page is empty. Prefer native filters such as `files list --query` to reduce backend output too. For JSONL, select the intended response's `.result` first.

## Paths and node IDs

Paths inside the evidence use `/`. `/` lists the partitions and volumes, and a path without a leading `/` is relative to the current directory. Use the node IDs that commands return (`--node-id`, `--parent-id`, `--root-id`) for duplicate names, including duplicate volume names, deleted entries and special streams; a path's first segment may also be a root node ID, as in `/part:0/Windows`. IDs belong to the opened evidence and session: do not construct them. `context set --node-id` changes the working directory unambiguously. See [references/workflows.md](references/workflows.md#paths-and-node-ids) for name rules.

## Batch with JSONL

`lovelycarve-cli -q session` reads one request per line on stdin and writes one response per line, `{"id":…,"result":…}` or `{"id":…,"error":…}`, in completion order; match them by `id`. `args` uses the camelCase names from `schema`. Optional top-level keys sit beside `args`: `fields` (a comma-separated string), `timezone`, `output` (writes a large result to a new host file), `force` and `dryRun`. `{"$ref":"<id>#/<JSON pointer>"}` reuses a value from an earlier successful response. `--jobs N` lets queries overlap; commands that change state still run in order. Use `files text get` for decoded text in JSONL; `files read` returns hex there.

```json
{"id":"text","command":"files text get","args":{"path":"<image-path>"}}
```

## Integrity and safety

- Evidence is opened read-only. Commands that write host files (`--destination`, JSONL `output`) refuse existing paths: choose a new path.
- Destructive commands need `--force` (JSONL `"force": true`) when nobody can confirm; check them with `--dry-run` first. Without a terminal, a missing required value fails instead of prompting.
- Keep secrets off the command line: pass `--password @<file>` or give the option without a value and pipe the secret on stdin. `--dry-run` redacts secrets.
- When `readFaultsDetected` is true, `badRegions` is nonempty or `syntheticBytes` is nonzero, a hash or export includes synthesized zeroes: do not claim those bytes or digests authenticate the source.
- A failed raw read may already have written partial stdout: discard it. Check full logical lengths, byte counts and the exit status before using any output.
- Unsupported or incomplete evidence is not empty evidence: report the error instead of concluding nothing is there. Format capabilities and limits are documented per format under `docs/formats/` in the LovelyCarve repository (https://github.com/LovelyTeams/LovelyCarve).
