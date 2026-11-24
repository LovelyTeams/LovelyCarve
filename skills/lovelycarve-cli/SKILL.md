---
name: lovelycarve-cli
description: Use lovelycarve-cli for forensic analysis of disk images, folders, archives, backups, physical disks and RAID members. Browse, search, recover, unlock, hash and export evidence by address (/<alias>/<volume>/<path>), with text or JSON results and a case backend that keeps evidence open. Applies to CLI analysis, not LovelyCarve source development or desktop UI operation.
---

# lovelycarve-cli

Analyze evidence read-only. Reading inside evidence uses the verbs people know — `ls`, `tree`, `stat`, `cat`, `text`, `strings`, `hash`, `find`, `grep`, `export` — and everything else is `<noun> <verb>`: `case new`, `evidence add`, `unlock`, `carve start`, `results rows`. Results are text for people. Add `--json` for one JSON document on stdout (raw content becomes `{"encoding":"hex","data":…}`, at most 16 MiB); then progress stays off and a failure is one JSON line on stderr.

Resolve the skill root from this `SKILL.md`'s absolute location. Prefer its `bin/lovelycarve-cli.exe` on Windows (`bin/lovelycarve-cli` on Unix, when supplied); every bare `lovelycarve-cli` example means that resolved executable, not a requirement to add it to `PATH`. Keep the adjacent DLLs, `lc-plugin-host`, `plugins/` and any `resources/` together; do not change the working directory to `bin`. Without a bundled executable use the CLI package root or `PATH`; in a source checkout, `just stage-cli-skill` stages an existing release and `just run-cli` runs a development build. `system` reports the version, optional features (QR, OCR) and the inference component.

Quote executable paths; PowerShell needs the call operator `&`. See [shells and quoting](references/workflows.md#shells-quoting-and-git-bash) for Git Bash, which rewrites arguments that start with `/`.

## Addresses

- `/` lists the evidence, `/<alias>` one evidence item's partitions and volumes, and `/<alias>/<volume>/<path>` a file: `/nas/part:0/etc/passwd`. The volume segment is the root entry's ID (`part:0`, `lvm:0:1`, `unsupported:0`); folder and archive evidence have one root entry such as `/<alias>/folder:` or `/<alias>/tar:`.
- `/<alias>/@<node id>` names any node by the `id` a result returned — duplicate names, deleted entries, streams and unallocated space. Copy IDs and paths from results; never build them.
- Every listed entry carries its `path`. NTFS, FAT/exFAT, HFS+ and case-insensitive APFS accept another letter case; symbolic links are followed as a shell follows them, and `stat` describes a final link itself.
- Aliases come from `--as` or the file name and are saved in the case. `evidence ls` lists them.

## Start with a case

Every investigation starts by creating a case, or opening the user's existing one, before adding evidence, so volume indexes, searches, timelines and analysis results persist. Use the smallest evidence set that answers the question.

```text
lovelycarve-cli case new "<new-absolute-case-dir>" --name "<case-name>" --open
lovelycarve-cli -C "<case-dir>" evidence add "<absolute-evidence-path>" --as nas --wait
lovelycarve-cli -C "<case-dir>" volumes
lovelycarve-cli -C "<case-dir>" ls /nas/part:0
lovelycarve-cli -C "<case-dir>" stat /nas/part:0/etc/os-release
lovelycarve-cli -C "<case-dir>" case close
```

`--open` starts the case's backend: every later command naming the case (`-C`, `LOVELYCARVE_CASE`, or a working directory inside it) runs there and reuses its open evidence, unlocked volumes, assembled arrays, scans and mounts. Without a backend each command opens the evidence itself — fine for one-off reads — and work that would end with the process (`unlock`, `raid assemble`, `carve start`, `vss open`, `mount`) is refused with `needsBackend`. `case status` shows the backend, evidence, indexes and running tasks. Close a backend you started when the work is done; leave a user's running backend open.

`evidence add --wait` waits for the volume indexes and reports them. Before relying on `find`, `grep` or an absence, check `volumes` or `index ls`: `ready` covers the volume; `building`, `stale` and `truncated` do not. Record encrypted, unsupported or unindexed volumes as coverage limits. `--image <path>` opens evidence for one command without a case.

## Keep output small without losing evidence

- `--json=name,path,size` keeps only those fields of each list entry; paging and time zone fields stay.
- Directory pages hold `--limit` entries (default 200): follow `nextCursor` with `--cursor` until it is null, or use `--all`. Text follows `nextOffset` with `--offset`; `find`, `search hits` and `results rows` page with `--offset` / `--limit`.
- A page, a bounded `tree` walk or a capped `grep` cannot show that nothing else matches. Prefer `find` (indexed names), `grep` (contents), `text` (decoded text) or a format parser (`sqlite`, `inspect`) over dumping bytes.

## Check the outcome

| Exit | Meaning |
| --- | --- |
| `0` | Success; still read task outcomes and coverage |
| `1` | Failure, including not found, a refused or unconfirmed command and a failed background task |
| `2` | Incomplete result: it is printed, then a warning names the fields that make it incomplete |
| `130` | Interrupted |

- Branch on the error `code`: `notFound`, `alreadyExists`, `usage`, `needsBackend`, `readFaultsDetected`, `indexUnavailable`, `caseBackendExited`, `caseLocked`, …. In PowerShell inspect `$LASTEXITCODE` right after each call; exit 1 or 130 stops a dependent chain.
- `readFaultsDetected`, nonempty `badRegions` or nonzero `syntheticBytes` mean zero-filled source bytes went into a hash or export; such digests do not authenticate the missing bytes. `cat` over them fails (exit 1) after any partial output — discard it, and use `export` to keep the data with its audit.
- Exports never overwrite: an existing destination fails with `alreadyExists` before anything is written. Rerun into new paths.
- Commands that delete or discard data ask first; scripts and batches need `-y`. `--dry-run` shows the parsed request without running it.
- Passwords and keys are never literal arguments: `--password @<file>`, `--password -` (stdin), or `--password` alone to be asked at a terminal.
- Report the evidence, address, examined scope, findings, relevant times, output paths and anything incomplete or unsupported. Give each finding a person will review its `lovelycarve://` link from `case link <address>` (with `--offset` and `--length` for matched bytes), which opens and highlights it in the desktop app; see [links](references/workflows.md#links-for-the-examiner). Missing source bytes, failed indexes and unsupported formats do not establish absence. Format limits are in `docs/formats/` of the [LovelyCarve repository](https://github.com/LovelyTeams/LovelyCarve).

## Choose the workflow

Read only the sections of [workflows.md](references/workflows.md) the task needs. Search [commands.md](references/commands.md) by command or keyword (`hash` / `哈希`, `unlock` / `解锁`, `export` / `导出`) for exact options; `lovelycarve-cli <command> --help` shows defaults and `lovelycarve-cli schema <command>` describes them as JSON.

| Task | Start here |
| --- | --- |
| Cases, evidence, the backend, scripts | [Cases and the backend](references/workflows.md#cases-evidence-and-the-backend), [batch](references/workflows.md#batch-requests), [shell](references/workflows.md#the-shell) |
| Find a file, read text, parse a database | [Find and read files](references/workflows.md#find-and-read-files), [filename globs](references/workflows.md#filename-globs) |
| Search file contents | [Content search](references/workflows.md#content-search) |
| Linux or PVE software and kernel versions | [Linux and PVE versions](references/workflows.md#linux-and-pve-versions) |
| Assemble disks or unlock a volume | [RAID and LVM](references/workflows.md#raid-lvm-and-assembled-volumes), [encrypted volumes](references/workflows.md#encrypted-volumes) |
| Recover deleted files or carve signatures | [File recovery](references/workflows.md#file-recovery) |
| Run analyzers or query events | [Plugin analysis](references/workflows.md#plugin-analysis), [timelines](references/workflows.md#timelines) |
| Hash or save evidence bytes | [Hashes, exports and read faults](references/workflows.md#hashes-exports-and-read-faults) |
| Anything without its own command | [rpc](references/workflows.md#backend-commands-without-their-own-command) |
