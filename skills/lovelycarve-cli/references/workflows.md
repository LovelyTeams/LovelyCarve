# lovelycarve-cli workflows

Read the sections the task needs. `lovelycarve-cli` means the executable resolved in [SKILL.md](../SKILL.md). Examples use the case directory placeholder `"<case-dir>"`, the alias `nas` and placeholders for host paths and returned IDs; substitute actual values. Inline JSON uses POSIX shell quoting; in PowerShell save JSON to a UTF-8 file and pass `@<absolute-file-path>`. Every `lovelycarve-cli` line and batch request below is checked against the command tree; exact options are in [commands.md](commands.md).

## Contents

- [Cases, evidence and the backend](#cases-evidence-and-the-backend)
  - [Links for the examiner](#links-for-the-examiner)
- [Shells, quoting and Git Bash](#shells-quoting-and-git-bash)
- [Pages and projection](#pages-and-projection)
- [Batch requests](#batch-requests)
- [The shell](#the-shell)
- [Find and read files](#find-and-read-files)
  - [Filename globs](#filename-globs)
  - [Symbolic links](#symbolic-links)
  - [Text decoding](#text-decoding)
  - [SQLite and other formats](#sqlite-and-other-formats)
  - [BCP exchange packages](#bcp-exchange-packages)
- [Addresses and node IDs](#addresses-and-node-ids)
- [File times and time zones](#file-times-and-time-zones)
- [Content search](#content-search)
- [RAID, LVM and assembled volumes](#raid-lvm-and-assembled-volumes)
- [Linux and PVE versions](#linux-and-pve-versions)
- [Encrypted volumes](#encrypted-volumes)
- [File recovery](#file-recovery)
- [Plugin analysis](#plugin-analysis)
- [Timelines](#timelines)
- [Hashes, exports and read faults](#hashes-exports-and-read-faults)
- [Background tasks](#background-tasks)
- [Special evidence and mounts](#special-evidence-and-mounts)
- [Backend commands without their own command](#backend-commands-without-their-own-command)

## Cases, evidence and the backend

- A case is the workspace. It keeps the evidence list with each item's alias, volume indexes, searches, timelines, analysis results and time zone settings. Without a case, browsing and byte reads work, but indexes last only as long as the process and `grep` is unavailable.
- Commands find their case from `-C <dir>`, then `LOVELYCARVE_CASE`, then the working directory or one of its parents that holds a case (a note on stderr says which). `case new` and `case rm` take their directory as an argument.
- A case backend (`case open`, or `case new --open`) is one background process per case: every command for that case runs in it, so evidence stays open and unlocked volumes, assembled arrays, carving scans and mounts last between commands. Only one process opens a case at a time; do not open a case in the desktop app while its backend runs.
- Without a backend, each command opens the case and its evidence itself and closes them when it ends. Commands whose result would end with the process (`unlock`, `raid assemble`, `carve start`, `vss open`, `mount`) fail with `needsBackend`: open the backend, or run them in one `shell` or `batch`.
- Use absolute host paths; the backend resolves paths where the command was typed, so relative paths still mean the calling shell's directory.

Start every investigation with a case, add the evidence, and verify the indexes before browsing, reading or analysis, even for a known path. Record unsupported, encrypted or unindexable volumes as coverage limits:

```text
lovelycarve-cli case new "<new-absolute-case-dir>" --name "<case-name>" --open
lovelycarve-cli -C "<case-dir>" evidence add "<absolute-evidence-path>" --as nas --wait
lovelycarve-cli -C "<case-dir>" volumes nas
lovelycarve-cli -C "<case-dir>" index ls
```

`volumes` lists each partition and volume with its file system, encryption state and index state; `index ls` lists the case's indexes. Coverage needs `ready`. `building` means not yet; `stale` or `truncated` means the index does not cover the volume as it is now. If a required index is missing, build it and wait:

```text
lovelycarve-cli -C "<case-dir>" index build /nas/part:2
lovelycarve-cli -C "<case-dir>" tasks wait
lovelycarve-cli -C "<case-dir>" index ls
```

Do not search an incomplete index and read no matches as absence. Report failed or truncated builds; do not rebuild a volume that fails the same way again. Unlock encrypted volumes and assemble arrays first, then build indexes for the volumes that became readable.

To reuse the user's existing case, open it instead of creating another:

```text
lovelycarve-cli case open "<case-dir>"
lovelycarve-cli -C "<case-dir>" case status
lovelycarve-cli -C "<case-dir>" evidence ls
```

`case status` reports the backend (`process.mode` is `Backend` while one serves the case), the evidence with aliases, the indexes and the running tasks. Reuse evidence already collected for follow-ups. When the work is done, `case close "<case-dir>"` stops a backend you started; it waits for running work and cancels the rest. Leave a user's backend running unless asked.

A backend that ended without `case close` (a crash, a killed process, a restart) is reported once with `caseBackendExited` and its record is cleared; later commands run in their own processes until `case open` starts a new backend. Unlocked volumes, assembled arrays, scans and mounts lived in the old process and must be set up again.

### Links for the examiner

`case link` prints a `lovelycarve://` link to the case, an evidence root or a node; clicking it in the desktop app opens the case there, selects the node and, with `--offset` and `--length`, highlights those bytes (a `grep` hit's offset and length within its file):

```text
lovelycarve-cli -C "<case-dir>" case link /nas/part:0/Users/alice/notes.txt --offset 1024 --length 16
lovelycarve-cli -C "<case-dir>" --json case link /nas/@ntfs:part:0:42
```

Put the link next to each finding a person will review. A link names the case by its uid and the node by ID, so it stays valid while the case and its indexes do; it carries no host paths. The desktop app cannot open a case while its backend runs: `case close` it before the examiner follows the links.

## Shells, quoting and Git Bash

In PowerShell, invoke a quoted executable with `&`. JSON options (`rpc --body`, `analyze run --options-json`) take inline JSON or `@file`; a UTF-8 file avoids quoting loss:

```powershell
& 'D:/<skill-root>/bin/lovelycarve-cli.exe' -C 'D:/<case-dir>' rpc fs_size_tree --body '@D:/<host-dir>/size-tree.json'
```

Git Bash / MSYS2 rewrites an argument that starts with `/` into a host path before a Windows program sees it, even when quoted: `/nas/etc` arrives as `C:/Program Files/Git/nas/etc`, and an address holding `:/`, such as `/ev/folder:/a.txt`, arrives as a `;`-separated path list. The CLI turns both forms back when they lie below the MSYS installation; anything else that starts with `/` (a `grep` pattern such as `/etc/passwd`, an address below a mounted directory such as `/c`) needs the rewriting off for that call:

```sh
MSYS2_ARG_CONV_EXCL='*' 'D:/<skill-root>/bin/lovelycarve-cli.exe' -C 'D:/<case-dir>' grep /etc/passwd /nas/part:0
```

Use forward-slash absolute Windows paths (`D:/…`) for host files. Batch requests arrive on stdin, so their argv is never rewritten. To see how a line was parsed without running it, add `--dry-run`:

```text
lovelycarve-cli --dry-run ls /nas/part:0 --limit 20
```

## Pages and projection

- `--json=<fields>` keeps only those fields of each list entry: `--json=name,path,kind,size,timestamps.modified`. Dot paths keep nesting. It applies to list results; on raw content (`cat`) it is refused before anything runs. Include identity (`path`, `id`) and the fields you need; include `timestampsOriginal` when comparing converted and source times.
- `ls` pages hold `--limit` entries (1–5000, default 200). Follow `nextCursor` with `--cursor` until it is null, or read every page with `--all`. Text follows `nextOffset` with `--offset`. `find` pages each volume with `--offset` / `--limit`; `search hits`, `results rows` and `timeline ls` use `--offset` / `--limit`; `sqlite query` returns `nextOffset`. Keep the scope, filters and order unchanged while paging.
- `tree` walks at most `--depth` levels (≤ 128) and inspects at most `--limit` entries; `truncated: true` means the walk stopped early, not that a cursor follows. Narrow the subtree instead.
- Prefer native filters (`ls --ext`, `ls --deleted`, `find --type`, `find --min-size`) over filtering a large page afterwards. A filtered page shows only that page: continue with `nextCursor` even when it filtered down to nothing. Report a deliberately bounded sample as a sample.

```text
lovelycarve-cli -C "<case-dir>" --json=name,path,kind,size ls /nas/part:0/Users --limit 50
lovelycarve-cli -C "<case-dir>" ls /nas/part:0/Users --limit 50 --cursor "<returned-next-cursor>"
```

## Batch requests

`lovelycarve-cli batch` reads one request per stdin line and writes one response per stdout line. A request is the command line itself, `{"id":"<unique id>","argv":[…]}`, so batches use the same commands and options as typed ones. Responses are `{"id":…,"result":…}`, `{"id":…,"error":…}`, or a `result` with a `warning` beside it for incomplete results. With a backend serving the case, every request goes to it; otherwise the batch process holds the case and evidence itself, so unlocks and scans in it last until stdin ends.

- `-C`, `--image`, `-q` and `--events` belong to the `batch` process, not to its requests. `--json=fields`, `--tz`, `-y` and `--dry-run` work per request.
- `{"$ref":"<id>#/<JSON pointer>"}` as an argv element takes a value from an earlier successful result, such as an address or a scan ID. A pending reference waits for its request; a failed or missing one fails the request that uses it.
- `--jobs N` (1–64, default 1) overlaps read-only queries; anything that changes state is a barrier. Responses then arrive as requests finish: match them by `id`. A failed request does not stop the others.
- Raw content comes back as hex, at most 16 MiB per request; read ranges with `--offset` / `--length`, or add `"output":"<new-host-file>"` to a request to save its result to a new file instead.
- Keep stdin open to read results before sending dependent requests; at end of input the batch waits for its background work, then exits with 1 if any request failed, else 2 if any result was incomplete.

```json
{"id":"add","argv":["evidence","add","<absolute-evidence-path>","--as","nas"]}
{"id":"volumes","argv":["--json=path,kind,index","ls","/nas"]}
{"id":"etc","argv":["ls","/nas/part:0/etc","--limit","50"]}
{"id":"release","argv":["text",{"$ref":"etc#/items/0/path"},"--max-bytes","4096"]}
```

## The shell

`lovelycarve-cli shell` reads one command per line with a working directory inside the workspace: `cd`, `pwd`, `help` and `exit` belong to the shell, relative addresses start from the directory, and `ls` / `grep` without a path use it. Like a batch, it keeps unlocks and scans for its whole run and forwards to a running backend. Without a terminal it runs a script, stopping at the first failure:

```sh
printf '%s\n' 'evidence add "<absolute-evidence-path>" --as nas' 'cd /nas/part:0/etc' 'ls' 'text os-release' | lovelycarve-cli shell
```

## Find and read files

Choose a name search for names, paths, extensions or sizes, and a content search for bytes or text. Start in a known subtree rather than listing every file.

- `find <glob>` searches the volume indexes; `tree <dir> --name <text>` is a bounded walk for a substring of names.
- `stat <path>` reads metadata; `stat --content` adds MIME, magic, entropy and data runs; `identify` detects the content type from the bytes. They answer different questions.
- `text <path>` detects the encoding and returns decoded text from at most 2 MiB of source bytes (`--max-bytes`). While `nextOffset` is not null more follows: continue with `--offset <nextOffset>`. For exact bytes use `cat --offset --length`; use a parser (`sqlite`, `inspect`) when structure matters. QR and OCR need a full build (`system` reports them).

```text
lovelycarve-cli -C "<case-dir>" tree /nas/part:0/Users --depth 4 --name report --limit 2000
lovelycarve-cli -C "<case-dir>" find "report*.pdf" --under /nas/part:0/Users --type file --limit 50
lovelycarve-cli -C "<case-dir>" stat "/nas/@<returned-file-node-id>"
lovelycarve-cli -C "<case-dir>" text "/nas/@<returned-file-node-id>" --max-bytes 16384
lovelycarve-cli -C "<case-dir>" text "/nas/@<returned-file-node-id>" --max-bytes 16384 --offset 16383
```

In the last line, use the `nextOffset` the previous page returned.

### Filename globs

`find` matches a case-insensitive glob against the **whole file name** in the volume indexes: `*`, `?`, `[abc]`, `[a-z]` and `[^abc]`. `%` and `_` are literal; `[[]` matches `[`. It is not a regular expression or shell expansion: braces and `**` have no special meaning. Scope a directory with `--under <address>`, and narrow with `--type`, `--deleted` / `--no-deleted`, `--min-size` / `--max-size`.

```text
lovelycarve-cli -C "<case-dir>" find "auth*.log.*" --type file --limit 50
lovelycarve-cli -C "<case-dir>" find "auth*.log.*" --under /nas/part:0/var/log --limit 100 --offset 100
```

Conditions apply before paging. Each volume returns at most `--limit` matches from `--offset`; `truncated` says a volume reached the limit, so continue with the next offset. A volume without an index is listed in `unindexed` and makes the result incomplete (exit 2); with no index at all, `find` fails with `indexUnavailable` — build the indexes in a case first. `find` finds names; `grep` searches contents.

### Symbolic links

Paths follow symbolic links as a shell does: always in the middle of a path, and at its end for the commands that read, hash, export, decode or list. `stat` describes a final link itself, with `linkTarget`; an `@<node id>` address names exactly that node and never follows a link.

A relative target resolves against the link's directory. An absolute target resolves from the root of the volume holding the link (its partition, logical volume, APFS volume, btrfs subvolume, container or archive root), never from the workspace root, and `..` stops at that root. `stat` through links reports the `path` it reached. Resolution fails with the reason — rather than guessing — when a target lies in another volume (a separate `/usr` or `/var`), names a Windows drive (`C:\…`), is not recorded by the volume, loops, or needs more than 40 links: find such a target in its own volume and do not substitute a same-named file.

### Text decoding

`text` returns `text`, `encoding`, `fromBom`, `hadErrors`, `offset` and `nextOffset`. A BOM declares the encoding; otherwise it is guessed from the bytes, from the start of the file for later pages. `hadErrors: false` means no replacement characters were needed, not that the guess is right: short legacy-encoded text, including GBK/GB18030, may decode cleanly under a wrong encoding.

If the text looks wrong, inspect a bounded `cat --offset --length` of the original bytes and the file's context before choosing an encoding, then decode again with `--encoding <label>` (a WHATWG label: `gbk`, `gb18030`, `big5`, `shift_jis`, `euc-kr`, `utf-16le`, …) on every page:

```text
lovelycarve-cli -C "<case-dir>" text "/nas/@<returned-file-node-id>" --encoding gbk
```

Record the encoding used and whether the whole file or only some pages were decoded. `strings --encoding ascii|utf-8|utf-16le|utf-16be` extracts printable runs from binary files instead.

### SQLite and other formats

`sqlite tables` lists tables, views and indexes with row counts; `sqlite rows <db> <table>` pages records; `sqlite query <db> "<SELECT …>"` runs one read-only `SELECT` (or `WITH … SELECT`) with `?1`, `?2`… bound by repeated `--param`; `sqlite deleted` recovers deleted records from free pages, the journal and the WAL. A WAL beside the database is read automatically (`sidecars` lists it). Encrypted databases take `--passphrase @<file>` or `--raw-key @<file>`. Page a query with an `ORDER BY` that is unique, the same parameters and `--offset <nextOffset>`.

```text
lovelycarve-cli -C "<case-dir>" sqlite tables /nas/part:0/data/msgstore.db
lovelycarve-cli -C "<case-dir>" sqlite query /nas/part:0/data/msgstore.db "SELECT id, body FROM messages WHERE chat_id = ?1 ORDER BY id" --param 12 --limit 50
```

`inspect` parses SQLite, Realm, ZIP, CSV, executables (PE, ELF, Mach-O, ar), GGUF, Vim swap files and KeePass KDBX databases; `--as` chooses the parser when the content type is ambiguous. `cat --member <path>` reads one member of a ZIP.

A KDBX database without credentials reports its outer header (version, cipher, key derivation parameters, seeds). `--password`, `--keyfile` (a host file) or `--transformed-key` decrypt it and report groups, entries, history, attachments and metadata, protected values in plaintext; wrong credentials fail with `invalidInput`:

```text
lovelycarve-cli -C "<case-dir>" inspect /nas/part:0/home/alice/Passwords.kdbx
lovelycarve-cli -C "<case-dir>" inspect /nas/part:0/home/alice/Passwords.kdbx --password @pw.txt --keyfile "<absolute-keyfile>"
```

### BCP exchange packages

Add the BCP ZIP, its extracted directory, or a directory holding only that ZIP to a case, import it once, then read it like a plugin run's results:

```text
lovelycarve-cli -C "<case-dir>" evidence add "<absolute-bcp-zip-or-directory>" --as bcp
lovelycarve-cli -C "<case-dir>" results import bcp
lovelycarve-cli -C "<case-dir>" results ls
lovelycarve-cli -C "<case-dir>" results rows "<run-id>" bcp.group_messages --evidence bcp --limit 100
```

Take `runId` and `artifactType` from `results ls`. `--keyword`, `--bcp-app` and `--bcp-account` narrow the records; keep each record's `sources` as provenance. Importing again reuses the published run; `--replace` rereads the package and replaces it.

## Addresses and node IDs

- `/` lists the evidence and `/<alias>` the partitions and volumes of one item, by their root IDs (`part:0`, `lvm:0:1`, `unsupported:0` for a locked or unreadable partition, `folder:` / `tar:` / `zip:` for logical evidence). The root entry's name works in that place too, but IDs are stable and ASCII; names that collide after sanitizing need the ID.
- Below the volume segment, addresses are the real names; `.` and `..` work. A missing name fails with `notFound`, naming the directory and the name: list that directory to see the real names.
- Names match exactly first. NTFS, FAT/exFAT, HFS+ and case-insensitive APFS then accept one case-insensitive match; other file systems match exactly. Synthetic entries have Chinese names (`分区0`, `$未分配簇`, `未使用空间1`): take them from listings.
- `/<alias>/@<node id>` addresses a node by its `id`. It reaches deleted entries, same-named files, streams and unallocated space. `ls -l` shows the IDs. Never build IDs from names.
- In a case, evidence whose source is missing (`indexOnly: true`) is still browsable from its index; commands that read bytes fail.

```text
lovelycarve-cli -C "<case-dir>" ls /nas
lovelycarve-cli -C "<case-dir>" ls -l /nas/part:0/Users --deleted
lovelycarve-cli -C "<case-dir>" stat "/nas/@<returned-node-id>"
```

## File times and time zones

- `ls`, `tree` and `stat` convert times for display; bytes, hashes, indexes, plugin results and timelines keep the original values.
- `auto` takes the evidence's own setting, then the case's, then a zone found in the evidence (`/etc/timezone`, `/etc/localtime`, NAS configuration), and finally UTC, which then is a fallback, not evidence of UTC. The examiner's time zone is never used. `--tz evidence` or `--tz case` forces that source and fails when it is unknown.
- `--tz` takes `auto`, `evidence`, `case`, `UTC`, an IANA name such as `Asia/Shanghai` (with each date's historical offset) or a fixed offset such as `+05:45`, for one command.
- `tz [<address>]` shows the effective zone, its source, candidates and warnings. `tz set <zone>` saves it for the case and `tz set <zone> /<alias>` for one evidence item; `auto` clears that setting.
- `timestampsOriginal` appears where the shown value differs. Local times without a known offset (FAT/exFAT, `-00:00`) stay as they are and are listed in `timestampDisplay.unconvertedFields`.

```text
lovelycarve-cli -C "<case-dir>" tz /nas
lovelycarve-cli -C "<case-dir>" --tz Asia/Shanghai stat /nas/part:0/home/alice/report.txt
lovelycarve-cli -C "<case-dir>" tz set Asia/Shanghai /nas
```

## Content search

- `grep <pattern> [address]` searches file contents: one file, a directory subtree, a volume, an evidence item, or every evidence item when the address is omitted. It needs a case and readable source evidence; the hits are saved in the case.
- Matching is case-sensitive and includes deleted files unless `-i` or `--no-deleted` is given; `-E` makes the pattern a regular expression; `--encoding UTF-8,GBK` limits the encodings, which are otherwise all supported ones.
- The result lists `items` (each with `path`, `offset`, `context`, `searchId`), `total`, `truncated` and `summary`. `--limit` (default 1000) caps the hits returned now; read the rest with `search hits <searchId> --offset <n> --limit <n>`. `summary.readErrors` or `summary.truncated` make the result incomplete (exit 2): paging saved hits does not repair unreadable files.
- `search ls` lists saved searches; `search export <id> -o <new.csv>` writes the hits as CSV; `search rm <id> -y` deletes one.

```text
lovelycarve-cli -C "<case-dir>" grep password /nas/part:0 -i --limit 50
lovelycarve-cli -C "<case-dir>" search hits "<returned-search-id>" --offset 50 --limit 50
```

## RAID, LVM and assembled volumes

- First try ordinary navigation: a complete single-PV LVM volume is browsable under its partition (`/<alias>/lvm:…`) without assembly. For arrays across disks, add every member to the case, then run `raid detect` in the backend. It reads LVM, md and firmware RAID metadata, assembles what is complete and adds each assembled array or logical volume as evidence with its own alias (`assembled[].path`). Later runs reuse what was assembled.
- For cloned hosts with identical VG/PV UUIDs, scope detection to one host with repeated `--member <alias>`. Only source evidence counts as a member; assembled evidence never does.
- Volumes that are incomplete or unsupported are not assembled; read `diagnostics` for why. Arrays with missing or stale members wait for confirmation: `raid detect --degraded <set-uuid>` assembles that set degraded. `raid assemble --members a,b,c --level 5 …` assembles a given geometry by hand, with `-` for a missing column.
- Assembled evidence exists only in the process that assembled it: after the backend ends, open it again and detect again.

```text
lovelycarve-cli case new "<new-absolute-case-dir>" --name "<case-name>" --open
lovelycarve-cli -C "<case-dir>" evidence add "<member-1-path>" --as disk1
lovelycarve-cli -C "<case-dir>" evidence add "<member-2-path>" --as disk2
lovelycarve-cli -C "<case-dir>" raid detect
lovelycarve-cli -C "<case-dir>" ls /
lovelycarve-cli -C "<case-dir>" tasks wait
lovelycarve-cli -C "<case-dir>" index ls
```

In a batch, inspect detection before choosing a volume; take its address from the result instead of copying IDs. The index `0` is illustrative, not a rule for choosing evidence:

```json
{"id":"raid","argv":["raid","detect"]}
{"id":"roots","argv":["ls",{"$ref":"raid#/assembled/0/path"}]}
```

## Linux and PVE versions

Use the system volume first and add data disks only when the system volume needs them. Find the volume with `ls /<alias>`, enter LVM where needed, and copy returned addresses. After assembly, confirm the selected evidence and root entry again. A cloned PV UUID does not prove that two hosts hold identical files: verify each requested host separately.

- **PVE version:** `/var/lib/dpkg/status`, the stanza with `Package: pve-manager`, `Status: install ok installed` and its exact `Version`. `proxmox-ve` is a separate metapackage; neither Debian's `/etc/os-release` nor a kernel file name establishes the pve-manager version.
- **Installed kernels:** list `/boot` for `vmlinuz-*` and `/lib/modules` (or its resolved `/usr/lib/modules`); correlate the exact release strings with installed `proxmox-kernel-*` / older `pve-kernel-*` packages. The kernel release is not the package's `Version`.
- **Boot selection:** read `/boot/grub/grub.cfg` and, where needed, saved boot state or EFI entries. A default entry, a symlink or a single installed kernel supports an installed or configured conclusion; it does not prove the kernel running at acquisition. A running-kernel claim needs a timestamped boot record, memory or live collection from that host, never the examiner's `uname`.
- Read only enough to answer and to resolve conflicts. Follow `nextCursor` before calling a listing complete.
- `/etc/pve` is a pmxcfs mount and is empty offline; cluster configuration may be in `/var/lib/pve-cluster/config.db` (`sqlite tables`) and `/etc/corosync/corosync.conf`. An empty mount point does not prove an empty cluster configuration.
- Binary journals, compressed logs and kernel images need parsers. `text` finding no string does not establish absence, a compression type or a lack of support. Do not decode megabytes of binary data to answer an installed-version question.

```text
lovelycarve-cli -C "<case-dir>" text /nas/part:0/var/lib/dpkg/status --max-bytes 1048576
lovelycarve-cli -C "<case-dir>" --json=name,path,kind,size ls /nas/part:0/boot --limit 50
lovelycarve-cli -C "<case-dir>" --json=name,path,kind ls /nas/part:0/lib/modules --limit 50
```

Parse dpkg records by blank-line stanzas, not a fixed number of lines after `Package`: `Version` can come later and a record can cross text pages. In PowerShell, with `$cli`, `$case` and `$status` set:

```powershell
$chunks = [System.Collections.Generic.List[string]]::new()
$offset = 0
while ($true) {
    $raw = & $cli -C $case --json text $status --max-bytes 1048576 --offset $offset
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "dpkg read failed or incomplete (exit $code); inspect stderr" }
    $page = $raw | ConvertFrom-Json
    if ($page.hadErrors) { throw 'dpkg text decoding has errors; inspect the encoding' }
    $chunks.Add($page.text)
    if ($null -eq $page.nextOffset) { break }
    if ($page.nextOffset -le $offset) { throw 'Text offset did not advance' }
    $offset = $page.nextOffset
}
foreach ($stanza in [regex]::Split(($chunks -join '').Replace("`r`n", "`n"), "`n`n+")) {
    $fields = @{}
    foreach ($line in ($stanza -split "`n")) {
        if ($line -match '^([A-Za-z0-9-]+):[ \t]*(.*)$') { $fields[$matches[1]] = $matches[2] }
    }
    if ($fields.Status -eq 'install ok installed' -and
        $fields.Package -match '^(pve-manager|proxmox-ve|(?:proxmox|pve)-kernel-.+)$') {
        [pscustomobject]@{ Package = $fields.Package; Version = $fields.Version; Status = $fields.Status }
    }
}
```

Report the installed PVE version, the installed kernel release and a confirmed running kernel as separate facts; package, module and boot artifacts from one installation are not independent observations. When earlier verified evidence answers a follow-up, cite it and read only what is missing.

## Encrypted volumes

`encryption <address>` reports a partition's or container's scheme and state without credentials: BitLocker metadata (method, volume GUID, protector types), FileVault logical volumes, LUKS. `unlock <address>` opens BitLocker, LUKS, VeraCrypt and FileVault the same way, with a credential given as `@<file>`, `-` (stdin) or a terminal prompt; the scheme comes from the partition type, except VeraCrypt (no signature) and some iOS backups, which take `--scheme`. Unlocking lasts as long as the process: do it in the backend, a shell or a batch, and unlock again after a restart. Unlock before building indexes for the volumes it reveals.

- A locked partition is listed as `unsupported:N`; `part:N` names it too. `volumes` shows each encrypted volume's scheme and whether it is locked.
- By default the plaintext attaches to the source evidence and the partition then browses as usual (`path` in the result names the readable volume); `--new-image --as <alias>` makes it separate evidence instead. `--offset` and `--length` (bytes in the source) select a region that is not a partition; that always makes new evidence.
- With the plaintext attached in place, `evidence hash` and a plain `evidence export` still read the ciphertext; `evidence export <alias> --unlocked` writes every unlocked partition in plaintext into the disk layout, keeping the partition table, other regions and the total length.
- Unsupported layouts and KDF resource limits fail explicitly: report the reason rather than retrying other schemes or concluding the password is wrong. Cancellation may wait for a running KDF.

### BitLocker

- One credential: `--password`, `--recovery-password`, `--startup-key <.BEK file>`, `--fvek` (with `--tweak` for the Elephant Diffuser) or `--vmk`; with none, a clear key stored on the volume is tried. `--vmk-from <memory image>` finds VMKs in memory and tries each; the result names the one that worked (`vmkOffset`) and the recovery password it yields (`recoveryPassword`). A raw FVEK has no authentication: check the decrypted content. TPM and SID protectors are not supported.

```text
lovelycarve-cli -C "<case-dir>" encryption /nas/part:2
lovelycarve-cli -C "<case-dir>" unlock /nas/part:2 --recovery-password "@<absolute-recovery-password-file>"
lovelycarve-cli -C "<case-dir>" ls /nas/part:2
lovelycarve-cli -C "<case-dir>" evidence export nas -o "<new-host-path>" --unlocked
```

Partition `part:2` is illustrative: take the encrypted partition from `volumes`.

### LUKS

LUKS1 and LUKS2 unlock with `--password` or a recovered volume master key (`--master-key`, verified against the header digest; it is not a key-slot passphrase). A whole-image LUKS container shows as `unsupported:0`. Detached headers, LUKS2 integrity or reencryption and several data segments are not supported. See `docs/formats/LUKS.md`.

```text
lovelycarve-cli -C "<case-dir>" unlock /lk/unsupported:0 --password "@<absolute-password-file>"
lovelycarve-cli -C "<case-dir>" ls /lk/part:0
```
### VeraCrypt, FileVault and iOS backups

VeraCrypt (and TrueCrypt) has no signature: give `--scheme veracrypt`, with `--pim`, `--keyfile <file or folder>` (repeatable; keyfiles alone need no `--password`) and `--prf` where they apply. The default `--header auto` tries the normal and the hidden volume header, like VeraCrypt; `--header backup` reads the embedded backup headers when the primary ones are damaged; `--header system` reads a system encryption header from sector 62 of the drive: address the encrypted system partition to read it in place, or the whole evidence for a plaintext view of the drive as new evidence. The result's `veracrypt` field names the header, PRF, iterations and cipher that opened, and `encryptionComplete: false` marks paused system encryption whose rest reads as stored. A VeraCrypt volume is a whole number of the evidence's sectors, so a region of any other size is refused (`invalidInput`); for a container inside a larger file, give its span with `--offset` and `--length`. See `docs/formats/VERACRYPT.md`. FileVault: read the logical volumes with `encryption`, then unlock one by its `--logical-volume` index (`--password` also accepts the personal recovery key); with several logical volumes, make each new evidence with `--new-image`. APFS native encryption is not supported. Encrypted iOS backups unlock with `--password` on the evidence root.

```text
lovelycarve-cli -C "<case-dir>" unlock /vc --scheme veracrypt --password "@<absolute-password-file>" --pim 0
lovelycarve-cli -C "<case-dir>" unlock /vc --scheme veracrypt --keyfile "<absolute-keyfile>" --header hidden
lovelycarve-cli -C "<case-dir>" unlock /disk/part:2 --scheme veracrypt --header system --password "@<absolute-password-file>"
lovelycarve-cli -C "<case-dir>" encryption /mac/part:1
lovelycarve-cli -C "<case-dir>" unlock /mac/part:1 --scheme filevault --password "@<absolute-password-file>" --logical-volume 0 --new-image --as mac-hd
lovelycarve-cli -C "<case-dir>" unlock /phone --scheme ios --password "@<absolute-password-file>"
```

## File recovery

- `carve start [alias]` starts one background scan with two phases: file system record recovery (`--fs fat,ntfs,exfat`; `--existing` also lists the existing files of known partitions) and signature carving (`--types png,pdf`, see `carve formats`; `--no-signatures` skips it). `--scope whole|unallocated|partition|range` with `--partition`, `--start-sector`, `--sectors` sets the range.
- Scans live in the process: start, wait, read and export them in the backend, a shell or a batch. `carve pause` and `carve resume` control one; a paused scan must be resumed or cancelled before `tasks wait` returns.
- `carve status <scan>` shows phase, progress and counts. `carve results <scan>` pages carved files; `carve results <scan> --fs` lists recovered file system records once that phase has ended. `carve cat <scan> <artifact>` prints one carved file.
- `carve export <scan> -o <new dir>` writes every carved file by default; `--artifacts id,…` picks some and `--no-artifacts` none, `--files <candidate>,…` adds recovered records by their candidate ID. Records keep their folders under one folder per volume, and a JSONL report records each file's status, source and SHA-256.

```text
lovelycarve-cli -C "<case-dir>" carve start nas --types png,pdf --scope unallocated
lovelycarve-cli -C "<case-dir>" tasks wait
lovelycarve-cli -C "<case-dir>" carve status "<returned-scan-id>"
lovelycarve-cli -C "<case-dir>" carve results "<returned-scan-id>" --limit 50
lovelycarve-cli -C "<case-dir>" carve export "<returned-scan-id>" -o "<new-absolute-host-dir>"
```

Check completion and coverage before exporting. For deleted file system records, pick readable candidates by their returned ID, not a row number or a file node ID:

```json
{"id":"scan","argv":["carve","start","--fs","fat,ntfs,exfat","--no-signatures"]}
{"id":"done","argv":["tasks","wait"]}
{"id":"files","argv":["carve","results",{"$ref":"scan#/scanId"},"--fs"]}
{"id":"export","argv":["carve","export",{"$ref":"scan#/scanId"},"-o","<new-absolute-host-dir>","--no-artifacts","--files",{"$ref":"files#/candidates/0/id"}]}
```

## Plugin analysis

- `analyze ls` lists the analyzers (scope, platforms, options) and the systems found on the evidence (IDs such as `linux@part:2`).
- `analyze run <analyzer>` creates a plugin task: system analyzers take `--system <id>`, file analyzers `--file <address>`, evidence analyzers neither. Options are `--option key=value` (JSON values keep their type) or `--options-json @<file>`; omitted options take their defaults. A task already running for the same analyzer and target is returned instead of a new one.
- Plugin tasks cannot pause: `tasks cancel plugin:<n>` stops one and `tasks restart plugin:<n>` runs it again; a successful run replaces the previous results.
- Results live in the case: `results ls` lists them by evidence, category, run and record type with each `runId`; `results rows <run> <type>` pages records (`--keyword`, `--sort`, `--desc`); `results show <record>` shows one; `results run <run>` shows inputs, options, warnings and zero-filled source bytes. Results of older plugin versions are marked `legacy: true` and read with `--legacy`.

```text
lovelycarve-cli -C "<case-dir>" analyze ls --evidence nas
lovelycarve-cli -C "<case-dir>" analyze run "<returned-analyzer>" --system "<returned-system-id>" --evidence nas
lovelycarve-cli -C "<case-dir>" tasks wait
lovelycarve-cli -C "<case-dir>" results ls
lovelycarve-cli -C "<case-dir>" results rows "<run-id>" "<returned-record-type>" --evidence nas --limit 50
```

For the bundled Linux basic-info analyzer, `--option 'items=["users","logins"]'` limits what it collects; use each analyzer's own declared options. Read a run's warnings before treating an empty page as a finding.

## Timelines

- `timeline build` needs a case and rebuilds the evidence's timeline; `timeline status` then shows build state, time range and event types. It does not run analyzers: run the ones whose records should contribute first.
- `timeline ls` pages events (`--from` / `--to` as Unix seconds, RFC 3339 or a date; `--app`, `--type`, `--text`, `--deleted`, `--desc`); `timeline stats --by day` counts them per bucket, with `--utc-offset` setting the bucket zone (separate from `--tz`); `timeline export -o <new.csv>` writes the same query.
- Times without a known offset are not projected as instants, and missing events do not prove that nothing happened.

```text
lovelycarve-cli -C "<case-dir>" timeline build --evidence nas
lovelycarve-cli -C "<case-dir>" timeline status --evidence nas
lovelycarve-cli -C "<case-dir>" timeline ls --evidence nas --from 2024-06-01 --to 2024-06-30 --limit 50
lovelycarve-cli -C "<case-dir>" timeline stats --evidence nas --by day --utc-offset 8
```

## Hashes, exports and read faults

- `hash` tolerates source read faults: it still returns digests, with `readFaultsDetected`, `syntheticBytes` and `badRegions` describing the zero-filled source ranges, but exits 2 with a warning. `evidence hash`, `evidence export` and `export` do the same; `--strict` makes the evidence commands fail at the first bad sector instead. `evidence bad-regions` lists the zero-filled ranges. Check the exit status before relying on a digest.
- `cat` writes raw bytes and fails (exit 1) on a read fault, possibly after partial output: discard it, and use `export` to keep the data with its audit.
- `export <address> -o <new path>` writes a file, or a directory as a tree (`--zip` for a ZIP); symbolic links inside a directory are kept as links, not followed. Several addresses export into one new directory, each under its own name. `--stream <name>` exports a named stream, resource fork or extended attribute (`streams` lists them; GFS2 / OCFS2 names keep prefixes such as `user.`).
- Exports never overwrite: an existing destination fails with `alreadyExists` before anything is written, which is not a failed read of the evidence. Rerun into new paths.

```text
lovelycarve-cli -C "<case-dir>" hash /nas/part:0/Users/alice/report.docx --algo sha256
lovelycarve-cli -C "<case-dir>" export /nas/part:0/Users/alice/report.docx -o "<new-host-path>"
lovelycarve-cli -C "<case-dir>" export /nas/part:0/Users/alice --zip -o "<new-host-path>.zip"
lovelycarve-cli -C "<case-dir>" evidence hash nas --algo sha256
lovelycarve-cli -C "<case-dir>" evidence bad-regions nas
```

## Background tasks

- Background tasks keep running after their command returns (`carve start`, `index build`, plugin tasks), and adding evidence to a case starts some (volume indexes, platform detection, analyzers). A one-shot command waits for its process's tasks before exiting and fails after its output if one failed; in the backend they keep running.
- `tasks ls` lists built-in and plugin tasks (`plugin:<n>`), `tasks show <id>` one, `tasks cancel <id>` cancels a running one at its next checkpoint. An unknown ID fails with `notFound`; cancelling an ended task fails too. Without a backend a process sees only its own tasks.
- `tasks wait` returns when no task is active, with the tasks that ended since the last report and their `status`. It fails with `tasksFailed` when one failed; cancelled tasks are listed but do not fail it. Read scan status, search hits and results with their own commands.

## Special evidence and mounts

- An incomplete split RAW opens strictly. `evidence add <first segment> --raw-read-mode force` reads the contiguous prefix instead, in this process only; digests and exports of that prefix do not show that the image is complete.
- QCOW legacy AES images take `--qcow-password @<file>` or `--qcow-key @<file>`; opening does not prove the password, as the format has no key check.
- Optical discs: add the `.cue` for multi-track or multi-file discs; `evidence show` reports tracks and sessions in `opticalDisc`.
- UBI images and Xiaomi HDR1 firmware list their named volumes under `ls /<alias>/part:0`.
- `vss ls /<alias>/part:N` lists NTFS shadow copies; `vss open /<alias>/part:N <index> --as <alias>` opens one as read-only evidence.
- `mount <address>` (Windows, Dokan) mounts a volume or directory read-only, optionally shared over SMB with `--share`; mounts last as long as the backend. Reading physical disks (`disks`), mounting and memory scans need an administrator terminal.

```text
lovelycarve-cli -C "<case-dir>" evidence add "<first-segment-path>" --as partial --raw-read-mode force
lovelycarve-cli -C "<case-dir>" evidence show partial
```

### bcachefs

Use the ordinary file commands and enter native subvolume and snapshot directories by their returned addresses. Equal inode numbers in different snapshots keep separate bytes: do not turn them into IDs or deduplicate the views. Only single-device volumes are supported; see `docs/formats/BCACHEFS.md`.

### NILFS2

NILFS2 reads its latest committed tree with the ordinary file commands. Reads and hashes keep sparse holes and report missing blocks or log CRC failures. Modification and change times are kept; access and creation times stay absent. Dirty or recovering views fail explicitly. The CLI does not select historical checkpoints; see `docs/formats/NILFS2.md`.

## Backend commands without their own command

`rpc <operation> --body <json>` calls any backend operation by its IPC name with the desktop's parameters, and `rpc --list` lists them. `imageId` may be an evidence alias. Use it only where no command covers the task, such as a size tree, the iOS backup audit or locating a plugin-relative path:

```text
lovelycarve-cli -C "<case-dir>" rpc --list
lovelycarve-cli -C "<case-dir>" rpc ios_backup_audit --body '{"imageId":"phone"}'
lovelycarve-cli -C "<case-dir>" rpc fs_source_locate --body '{"imageId":"nas","rootId":"root","path":"data/system/packages.xml"}'
```
