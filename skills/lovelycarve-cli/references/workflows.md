# lovelycarve-cli workflows

Read the sections needed for the task. Examples use `exam` as a session name and placeholders for host paths and returned IDs; substitute actual values. Inline JSON command examples use POSIX shell quoting; in PowerShell, save the JSON as a UTF-8 file and pass `@<absolute-file-path>` instead. JSONL blocks within a workflow are stages in one process; different workflows are independent examples. Every `lovelycarve-cli` line and JSONL request below is checked against the command catalogue; exact options are in [commands.md](commands.md).

## Contents

- [Sessions and cases](#sessions-and-cases)
- [Pagination and projection](#pagination-and-projection)
- [Batch with JSONL](#batch-with-jsonl)
- [Find and inspect files](#find-and-inspect-files)
- [Paths and node IDs](#paths-and-node-ids)
- [File times and time zones](#file-times-and-time-zones)
- [Content search and global search](#content-search-and-global-search)
- [RAID, LVM and assembled volumes](#raid-lvm-and-assembled-volumes)
- [BitLocker](#bitlocker)
- [LUKS, VeraCrypt and FileVault](#luks-veracrypt-and-filevault)
- [File recovery](#file-recovery)
- [Plugin analysis](#plugin-analysis)
- [Timelines](#timelines)
- [Hashes, exports and read faults](#hashes-exports-and-read-faults)
- [Background tasks](#background-tasks)
- [Special evidence and mounts](#special-evidence-and-mounts)

## Sessions and cases

- Prefer a named session for separate tool calls; use a JSONL process when the caller can keep stdin open. One-shot commands suit self-contained reads, but lose unlocks, assembled volumes and in-memory recovery scans at exit.
- `--image` takes a loaded image ID or an evidence path opened on first use. Without it, selection comes from `LOVELYCARVE_IMAGE` or the current image. `--case` / `LOVELYCARVE_CASE` opens an existing case; it does not create one. Image and case defaults also come from `.env.local` or `.env` in the working directory. `--session` overrides `LOVELYCARVE_SESSION`.
- `sessions open` on an existing name returns its state; startup `--image` and `--case` apply only to a new session. Use `sessions get`, `context get` and `images list` before reusing one; use `images open` / `context use` and `cases open` to change its context explicitly.
- Use absolute host paths. Relative host paths resolve against `sessions get`'s `workingDirectory`, not the connecting command's directory. Evidence paths remain separate and use `/`.
- Cases persist indexes, searches, timelines and analysis results. Without one, browsing and byte operations work, but indexes last only for the process. Do not open the same case in the desktop app and CLI simultaneously.

For an existing case:

```text
lovelycarve-cli -q --case "<absolute-case-dir>" --image "<absolute-evidence-path>" sessions open exam
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes volumes list
```

To create a case for the requested analysis, start a new session, create the case, then open the evidence. Already loaded evidence is also registered when creating a case:

```text
lovelycarve-cli -q sessions open exam
lovelycarve-cli -q --session exam cases create "<new-absolute-case-dir>" --name "<case-name>"
lovelycarve-cli -q --session exam images open "<absolute-evidence-path>"
lovelycarve-cli -q --session exam tasks wait
```

Inspect each result before continuing. `tasks wait` can report cancelled tasks without failing; verify the required index or analysis actually completed. After finishing the work, close a session you created with `sessions close exam`; that cancels remaining tasks and releases its evidence. Leave a reused user session open unless asked to close it.

## Pagination and projection

- `--fields name,id,kind,size,timestamps.modified` projects list entries, preserving response metadata. Dot paths keep nesting. Include identity and fields needed to assess the result; include `timestampsOriginal` when comparing converted and source times (absent when identical).
- Directory and journal pages use `nextCursor` / `--cursor`. Search hits and analysis records use `--skip` / `--limit`; indexed-file filters carry `offset` / `limit` inside the filter JSON. For indexed files, choose `--volume-key` to page one volume: without it the filter's limit and offset apply independently to every indexed volume, and the results are concatenated. Keep scope, filtering and sorting stable.
- A tree walk's `truncated: true` means its depth/inspection budget needs review; it is not a cursor page. Narrow the subtree or continue in its child directories. If a tool truncates stdout, rerun with smaller pages or save the result to a new host file instead of trying to parse the clipped JSON.
- Prefer native filters (`files list --query`, `indexes files list --filter`) to reduce backend work and output. If `jq` is available, filter a projected page before it reaches context:

```sh
jq -c '.items |= map(select(.kind == "file" and .size > 1048576))'
```

This preserves pagination and time zone metadata. It filters only that page: follow `nextCursor` even when the filtered page is empty. In JSONL, select the response by `id` and then its `.result` first. An intentionally bounded sample must be reported as a sample.

## Batch with JSONL

`lovelycarve-cli -q session` reads one JSON request per stdin line and emits one response per stdout line, `{"id":…, "result":…}` or `{"id":…, "error":…}`; incomplete results add `warning` beside `result`. Give it the required `--image` / `--case` at startup or open them with requests. It owns its own runtime, separate from named sessions; do not copy in-memory IDs between them or concurrently open the same case. Keep stdin open to inspect results and submit the next stage; EOF waits for dispatched requests and background tasks, then exits.

- Use unique nonempty request IDs. `args` uses top-level camelCase keys from `schema`. Optional request keys beside `args` are `fields` (comma-separated string), `timezone`, `output` (new host file), `force` and `dryRun`.
- `{"$ref":"<id>#/<JSON pointer>"}` selects from an earlier successful response's **result**, not its envelope. Pending references wait; unknown or failed IDs and missing pointers fail explicitly. A reference can select an ID but does not check a task's status or a warning.
- `--jobs N` (1–64, default 1) overlaps supported queries. State changes and `tasks wait` are barriers. Responses arrive in completion order; associate them by `id`.
- A failed request does not stop subsequent requests. Submit preparation and discovery first, check errors, warnings and completion, then submit analysis or export. Do not prequeue an export behind `tasks wait` and assume a failed wait will prevent it.
- Raw reads return hex in JSONL, limited to 16 MiB per read. Prefer `files text get` for decoded text, bounded `offset` / `length` for bytes, or top-level `output` to save a large result to a new file. An `output` response contains the host path, so it cannot also supply the original payload's IDs to later references.

```json
{"id":"image","command":"images open","args":{"path":"<absolute-evidence-path>"}}
{"id":"roots","command":"files list","args":{"imageId":{"$ref":"image#/id"},"path":"/","pageSize":50},"fields":"name,id,kind,size"}
```

## Find and inspect files

Choose a metadata search for names, paths, extensions or sizes; use content search for bytes or text. Start within a known subtree rather than recursively dumping every file.

- `files tree list --name` is a bounded name search without a case requirement. For indexed evidence, `indexes files list --filter` searches metadata. Select a volume key from `indexes volumes list` to bound the query; returned `nid` values are node IDs for subsequent file commands.
- `files get` reads metadata; `files content get` reads MIME, magic, entropy and data runs; `files identify` detects the content type. These answer different questions.
- `files text get --max-bytes` decodes detected encodings to UTF-8, up to 2 MiB. A capped read is a preview. For exact bytes use `files read --offset --length`; choose a format-specific parser (`files binary get`, `files sqlite …`, `files zip …`) when structure matters. QR/OCR requires a full build; `system get` reports build capabilities.

```text
lovelycarve-cli --session exam files tree list --path "/<volume>/Users" --depth 4 --name report --limit 200
lovelycarve-cli --session exam indexes volumes list
lovelycarve-cli --session exam indexes files list --volume-key "<returned-volume-key>" --filter '{"nameContains":"report","extensions":["pdf","docx"],"limit":50,"offset":0}'
lovelycarve-cli --session exam files get --node-id "<returned-file-node-id>"
lovelycarve-cli --session exam files text get --node-id "<returned-file-node-id>" --max-bytes 16384
```

The metadata filter matches names and extensions, not document contents. Its `offset` / `limit` belong inside the JSON object.

## Paths and node IDs

- `/` is the evidence's synthetic root: it lists partitions and volumes (folder and archive evidence show their root name first). Other paths are relative to the current directory, accept `.` and `..`, and do not follow symbolic links.
- Volume entry names keep letters, digits, CJK characters, `.`, `-` and `_`; spaces, parentheses, `·` and other separators become `_`, so `ext4 (md · host:0)` is `ext4_md_host_0` and `APFS 0 · Macintosh HD` is `APFS_0_Macintosh_HD`. Real file and directory names inside a volume, raw volume labels and node IDs are unchanged. Names that collide after this conversion need IDs.
- The first segment of an absolute path may be a root node ID from `files list --path /` (`items[].id`, for example `/part:0/Windows/System32/config/SAM`). If a root node's name equals another root node's ID, the path is ambiguous and fails: use an ID option instead.
- Every command that takes a path also takes a node ID (`--node-id`, `--parent-id` or `--root-id`), which also reaches deleted entries and special streams. Copy IDs from results; never build them from names.
- Names match exactly first. NTFS, FAT/exFAT, HFS+ and case-insensitive APFS volumes then accept a unique case-insensitive match; other file systems and the synthetic root match exactly. Synthetic nodes have Chinese names (`分区0`, `$未分配簇`, `未使用空间1`): take them from `files list`.
- `context set --node-id <id>` enters a directory by ID (exclusive with `--dir`), and `context get` shows the current `nodeId`. In `shell` and `sessions attach`, `cd --node-id <id>` does the same.
- `files list` pages hold 50 entries for agents and 500 for people unless `--page-size` (1–5000) says otherwise. `files tree list` walks a subtree: `--depth` (at most 128), `--name` (case-insensitive substring) and `--limit` (default 10000, at most 100000) nodes inspected; its `items[].path` values are absolute, and `truncated: true` means the walk stopped early.
- In a case, evidence whose source is missing (`indexOnly`) can still be browsed from its index, but commands that read bytes fail.

```text
lovelycarve-cli --session exam files list --path / --fields name,id,kind
lovelycarve-cli --session exam context set --node-id "<returned-directory-node-id>"
lovelycarve-cli --session exam context get
lovelycarve-cli --session exam files get "<relative-file-path>"
```

## File times and time zones

- `files list`, `files get` and `files tree list` convert times for display; bytes, hashes, indexes, plugin results and timelines keep the original values.
- The default `auto` policy takes the image setting, then the case setting, then a system time zone found in the evidence (Linux and NAS configuration such as `/etc/localtime`), and finally UTC, which means a fallback rather than evidence of UTC. The host's time zone is never used. `--timezone evidence` or `--timezone case` forces that source and fails when it is unknown.
- `--timezone` takes `auto`, `evidence`, `case`, `UTC`, an IANA name such as `Asia/Shanghai`, or a fixed offset such as `+05:45`. IANA names apply each date's historical offset.
- `timezones get` shows the effective zone, candidates, configuration paths, sources and warnings. `timezones set <zone> --scope session|case|image` stores a default; `case` and `image` persist in the case, and `auto` clears that layer.
- `timestampsOriginal` appears only when the converted value differs. Local times without a known offset (FAT/exFAT, `-00:00`) stay as they are and are listed in `timestampDisplay.unconvertedFields` (`timestampUnconvertedFields` on list entries).
- In JSONL, a top-level `timezone` overrides one request only.

```text
lovelycarve-cli --session exam timezones get
lovelycarve-cli --session exam --timezone Asia/Shanghai files get "/<volume>/report.txt"
lovelycarve-cli --session exam timezones set evidence
```

## Content search and global search

- `files search <pattern> [path]` searches file contents below the current directory, one file when the path is a file, or every indexed volume for `/`. It needs an open case, a built volume index and readable source evidence. After opening evidence in a case, use `tasks wait` and `indexes volumes list` to verify index availability; an absent or failed index does not establish that there are no matches.
- Matching is case-sensitive and includes deleted files unless `--ignore-case` or `--exclude-deleted` is given; `--regex` treats the pattern as a regular expression; `--codepage UTF-8` (repeatable) limits the encodings, which are otherwise all supported encodings.
- The result is `{summary, hits, total, truncated}`; `--limit` (default 10000) caps the returned hits. Persisted hits stay in the case: page them with `searches hits list --search-id <id> --skip <n> --limit <n>`. Inspect `summary` for incomplete scanning too: paging stored hits does not repair unreadable files or a search engine cap.
- `searches create --options <json>` runs a global search over every indexed volume of the evidence and waits for its summary. `schema searches create` describes the request's `options` parameter, not its nested fields; for a simple keyword or regex use `files search` instead of inventing an options object.

```text
lovelycarve-cli --session exam tasks wait
lovelycarve-cli --session exam indexes volumes list
lovelycarve-cli --session exam files search password / --ignore-case --search-id "<unique-search-id>" --limit 50
lovelycarve-cli --session exam searches hits list --search-id "<unique-search-id>" --skip 50 --limit 50
```

## RAID, LVM and assembled volumes

- Open every member disk in one session, then run `raid detect`. It reads LVM and md metadata (and firmware RAID), assembles the arrays and registers every complete linear logical volume that is not an md member as its own image. Run it once per session; later calls reuse the assembled images.
- Select a volume with `context use <assembledImageId>` (from `volumeGroups[].logicalVolumes[].assembledImageId`); it returns to `/`. Volumes that are incomplete or unsupported get no ID, and `diagnostics` explains why.
- Arrays whose recorded members are missing or stale wait for confirmation: `raid detect --degraded-sets <set-id>` assembles the named sets degraded. `raid assemble --params <json>` assembles a given geometry by hand.
- `assembled://` references and assembled image IDs exist only in the session that created them; after it ends, open the members and detect again.

```text
lovelycarve-cli -q sessions open array
lovelycarve-cli -q --session array images open "<member-1-path>"
lovelycarve-cli -q --session array images open "<member-2-path>"
lovelycarve-cli -q --session array raid detect
lovelycarve-cli -q --session array context use "<assembled-image-id>"
lovelycarve-cli -q --session array files list
```

In JSONL, inspect detection first and select the intended logical volume. The index `0` below is illustrative, not a rule for choosing evidence. Then reference its result instead of copying IDs:

```json
{"id":"raid","command":"raid detect"}
```

After checking the selected volume and its diagnostics:

```json
{"id":"use","command":"context use","args":{"imageId":{"$ref":"raid#/volumeGroups/0/logicalVolumes/0/assembledImageId"}}}
{"id":"roots","command":"files list"}
```

## BitLocker

- `bitlocker metadata get <partition-id>` needs no credential: it returns the algorithm, volume GUID, FILETIME, description and protector types, never key material.
- `bitlocker unlock <partition-id>` takes one credential: `--password`, `--recovery-password`, `--startup-key-path` or `--fvek-hex` (plus `--tweak-hex` for Elephant Diffuser). With none, it tries a clear key stored on the volume. A raw FVEK has no authentication tag, so check the decrypted content.
- `bitlocker vmks scan <memory-path> --partition-id <n>` finds volume master keys in a memory image, and `bitlocker vmks unlock <partition-id> --vmk-hex` unlocks with one. TPM and SID protectors are not supported.
- Unlocking attaches the plaintext to the partition, which then browses as usual. Unlock state lives in the process: unlock and analyze in one session.
- `bitlocker export <partition-id> --destination <new-path>` writes the decrypted volume and fails on synthesized source bytes (see `images bad-regions list`).

```text
lovelycarve-cli -q sessions open locked --image "<evidence-path>"
lovelycarve-cli -q --session locked bitlocker metadata get 0
lovelycarve-cli -q --session locked bitlocker unlock 0 --recovery-password
lovelycarve-cli -q --session locked files list --path / --fields name,id,kind
lovelycarve-cli -q --session locked bitlocker export 0 --destination "<new-host-path>"
```

Partition ID `0` is illustrative: select the actual partition from `images get` before unlocking or exporting. `--recovery-password` without a value reads the secret from stdin when stdin is not a terminal, or use `--recovery-password @<file>`.

## LUKS, VeraCrypt and FileVault

- `encrypted-volumes unlock --options <json>` unlocks a region of a loaded image. `options.scheme` is `luks`, `veracrypt` or `filevault`; give `password` or, for LUKS and FileVault, `masterKeyHex` (not both); VeraCrypt also takes `pim` (0–1000) and `hidden`. Choose the region with `partitionId`, or with `offset` and optional `length` relative to the source image; the default is the whole image.
- By default (`target: "newImage"`) the result is a separate read-only image whose `image.path` (`decrypted://…`) `images open` accepts. `target: "sourcePartition"` instead attaches the plaintext to the source partition (`part:N`) and keeps the source image ID; it needs a single complete volume.
- Put the options in a file and pass `--options @unlock.json` so the password stays out of the command line; `--dry-run` redacts `password` and `masterKeyHex`. `.hc`, `.tc` and `.luks` files open directly as evidence.
- Unlocked images are not saved in the case; unlock again in a new process. `tasks cancel <taskId>` stops an unlock, and closing the source image closes its decrypted images.
- FileVault: run `filevault get --options <json>` with the same region selection to list the logical volumes, then pass the chosen `index` as `logicalVolumeIndex`. `password` also accepts the personal recovery key. Unlock each of several logical volumes as its own `newImage`. APFS native encryption is not supported.

```json
{"id":"source","command":"images open","args":{"path":"<container-path>"}}
{"id":"unlock","command":"encrypted-volumes unlock","args":{"imageId":{"$ref":"source#/id"},"options":{"scheme":"veracrypt","password":"<password>","pim":0,"hidden":false}}}
{"id":"plain","command":"images open","args":{"path":{"$ref":"unlock#/image/path"}}}
{"id":"roots","command":"files list","args":{"parentId":""}}
```

```json
{"id":"inventory","command":"filevault get","args":{"options":{}}}
```

Inspect the inventory before submitting the unlock; `0` below must be replaced with the intended logical volume's returned index:

```json
{"id":"unlock","command":"encrypted-volumes unlock","args":{"options":{"scheme":"filevault","password":"<password>","logicalVolumeIndex":0,"target":"newImage"}}}
```

Encrypted iOS backups unlock with `ios-backups unlock --password`, and `ios-backups audit` reports the auditor's findings.

## File recovery

- `carve scans create --options <json>` starts one background scan with two phases: file-system record recovery and signature carving. In `options`, `filesystems` turns on `fat`, `ntfs` and `exfat` probing (all off by default, which means carving only) and `include_existing` also lists the existing files of known partitions; `signatures` (default true) controls carving; `format_ids` selects carving formats (`carve formats list`); `scope` is `whole`, `partition`, `unallocated` or `range` (with `start_sector` and `sector_count`).
- Scans live in memory: create, wait, page and export in one session or JSONL process. `carve scans pause` and `carve scans resume` control a scan, and a paused scan must be resumed or cancelled before `tasks wait`.
- `carve files list --scan-id <id>` returns null until the file-system phase ends; `carve artifacts list --scan-id <id>` pages the carved files.
- `carve scans export <scan-id> --options <json>` writes to `options.destination`: `file_ids` takes readable candidates from the file-system results, and `artifact_ids` null (the default) exports every carved file while `[]` exports none. File-system records keep their directory structure, and a JSONL report records each file's status, source and SHA-256.

```json
{"id":"scan","command":"carve scans create","args":{"options":{"format_ids":["png","pdf"],"scope":"whole"}}}
{"id":"done","command":"tasks wait"}
{"id":"status","command":"carve scans get","args":{"scanId":{"$ref":"scan#/scanId"}}}
{"id":"artifacts","command":"carve artifacts list","args":{"scanId":{"$ref":"scan#/scanId"},"limit":50}}
```

Inspect completion and artifact coverage before exporting. Omitting `artifact_ids` exports all carved artifacts, so use this only when the requested scope includes them all:

```json
{"id":"recover","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"destination":"<new-absolute-host-dir>","task_id":"recover-1"}}}
```

```json
{"id":"scan","command":"carve scans create","args":{"options":{"scope":"whole","filesystems":{"fat":true,"ntfs":true,"exfat":true,"include_existing":false},"signatures":false}}}
{"id":"done","command":"tasks wait"}
{"id":"files","command":"carve files list","args":{"scanId":{"$ref":"scan#/scanId"}}}
```

Select a returned readable candidate ID, not its row number or a filesystem node ID:

```json
{"id":"export","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"artifact_ids":[],"file_ids":["<returned-readable-candidate-id>"],"destination":"<new-absolute-host-dir>","task_id":"recover-files-1"}}}
```

## Plugin analysis

- `plugins systems list` lists the operating systems found on the evidence (IDs such as `linux@part:2`) and the volumes they mount; `plugins functions list` lists each analyzer's scope (system, evidence or files), platforms and options.
- `plugins tasks create --function <analyzer>` runs one: system-scope analyzers take `--system-id`, file-scope analyzers take `--node-id` and `--file-name`, and evidence-scope analyzers take neither. Omitted options use their defaults. Creating a task that already runs for the same analyzer and target returns the existing one.
- Plugin tasks cannot pause: `plugins tasks cancel` stops one and `plugins tasks restart` runs it again; a successful run replaces the previous results.
- Results live in the case. `analysis tree get` lists them by evidence, category, run and record type with each `runId`; `analysis records list` needs `--run-id` and `--record-type` and takes `--keyword`, `--sort` and `--descending`; `analysis runs get <run-id>` shows inputs, options, warnings and zero-filled source bytes. Results from older plugin versions are marked `legacy: true` and read with `--legacy`.

```text
lovelycarve-cli --session exam plugins functions list
lovelycarve-cli --session exam plugins systems list
lovelycarve-cli --session exam plugins tasks create --function "<returned-function-id>" --system-id "<returned-system-id>"
lovelycarve-cli --session exam tasks wait
lovelycarve-cli --session exam analysis tree get
lovelycarve-cli --session exam analysis records list --run-id "<run-id>" --record-type "<returned-record-type>" --skip 0 --limit 50
```

The example uses the function's default options. For custom settings, use its declared options with `--options @<absolute-file-path>`, and the returned record schema for filtering and sorting. For the bundled Linux basic-info analyzer, an options file can contain `{"items":["users","logins"]}`; do not apply it to another analyzer. Inspect the run's warnings before interpreting an empty record page as a finding.

## Timelines

- `timeline build` requires a case and waits for the selected evidence's timeline rebuild. Inspect `timeline get` afterwards: cancellation can leave a partial timeline. It does not run missing plugin analyzers; run requested analyzers first if their records should contribute events.
- `timeline range get` and `timeline types list` establish available time coverage and event types. `timeline events list` pages with `skip` / `limit` and `order`; `timeline events export` writes the same query to a new CSV destination.
- Nested `query` uses snake_case. `start` / `end` are inclusive Unix seconds. `time_zone_hours` is a fixed aggregation offset, separate from file-display `--timezone`; `time_unit` controls bucket granularity. Keep query and ordering stable when paging. Naive timestamps are not projected as known instants; missing timeline events do not prove no activity occurred.

```text
lovelycarve-cli --session exam timeline build
lovelycarve-cli --session exam timeline get
lovelycarve-cli --session exam timeline range get
lovelycarve-cli --session exam timeline types list
```

After checking build status, use a bounded query. This includes deleted-source rows and uses UTC aggregation; add `start` / `end` for the requested interval. In PowerShell, save the query object to a UTF-8 file and pass it with `--query @<absolute-file-path>`:

```sh
lovelycarve-cli --session exam timeline events list --query '{"time_zone_hours":0,"time_unit":"day","from_apps":[],"data_types":[],"pairs":[],"show_delete":true}' --skip 0 --limit 50 --order asc
```

## Hashes, exports and read faults

- `files hash` tolerates source read faults: it still returns digests, with `readFaultsDetected`, `syntheticBytes` and `badRegions` describing the zero-filled source ranges, but exits 2 and warns on stderr (a JSONL response adds `warning`). `images hash`, `images export`, `files export` and `files directories export` do the same. Check the exit status or `readFaultsDetected` before relying on a digest; `truncated: false` does not mean there were no faults.
- `files read` writes raw bytes and fails with exit status 1 on a read fault, after possibly writing partial output: discard it. Use `files export` to keep tolerant data together with its audit.
- `images hash` and `images export` read the whole evidence; `--read-fault-policy strict` fails at the first bad sector instead of zero-filling and reporting. `images export --mode unlockedPartitions` writes unlocked partitions as plaintext in place. `images bad-regions list` lists the synthesized source ranges.
- `files streams list/get/export` also exposes BeFS, GFS2 and OCFS2 inline and external attributes. GFS2 / OCFS2 names retain native namespace prefixes such as `user.`. Copy returned stream names exactly; attribute type codes remain available in the Rust reader, and a stream name is not a host path.
- Exports never overwrite: `files export` and `files streams export` need a new file, `files directories export --mode tree` a new directory and `--mode zip` a new file (`skippedCount` or `truncated` mean the archive is partial). A failed single-file export leaves nothing behind and can be retried.

```text
lovelycarve-cli --session exam files hash "/<volume>/Users/alice/report.docx" --algos sha256
lovelycarve-cli --session exam files export "/<volume>/Users/alice/report.docx" --destination "<new-host-path>"
lovelycarve-cli --session exam files directories export "/<volume>/Users/alice" --mode zip --destination "<new-host-path>.zip"
lovelycarve-cli --session exam images hash --algorithms sha256
lovelycarve-cli --session exam images bad-regions list
```

## Background tasks

- Background tasks are the ones still running when their command returns (`carve scans create`, `indexes create`, plugin tasks) and the ones a case starts when evidence joins it (volume indexes, platform detection, plugin tasks). A one-shot command waits for them before exiting and exits 1 after its output when one of them failed; in a session or JSONL process they keep running.
- `tasks list` shows built-in tasks and plugin tasks (IDs `plugin:<id>`), `tasks get <id>` one task and `tasks cancel <id>` cancels a running task at its next checkpoint. An unknown ID fails with `notFound`, and cancelling an ended task fails too. A one-shot process sees only its own built-in tasks: inspect and cancel background tasks in a session.
- `tasks wait` returns when no task is active, with `{"tasks":[…]}`: the background tasks that ended since the last report, each with its `status`. It fails with `tasksFailed` when one of them failed or ended without reporting how; cancelled tasks are listed but do not fail it. Read scan status, search hits or analysis records with their own commands.
- Task and search IDs (`--task-id`, `--search-id`) are generated when omitted; pass your own to cancel a task by name.

## Special evidence and mounts

- Incomplete split RAW opens strictly by default. `images open <first-segment> --raw-read-mode force` reads the contiguous prefix of segments, in the current process only; digests and exports of that prefix do not show the image is complete.
- QCOW legacy AES images take `--qcow-password` or `--qcow-key-hex`; a successful open does not prove the password, since the format has no key check.
- Optical discs: open the `.cue` for multi-track or multi-file discs; `images get` reports tracks and sessions in `opticalDisc`.
- UBI images and Xiaomi HDR1 firmware list their named volumes under `files list --parent-id part:0`.
- `shadow-copies list` and `shadow-copies open <partition-id> --index <n>` expose NTFS volume shadow copies as read-only images.
- Mounts (`mounts create`, Windows with Dokan) and SMB shares last only as long as the process: create them in a session. Reading physical disks (see `disks list`), mounting and scanning memory need an administrator terminal.

```text
lovelycarve-cli -q sessions open partial
lovelycarve-cli -q --session partial images open "<first-segment-path>" --raw-read-mode force
lovelycarve-cli -q --session partial images get
```

### bcachefs

Use ordinary file commands and enter native subvolume/snapshot directory
entries by their returned node IDs. Equal native inode numbers in different
snapshots retain separate bytes; do not turn those numbers into file IDs or
deduplicate the views. Only single-device volumes are supported. Bounds,
codecs and native-oracle coverage are documented in `docs/formats/BCACHEFS.md`.

### NILFS2

NILFS2 uses the ordinary file commands against its latest committed tree.
Read and hash preserve sparse holes and report missing allocated blocks or log
CRC failures. Recorded modification/change times are retained; access and
creation times remain absent. Dirty/recovery views fail explicitly. The CLI
does not currently select native historical snapshots; the standalone Rust
reader provides that API. See `docs/formats/NILFS2.md` for verified boundaries.
