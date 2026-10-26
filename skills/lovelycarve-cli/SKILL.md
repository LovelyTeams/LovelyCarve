---
name: lovelycarve-cli
description: Use lovelycarve-cli for forensic analysis of disk images, folders, archives, backups, physical disks and RAID members. Browse, search, recover, unlock, hash and export evidence through persistent sessions and structured results. Applies to CLI analysis, not LovelyCarve source development or desktop UI operation.
---

# lovelycarve-cli

Analyze evidence read-only with `lovelycarve-cli [global options] <collection>… <verb> [subject] [options]`. Results go to stdout as JSON (compact for agents), or raw bytes for content commands; progress and errors go to stderr. If the executable is not on `PATH`, look at the CLI package root alongside `skills/lovelycarve-cli`. In a source checkout, use `just run-cli` rather than assuming a packaged binary is installed.

## Choose the workflow

Read only the relevant sections of [workflows.md](references/workflows.md). Search [commands.md](references/commands.md) by collection or keyword (`hash` / `哈希`, `unlock` / `解锁`, `export` / `导出`) for exact commands and options; do not load the whole catalogue by default.

| Task | Start here |
| --- | --- |
| Open evidence, select a case, reuse a process | [Sessions and cases](references/workflows.md#sessions-and-cases) |
| Find a file, read text or inspect a native format | [Find and inspect files](references/workflows.md#find-and-inspect-files), [paths and node IDs](references/workflows.md#paths-and-node-ids) |
| Search file contents across a subtree or evidence | [Content search](references/workflows.md#content-search-and-global-search) |
| Assemble disks or unlock a volume | [RAID and LVM](references/workflows.md#raid-lvm-and-assembled-volumes), [BitLocker](references/workflows.md#bitlocker), [other encrypted volumes](references/workflows.md#luks-veracrypt-and-filevault) |
| Recover deleted records or carve signatures | [File recovery](references/workflows.md#file-recovery) |
| Run analyzers or query events | [Plugin analysis](references/workflows.md#plugin-analysis), [timelines](references/workflows.md#timelines) |
| Hash or save evidence bytes | [Hashes and exports](references/workflows.md#hashes-exports-and-read-faults) |
| Batch requests, handle large results or wait for work | [JSONL](references/workflows.md#batch-with-jsonl), [pagination](references/workflows.md#pagination-and-projection), [background tasks](references/workflows.md#background-tasks) |

Use `<command> --help` for option defaults and `schema <command>` for top-level camelCase request keys. Nested JSON objects keep their own contract: recovery options and timeline queries include snake_case keys. `schema` names those types but does not expand their fields; use the workflow examples or the matching source type rather than guessing. In PowerShell, pass JSON options with `@<absolute-file-path>` to preserve quotes.

## Start once, inspect, then act

Use a unique session name for related calls. Replace the example paths and IDs with values from the actual evidence:

```text
lovelycarve-cli -q sessions open exam --image "<absolute-evidence-path>"
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam files list --path / --page-size 50 --fields name,id,kind,size
```

For an existing case, add `--case "<absolute-case-dir>"` when opening the session. Content search, global search, timelines and plugin results need a case. Evidence registration starts background indexes; wait for the required indexes before searching. Keep unlocked volumes, assembled arrays and recovery scans in the same session. Close a session you created with `sessions close exam` after its required work finishes; closing cancels remaining work. Reusing a user's session does not imply permission to close it.

Paths inside evidence use `/`; `/` lists volumes. Copy returned node IDs for duplicate names, deleted entries and special streams; do not construct IDs. Relative host paths resolve against the session's starting directory, so use absolute host paths. Verify the selected image before acting: defaults may come from `LOVELYCARVE_IMAGE`, `LOVELYCARVE_CASE`, `LOVELYCARVE_SESSION` or local dotenv files.

## Bound output without losing evidence

Start with small pages and `--fields` containing identity plus the needed fields. For directory pages, follow `nextCursor` with `--cursor` until null; other collections use `skip`/`limit` or `offset`. A page, a bounded tree walk or a tool-truncated response cannot establish that no other matches exist. Prefer native filtering and decoded `files text get` or a format-specific parser over dumping raw bytes.

## Check the outcome

| Exit | Meaning |
| --- | --- |
| `0` | Command succeeded; inspect task outcomes and result coverage too |
| `1` | Failure, including not found or a failed background task |
| `2` | Incomplete result; inspect the reported fields and warnings |
| `130` | Interrupted |

- JSONL reports each request through `result`, `error` and optional `warning`; a failed request does not stop subsequent lines. Inspect the relevant responses before submitting dependent work.
- `readFaultsDetected`, nonempty `badRegions` or nonzero `syntheticBytes` mean synthesized zeroes contributed to a hash or export. Report the audit; those digests do not authenticate the missing source bytes. Check logical lengths and byte counts. A failed raw read may already have emitted partial stdout: discard it.
- Preserve timestamp role and provenance. Display conversion does not establish an evidence time zone; unknown-offset times stay unconverted. See [time zones](references/workflows.md#file-times-and-time-zones).
- Host exports refuse existing paths; choose a new destination. For an authorized destructive command, inspect `--dry-run` before using `--force` (JSONL `"force": true`). Keep secrets out of arguments with `--password @<file>` or stdin; do not echo credential files or JSONL requests containing secrets.
- Report the evidence and file/volume identity, examined scope, findings, relevant times, output paths and any incomplete or unsupported coverage. Missing source bytes, failed indexes and unsupported formats cannot establish absence of evidence. Format limits live under `docs/formats/` in the [LovelyCarve repository](https://github.com/LovelyTeams/LovelyCarve).
