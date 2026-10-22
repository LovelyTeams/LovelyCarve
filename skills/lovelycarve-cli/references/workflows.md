# lovelycarve-cli workflows

Multi-step tasks, with the facts that decide how to run them. Every `lovelycarve-cli` line and JSONL request below is checked against the command catalogue; exact options are in [commands.md](commands.md).

## Contents

- [Paths and node IDs](#paths-and-node-ids)
- [File times and time zones](#file-times-and-time-zones)
- [Content search and global search](#content-search-and-global-search)
- [RAID, LVM and assembled volumes](#raid-lvm-and-assembled-volumes)
- [BitLocker](#bitlocker)
- [LUKS, VeraCrypt and FileVault](#luks-veracrypt-and-filevault)
- [File recovery](#file-recovery)
- [Plugin analysis](#plugin-analysis)
- [Hashes, exports and read faults](#hashes-exports-and-read-faults)
- [Background tasks](#background-tasks)
- [Special evidence and mounts](#special-evidence-and-mounts)

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
lovelycarve-cli --session exam context set --node-id part:2
lovelycarve-cli --session exam files get Windows
lovelycarve-cli --session exam files tree list --path /part:0/Users --depth 3 --name report
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

- `files search <pattern> [path]` searches file contents below the current directory, one file when the path is a file, or every indexed volume for `/`. It needs an open case, a built volume index and readable source evidence.
- Matching is case-sensitive and includes deleted files unless `--ignore-case` or `--exclude-deleted` is given; `--regex` treats the pattern as a regular expression; `--codepage UTF-8` (repeatable) limits the encodings, which are otherwise all supported encodings.
- The result is `{summary, hits, total, truncated}`; `--limit` (default 10000) only caps the returned hits. All hits stay in the case: page them with `searches hits list --search-id <id>`.
- `searches create --options <json>` runs a global search over every indexed volume of the evidence; `lovelycarve-cli schema searches create` shows the options.

```text
lovelycarve-cli --case "<case-dir>" --image "<evidence-path>" files search password / --ignore-case
lovelycarve-cli --case "<case-dir>" searches hits list --search-id "<search-id>" --limit 100
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

In JSONL, reference the detection result instead of copying IDs:

```json
{"id":"raid","command":"raid detect"}
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

`--recovery-password` without a value reads the secret from stdin when stdin is not a terminal, or use `--recovery-password @<file>`.

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
{"id":"recover","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"destination":"<new-host-dir>","task_id":"recover-1"}}}
```

```json
{"id":"scan","command":"carve scans create","args":{"options":{"scope":"whole","filesystems":{"fat":true,"ntfs":true,"exfat":true,"include_existing":false},"signatures":false}}}
{"id":"done","command":"tasks wait"}
{"id":"files","command":"carve files list","args":{"scanId":{"$ref":"scan#/scanId"}}}
{"id":"export","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"artifact_ids":[],"file_ids":["0"],"destination":"<new-host-dir>","task_id":"recover-files-1"}}}
```

## Plugin analysis

- `plugins systems list` lists the operating systems found on the evidence (IDs such as `linux@part:2`) and the volumes they mount; `plugins functions list` lists each analyzer's scope (system, evidence or files), platforms and options.
- `plugins tasks create --function <analyzer>` runs one: system-scope analyzers take `--system-id`, file-scope analyzers take `--node-id` and `--file-name`, and evidence-scope analyzers take neither. Omitted options use their defaults. Creating a task that already runs for the same analyzer and target returns the existing one.
- Plugin tasks cannot pause: `plugins tasks cancel` stops one and `plugins tasks restart` runs it again; a successful run replaces the previous results.
- Results live in the case. `analysis tree get` lists them by evidence, category, run and record type with each `runId`; `analysis records list` needs `--run-id` and `--record-type` and takes `--keyword`, `--sort` and `--descending`; `analysis runs get <run-id>` shows inputs, options, warnings and zero-filled source bytes. Results from older plugin versions are marked `legacy: true` and read with `--legacy`.

```text
lovelycarve-cli --session exam plugins systems list
lovelycarve-cli --session exam plugins tasks create --function org.lovelycarve.linuxinfo/linuxbaseinfo --system-id linux@part:2 --options '{"items":["users","logins"]}'
lovelycarve-cli --session exam tasks wait
lovelycarve-cli --session exam analysis tree get
lovelycarve-cli --session exam analysis records list --run-id "<run-id>" --record-type linux.user --sort uid --limit 50
```

## Hashes, exports and read faults

- `files hash` tolerates source read faults: it still returns digests and exits 0, with `readFaultsDetected`, `syntheticBytes` and `badRegions` describing the zero-filled source ranges. Check `readFaultsDetected` before relying on a digest; `truncated: false` does not mean there were no faults.
- `files read` writes raw bytes and fails with exit status 1 on a read fault, after possibly writing partial output: discard it. Use `files export` to keep tolerant data together with its audit.
- `images hash` and `images export` read the whole evidence; `--read-fault-policy strict` fails at the first bad sector instead of zero-filling and reporting. `images export --mode unlockedPartitions` writes unlocked partitions as plaintext in place. `images bad-regions list` lists the synthesized source ranges.
- Exports never overwrite: `files export` and `files streams export` need a new file, `files directories export --mode tree` a new directory and `--mode zip` a new file (`skippedCount` or `truncated` mean the archive is partial). A failed single-file export leaves nothing behind and can be retried.

```text
lovelycarve-cli --session exam files hash "/<volume>/Users/alice/report.docx" --algos sha256
lovelycarve-cli --session exam files export "/<volume>/Users/alice/report.docx" --destination "<new-host-path>"
lovelycarve-cli --session exam files directories export "/<volume>/Users/alice" --mode zip --destination "<new-host-path>.zip"
lovelycarve-cli --session exam images hash --algorithms sha256
lovelycarve-cli --session exam images bad-regions list
```

## Background tasks

- A one-shot command waits for the background tasks it started before exiting; in a session or JSONL process they keep running.
- `tasks list` shows built-in tasks and plugin tasks (IDs `plugin:<id>`), `tasks get <id>` one task and `tasks cancel <id>` cancels it at its next checkpoint.
- `tasks wait` returns when no task is active. It does not report success: read the matching status or result command afterwards.
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
