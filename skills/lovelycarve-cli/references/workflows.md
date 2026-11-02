# lovelycarve-cli workflows

Read the sections needed for the task. Resolve `lovelycarve-cli` to the executable in this skill's `bin/` as described in [SKILL.md](../SKILL.md). Examples use `exam` as a session name and placeholders for host paths and returned IDs; substitute actual values. Inline JSON command examples use POSIX shell quoting; in PowerShell, save the JSON as a UTF-8 file and pass `@<absolute-file-path>` instead. JSONL blocks within a workflow are stages in one process; different workflows are independent examples. Every `lovelycarve-cli` line and JSONL request below is checked against the command catalogue; exact options are in [commands.md](commands.md).

## Contents

- [Sessions and cases](#sessions-and-cases)
- [Shell invocation and argument files](#shell-invocation-and-argument-files)
- [Pagination and projection](#pagination-and-projection)
- [Batch with JSONL](#batch-with-jsonl)
- [Find and inspect files](#find-and-inspect-files)
- [Filename glob search](#filename-glob-search)
- [Paths and node IDs](#paths-and-node-ids)
- [File times and time zones](#file-times-and-time-zones)
- [Content search and global search](#content-search-and-global-search)
- [RAID, LVM and assembled volumes](#raid-lvm-and-assembled-volumes)
- [Linux and PVE versions](#linux-and-pve-versions)
- [BitLocker](#bitlocker)
- [LUKS, VeraCrypt and FileVault](#luks-veracrypt-and-filevault)
  - [LUKS](#luks)
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

Known-path browsing and reads work without a full index. Create a case when persistence or indexed analysis is needed, before loading evidence so automatic indexing is persisted in that case:

```text
lovelycarve-cli -q sessions open exam
lovelycarve-cli -q --session exam cases create "<new-absolute-case-dir>" --name "<case-name>"
lovelycarve-cli -q --session exam images open "<absolute-evidence-path>"
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes list
```

Match the index entry to the selected `imageId` and `volumeKey`. Ready, complete coverage requires `indexed: true`, `stale: false`, `truncated: false`. `tasks wait` alone is insufficient because cancelled tasks need not make it fail. `indexes volumes list` supplies volume keys, not proof that a usable index exists. If a required index is missing or stale, build that volume explicitly, then wait and inspect publication again:

```text
lovelycarve-cli -q --session exam indexes volumes list
lovelycarve-cli -q --session exam indexes create --volume-key "<returned-volume-key>"
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes list
```

Do not search an incomplete index and interpret no matches as absence. Report failed or truncated builds and their coverage; avoid repeatedly rebuilding a volume that fails for the same unsupported layout. Unlock encrypted evidence or assemble arrays before building indexes for the newly available volumes.

When the user selects an existing case, reopen it instead of creating a replacement:

```text
lovelycarve-cli -q --case "<absolute-case-dir>" --image "<absolute-evidence-path>" sessions open exam
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam tasks wait
lovelycarve-cli -q --session exam indexes list
lovelycarve-cli -q --session exam indexes volumes list
```

Inspect each result before continuing. `tasks wait` can report cancelled tasks without failing; verify the required index or analysis actually completed. Reuse evidence already collected when it answers a follow-up; rebuild volatile state only for new reads. After finishing the work, close a session you created with `sessions close exam`; that cancels remaining tasks and releases its evidence. Leave a reused user session open unless asked to close it.

A session whose background process exited (a crash, a killed process, a restarted machine) fails with `sessionExited`: the command that found it has already removed its record, `sessions list` stops listing it and `sessions close` on it succeeds with `"exited": true`. Start it again with `sessions open exam`; unlocked volumes, assembled arrays, recovery scans and the current directory lived in the old process and must be rebuilt. `sessionUnavailable` instead means the process still owns the name but did not answer, for example while it is still opening evidence: check again with `sessions get exam`.

## Shell invocation and argument files

In PowerShell, invoke a quoted executable with `&`. JSON options (`--options`, `--body`, `--query`, `--filter`) accept inline JSON or `@file`; a UTF-8 JSON file avoids native-argument quote loss. Quote the whole `@file` argument when its host path contains spaces:

```powershell
& 'D:/<skill-root>/bin/lovelycarve-cli.exe' --session exam encrypted-volumes unlock --options '@D:/<host-dir>/unlock.json'
```

In Git Bash / MSYS2, a Windows executable receives arguments that start with `/` rewritten as host paths, even when quoted: `/分区1/etc` arrives as `C:/Program Files/Git/分区1/etc`. The CLI recognizes an evidence path rewritten below the MSYS installation and turns it back; one it cannot recognize (below a mounted directory such as `/tmp` or `/c`) fails with an explanation. Other arguments are not turned back, so a search pattern or option value that starts with `/` needs the conversion disabled. Scope the override to the CLI call:

```sh
MSYS2_ARG_CONV_EXCL='*' 'D:/<skill-root>/bin/lovelycarve-cli.exe' --session exam files search /etc/passwd --path '/<volume>'
```

Use forward-slash absolute Windows paths such as `D:/…` for host files passed to the native executable. JSONL request bodies arrive on stdin and are not shell arguments. For a suspected shell problem, inspect the parsed request with `--dry-run` before retrying an expensive operation:

```text
lovelycarve-cli -q files list --path /example-volume --dry-run
```

`--dry-run` is a command option: place it after the command, as above. A positional subject may also be written as its option (`files get --path <path>`), not both. Positional arguments keep their order: `files search <pattern> [path]` takes the pattern first, so name the directory with `--path` when the pattern is given as an option.

Argument files are expanded by the calling CLI before a resident-session request is sent. Thus a relative `@file` is relative to the calling shell, while host paths inside the JSON are resolved by the backend/session. Use absolute paths for both. File expansion is not recursive: `"password":"@secret.txt"` inside JSON is a literal password, not a credential-file reference. Put the actual credential in the private options file and never print it into diagnostics.

## Pagination and projection

- `--fields` is accepted for `list` / `search` output only; `files get`, `files text get` and mutations cannot use it. `--fields name,id,kind,size,timestamps.modified` projects list entries, preserving response metadata. Dot paths keep nesting. Include identity and fields needed to assess the result; include `timestampsOriginal` when comparing converted and source times (absent when identical).
- Directory and journal pages use `nextCursor` / `--cursor`; decoded text uses `nextOffset` / `--offset`. Search hits and analysis records use `--skip` / `--limit`; indexed-file filters carry `offset` / `limit` inside the filter JSON. For indexed files, choose `--volume-key` to page one volume: without it the filter's limit and offset apply independently to every indexed volume, and the results are concatenated. Keep scope, filtering and sorting stable.
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
- `files get <path>` reads metadata; `files content get` reads MIME, magic, entropy and data runs; `files identify` detects the content type. These answer different questions.
- `files text get <path> --max-bytes <n>` detects the encoding and returns decoded text from at most 2 MiB of source bytes per call. While `nextOffset` is not null more of the file follows: continue with `--offset <nextOffset>` until it is null. A character cut at the end of a page is left for the next one, and later pages decode with the encoding detected at the start of the file. Use [text decoding](#text-decoding) if it looks wrong. For exact bytes use `files read --offset --length`; choose a format-specific parser (`files binary get`, `files sqlite …`, `files zip …`) when structure matters. QR/OCR requires a full build; `system get` reports build capabilities.

```text
lovelycarve-cli --session exam files tree list --path "/<volume>/Users" --depth 4 --name report --limit 200
lovelycarve-cli --session exam indexes volumes list
lovelycarve-cli --session exam indexes files list --volume-key "<returned-volume-key>" --filter '{"nameContains":"report","extensions":["pdf","docx"],"limit":50,"offset":0}'
lovelycarve-cli --session exam files get --node-id "<returned-file-node-id>"
lovelycarve-cli --session exam files text get --node-id "<returned-file-node-id>" --max-bytes 16384
lovelycarve-cli --session exam files text get --node-id "<returned-file-node-id>" --max-bytes 16384 --offset 16383
```

The metadata filter matches names and extensions, not document contents. Its `offset` / `limit` belong inside the JSON object. In the last line, replace `16383` with the `nextOffset` the previous page returned.

### Filename glob search

After [case and index preparation](#sessions-and-cases), query metadata directly instead of paging every directory and filtering with `jq`. `indexes files list` accepts `filter.nameGlob`, a case-insensitive pattern over the **whole filename**. It supports `*` (zero or more characters), `?` (one character), `[abc]`, `[a-z]` and `[^abc]` (negated class). `%` and `_` are literal, not SQL wildcards; `[[]` matches a literal `[`. This is SQLite GLOB syntax, not regex or shell expansion: braces and `**` have no special directory meaning. Use `pathPrefix` separately for a directory scope, taking its spelling from returned `fullPath` values. Indexed paths may include an evidence/volume root name (for example `/evidence/var/log/auth.log.1`), so do not assume `/var/log/` or construct a prefix from the volume key.

Start with a filename query to see matching paths, then narrow the directory if needed:

```text
lovelycarve-cli --session exam indexes files list --volume-key "<returned-volume-key>" --filter '{"nameGlob":"auth*.log.*","kinds":["file"],"limit":50,"offset":0}'
```

```text
lovelycarve-cli --session exam indexes files list --volume-key "<returned-volume-key>" --filter '{"nameGlob":"auth*.log.*","pathPrefix":"/<returned-index-root>/var/log/","kinds":["file"],"limit":100,"offset":0}'
```

Replace the sample `pathPrefix` with the actual parent prefix from the returned paths, ending in `/`. This finds rotated names such as `auth.log.1` and `AUTH.log.2` under that directory without reading file contents. Omit `pathPrefix` to search the whole volume. Omit `--volume-key` to query every indexed volume of the current evidence; it does not search other evidence images. Each volume applies its own limit/offset, so select and page volumes individually for predictable coverage.

Glob matching and other conditions are combined **before** `limit` / `offset`. Use `limit` within 1–5000, advance `offset` by the number of matching entries actually returned, and continue until an empty page, keeping scope and filters unchanged. A leading literal prefix can narrow the indexed lookup; a leading `*` can still scan metadata. Do not assume a glob makes every search constant-time. `files tree list --name` remains a bounded substring-walk fallback; `files search` is content search. Use `jq` only to format or further refine already selected results.

### Symbolic links

For filename-only matching, use [filename glob search](#filename-glob-search); it matches indexed names without following links or reading their contents.

Paths follow symbolic links as a shell does. A link in the middle of a path is always followed. A link at the end is followed by the commands that read, hash, export, decode or list, and by `context set`; `files get` and `files ancestors list` report the link itself, with its `linkTarget`. A node ID names exactly that node and never follows a link.

A relative target resolves against the link's directory. An absolute target resolves from the root of the volume that holds the link (its partition, logical volume, APFS volume, btrfs subvolume, container or archive root), never from the CLI's synthetic `/`, and `..` in a target stops at that root. `files get` on a path through links returns the physical `path` it reached. Resolution fails with the reason rather than guessing when a target lies in another volume (a separate `/usr` or `/var`, a btrfs root mounted from another subvolume), names a Windows drive (`C:\…`), is not recorded by the volume adapter, or when more than 40 links are followed: locate such a target in its own volume, and do not substitute a same-named host file or a conventional target.

### Text decoding

`files text get` returns `text`, `encoding`, `fromBom`, `hadErrors`, `offset` and `nextOffset`. A BOM declares an encoding; otherwise the detector guesses from the bytes read, from the start of the file for later pages. `hadErrors: false` means decoding needed no replacement characters, not that the guessed encoding is correct: short legacy-encoded text, including GBK/GB18030, may decode cleanly under the wrong encoding.

If the text is garbled, inspect the original bytes (a bounded `files read`; JSONL returns hex) and the file's context before choosing an encoding; mojibake alone does not identify GB18030. Then decode again with `--encoding <label>`, a WHATWG label such as `gbk`, `gb18030`, `big5`, `shift_jis`, `euc-kr` or `utf-16le`, and pass the same label for every page:

```text
lovelycarve-cli --session exam files text get --node-id "<returned-file-node-id>" --encoding gbk
```

Record the encoding used and whether the whole file or only some pages were decoded. `files read --text` decodes as UTF-8 only; avoid routing raw bytes through text-processing shell pipelines.

## Paths and node IDs

- `/` is the evidence's synthetic root: it lists partitions and volumes (folder and archive evidence show their root name first). Other paths are relative to the current directory, accept `.` and `..`, and follow [symbolic links](#symbolic-links).
- Volume entry names keep letters, digits, CJK characters, `.`, `-` and `_`; spaces, parentheses, `·` and other separators become `_`, so `ext4 (md · host:0)` is `ext4_md_host_0` and `APFS 0 · Macintosh HD` is `APFS_0_Macintosh_HD`. Real file and directory names inside a volume, raw volume labels and node IDs are unchanged. Names that collide after this conversion need IDs.
- The first segment of an absolute path may be a root node ID from `files list --path /` (`items[].id`, for example `/part:0/Windows/System32/config/SAM`). If a root node's name equals another root node's ID, the path is ambiguous and fails: use an ID option instead.
- Every command that resolves an evidence path takes it as `--path` (positionally too where the synopsis shows `<path>`) and a node ID as `--node-id`, which also reaches deleted entries and special streams. `files list`, `files flat list` and `files extensions count` keep their `--parent-id` as well, the Docker commands `--root-id`. `files lookup` differs: its `--path` is relative to `--root-id`. Copy IDs from results; never build them from names.
- A path that does not resolve fails with `notFound`, naming the directory and the missing name; list that directory with `files list --path` to see the real names.
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

- `files search <pattern> [path]` searches file contents below the current directory, one file when the path is a file, or every indexed volume for `/`. It needs an open case, a built volume index and readable source evidence. After opening evidence in a case, use `tasks wait` and `indexes list` to verify index readiness; `indexes volumes list` supplies the volume mapping. An absent or failed index does not establish that there are no matches.
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

- First try ordinary navigation in the source image: a complete single-PV LVM volume can be browsed directly under its partition, without a separate assembly. For a multi-disk assembly, open its member disks in one session, then run top-level `raid detect`. It reads LVM and md metadata (and firmware RAID), assembles the arrays and registers every complete linear logical volume that is not an md member as its own image. Run it once per session; later calls reuse the assembled images.
- For cloned hosts with identical VG/PV UUIDs, use `raid detect --image-ids <loaded-member-id>…` to scope detection to one host. Only loaded source IDs are accepted; an empty selection fails. Scoped synthetic paths are distinct for different source path sets and stable across restarts. Without this option, detection still uses all loaded sources; `--image` alone does not restrict membership.
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

## Linux and PVE versions

Use the system/root volume first; add data disks only if the root volume needs them. Discover the volume name with `files list --path /`, navigate any LVM partition, and copy returned paths or node IDs. After assembly, confirm the selected image and root entry again. A cloned PV UUID is not proof that two host images contain identical files. Verify each requested host separately.

- **PVE application version:** `/var/lib/dpkg/status`, with `Package: pve-manager`, `Status: install ok installed`, and its exact `Version`. `proxmox-ve` is a separate metapackage; neither Debian's `/etc/os-release` nor a kernel filename establishes the pve-manager version.
- **Installed kernel release:** list `/boot` for `vmlinuz-*` and `/lib/modules` (or its resolved `/usr/lib/modules` target); correlate exact release strings with installed `proxmox-kernel-*` / older `pve-kernel-*` packages. Distinguish the kernel release from the package's `Version` and helper/metapackage versions.
- **Boot selection:** inspect `/boot/grub/grub.cfg` and, when needed, saved boot state or EFI entries. A default entry, a symlink or a single installed kernel supports an installed/configured conclusion; it does not prove the kernel running at acquisition. Runtime claims need a timestamped boot record, memory or live collection from the evidence host, not the analyst's `uname`.
- Read only enough to answer the question and resolve conflicts. Follow `nextCursor` before calling a directory listing complete. An empty index list means no published index, not a complete index. Known-path file reads can still succeed; check/build a specific volume index only for operations that require it.
- `/etc/pve` is a runtime pmxcfs mount and can be empty offline; cluster configuration may be in `/var/lib/pve-cluster/config.db` and `/etc/corosync/corosync.conf`. An empty mountpoint is not proof that the cluster had no configuration.
- Binary journal files, compressed logs and kernel images require appropriate parsers. `files text get` finding no string does not establish absence, compression type or lack of support for that format. Do not decode megabytes of binary data to answer an installed-version question.

For example, substitute the discovered volume root:

```text
lovelycarve-cli --session exam files text get --path "/<volume>/var/lib/dpkg/status" --max-bytes 1048576
lovelycarve-cli --session exam files list --path "/<volume>/boot" --page-size 50 --fields name,id,kind,size
lovelycarve-cli --session exam files list --path "/<volume>/lib/modules" --page-size 50 --fields name,id,kind
```

Parse dpkg records by blank-line stanzas, not a fixed number of lines after `Package`; `Version` can occur later and records can cross text pages. Capture and check the exit code of each native invocation before decoding. In PowerShell, after resolving `$cli`, `$session` and `$statusPath`:

```powershell
$chunks = [System.Collections.Generic.List[string]]::new()
$offset = 0
while ($true) {
    $raw = & $cli -q --session $session files text get --path $statusPath --max-bytes 1048576 --offset $offset
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "dpkg read failed or incomplete (exit $code); inspect stderr" }
    $page = $raw | ConvertFrom-Json
    if ($page.hadErrors) { throw 'dpkg text decoding has errors; inspect encoding' }
    $chunks.Add($page.text)
    if ($null -eq $page.nextOffset) { break }
    if ($page.nextOffset -le $offset) { throw 'Text cursor did not advance' }
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

Print only matching records plus the source identity and coverage status. Report installed PVE version, installed kernel release and confirmed boot-time kernel as separate facts; correlated package/module/boot artifacts are not independent runtime observations. If earlier verified evidence already answers a follow-up, cite it and read only the missing evidence.

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

### LUKS

Use `scheme: "luks"` for both LUKS1 and LUKS2. Choose either a password or a recovered **volume master key** (`masterKeyHex`), never both. The latter is not a password-slot/KDF key; the reader verifies it against the LUKS header digest. Do not supply VeraCrypt's `pim` or `hidden` options.

First inspect `images get` and select the encrypted region: `partitionId` is the numeric partition ID, not a node ID or the partition's row number. Alternatively give `offset` and optional `length` in **bytes relative to the source image**; do not combine them with `partitionId`. A standalone LUKS container normally uses the whole image, omitting all three region fields. Opening the source alone does not unlock it.

For named sessions, put an options object such as `{"scheme":"luks","partitionId":0,"password":"<passphrase>","target":"newImage"}` in a private UTF-8 file and pass it to `encrypted-volumes unlock` with `--options @<absolute-json-path>`. Replace `0` with the actual partition ID, or omit it for a standalone container. The credential is literal inside that JSON; `"password":"@file"` would not read another file.

The following JSONL example uses a whole-container source. Supply credentials without echoing them to logs or reports:

```json
{"id":"luksSource","command":"images open","args":{"path":"<absolute-luks-container-path>"}}
{"id":"luksUnlock","command":"encrypted-volumes unlock","args":{"imageId":{"$ref":"luksSource#/id"},"options":{"scheme":"luks","password":"<passphrase>","target":"newImage"}}}
```

Check unlock success, then explicitly select the returned plaintext image. This avoids accidentally hashing or exporting the source ciphertext:

```json
{"id":"luksUse","command":"context use","args":{"imageId":{"$ref":"luksUnlock#/image/id"}}}
{"id":"luksRoots","command":"files list","args":{"path":"/","pageSize":50},"fields":"name,id,kind,size"}
```

Inspect the roots and readable files before exporting. If a full plaintext volume was requested, export that image explicitly, with a new host destination:

```json
{"id":"luksExport","command":"images export","args":{"imageId":{"$ref":"luksUnlock#/image/id"},"destination":"<new-absolute-plaintext-raw-path>","readFaultPolicy":"strict"}}
```

With `target: "sourcePartition"`, the image ID instead stays the source's: browse the returned `volumeKey`. `images hash` and default `images export` still read source ciphertext; only `images export --mode unlockedPartitions` projects attached plaintext into the disk layout, preserving the partition table, other regions and total disk length. It is distinct from exporting a separate plaintext volume.

Unsupported layouts (including detached headers, LUKS2 integrity/reencryption or multiple data segments) and KDF resource limits fail explicitly. Report the reason rather than repeatedly trying a different scheme or concluding the password is wrong. Cancellation can wait for the current KDF. Keep work in the same session; close the derived image to release it or close the source to release all its derived views. See `docs/formats/LUKS.md` in the repository for the supported algorithms and limits.

### VeraCrypt and FileVault examples

These are alternative workflows; do not concatenate their JSONL requests into the same run or reuse the `unlock` request ID within one process.

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
- Exports never overwrite: `files export` and `files streams export` need a new file in an existing directory, `files directories export --mode tree` a new directory and `--mode zip` a new file (`skippedCount` or `truncated` mean the archive is partial). An existing destination fails with `alreadyExists` before anything is written, which is not a failed export of the evidence: rerun into new destinations, a fresh directory per batch run. A failed single-file export leaves nothing behind and can be retried.
- `files export` writes one file; on a directory it fails with `isDirectory`. Export a whole subtree with `files directories export`, which keeps its structure and skips the symbolic links inside it rather than following them.

```text
lovelycarve-cli --session exam files hash "/<volume>/Users/alice/report.docx" --algos sha256
lovelycarve-cli --session exam files export "/<volume>/Users/alice/report.docx" --destination "<new-host-path>"
lovelycarve-cli --session exam files directories export "/<volume>/Users/alice" --mode zip --destination "<new-host-path>.zip"
lovelycarve-cli --session exam images hash --algorithms sha256
lovelycarve-cli --session exam images bad-regions list
```

To export several chosen files, create a new empty host directory and send one `files export` request per file to a JSONL session; give each a destination name that stays unique when files share a name. Each response reports its own result, `warning` or error (`alreadyExists`, `notFound`, `isDirectory`), and a failure does not stop the others:

```json
{"id":"export-1","command":"files export","args":{"nodeId":"<returned-file-node-id-1>","destination":"<new-absolute-host-dir>/1-report.docx"}}
{"id":"export-2","command":"files export","args":{"nodeId":"<returned-file-node-id-2>","destination":"<new-absolute-host-dir>/2-report.docx"}}
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
