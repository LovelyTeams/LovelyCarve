# LovelyCarve Plugin Protocol (LPP) v1

LovelyCarve plugins are native libraries (`.dll` / `.so` / `.dylib`) that
parse filesystems and analyze files or whole evidence items. A plugin exports
one C ABI entry point; every call carries an LPP v1 method name and a
MessagePack payload. Plugins receive opaque, read-only logical handles and
never receive the evidence path. A logical source may represent a raw image,
partition slice, decrypted view, RAID volume, or another host-managed source;
a logical file may come from any built-in or plugin-backed filesystem.

The same library runs in one of two isolation modes, chosen in its manifest:

| | `isolation = "process"` (default) | `isolation = "in_process"` |
|---|---|---|
| Where the library runs | `lc-plugin-host`, a small loader process next to LovelyCarve | inside LovelyCarve |
| Transport | LPP frames over the loader's stdin/stdout | direct C calls |
| A crash in the plugin | marks the plugin `crashed`; the next use starts a new loader | ends LovelyCarve |
| Task cancel / pause / timeout | kills the loader process | asks the call to stop (`cancel`); a plugin that never checks keeps running |
| Memory limit | Windows Job Object, 512 MiB | none |
| Restart | new process, new instance | new instance; the library stays loaded |

Keep the default for anything that parses untrusted evidence. `in_process`
is meant for trusted, performance-sensitive plugins such as filesystem
parsers that issue many small reads.

## Discovery, manifest, and trust

For the complete Chinese field reference, defaults, accepted values, and current
validation limits, see [plugin.toml 配置参考](PLUGIN_MANIFEST.md). The manual background rescan is described in [插件发现流程](PLUGIN_DISCOVERY.md).

LovelyCarve discovers immediate plugin directories under the per-user app-data
`plugins` directory and the `plugins` directory next to the executable (where
the installer puts the bundled plugins). Development builds additionally
inspect `./plugins` and the repository's `plugins/`; an extra root may be
provided through `LOVELYCARVE_PLUGIN_DIR`. During a running session, the settings
page inspects these roots only when the user clicks 检查插件目录; there is no
periodic poll or file watcher. Each directory must contain a
`plugin.toml` and the library it names:

```toml
id = "org.example.parser"
name = "Example Parser"
version = "0.1.0"
protocol_min = 1
protocol_max = 1
permissions = ["file_materialize", "emit_artifacts"]

[library]
path = "example"          # example.dll / libexample.so / libexample.dylib
isolation = "process"     # default; "in_process" loads it into LovelyCarve

# Optional task-scheduling metadata (see "Task scheduling lifecycle"). Every
# field has a default: manual trigger, not internal, single instance, no pause.
[task]
dependence = "org.example.first/stage.one"   # upstream function; omit for a chain root
trigger = "manually"                         # "auto" roots are created when evidence comes online
internal = false                             # true hides the task from the default task list
single_instance = true                       # one active task per (function, evidence)
can_pause = false                            # allow /task/pause; resume re-runs from scratch
type = "fileanalyzer"                        # display category; current default for both analyzer kinds
platform = "linux"                           # target platform; omit for "all"
support = ["系统信息", "用户列表"]             # sub-items the function covers
evidence_types = ["disk"]                    # supported evidence types; omit for all

[[capabilities]]
type = "filesystem"
filesystem = "vendorfs"
priority = 100

[[capabilities]]
type = "file_analyzer"
analyzer = "vendor.record"
extensions = ["dat"]
mime_types = ["application/octet-stream"]
priority = 10

[[capabilities]]
type = "evidence_analyzer"
analyzer = "vendor.baseinfo"
display = "厂商基本信息"                      # function display name; defaults to "<plugin> · <analyzer>"
```

`library.path` is relative to the plugin directory and may not leave it. A
path without a `.dll` / `.so` / `.dylib` extension is a library name and gets
the platform prefix and suffix (`linuxinfo` → `linuxinfo.dll` on Windows,
`liblinuxinfo.so` on Linux, `liblinuxinfo.dylib` on macOS); a path with one
is used as written. On Windows the library's own directory is searched first
for its dependent DLLs. Manifests with the former `[process]` section are
rejected with a message that points here.

Discovered plugins are trusted on discovery — every plugin with a valid
manifest starts in the `configured` state and can be probed, dispatched, and
analyzed with immediately. There is no approval step and no disable switch; a
SHA-256 fingerprint of the complete directory is computed for display only
(settings page, `PluginDefinition`) and deliberately does not gate execution.
Symlinks and special filesystem entries are rejected, and fingerprinting is
capped at 10,000 files and 512 MiB. A loaded library is never unloaded (an
abandoned call may still run inside it), so replacing a plugin library takes
effect on the next application start. Restarting a plugin ends its instance —
the `lc-plugin-host` process, or the in-process instance once its calls have
returned — closes sessions, unregisters logical sources, and removes
file/materialization leases.

Permissions in v1 are:

- `file_materialize`: permits a logical file to be copied into a host-owned
  temporary lease when a native library strictly requires a path.
- `emit_artifacts`: permits the plugin to return artifacts, timeline events, or
  findings. Summary-only analysis does not need this permission.
- `network`: makes network intent visible in the settings page and, for the
  process mode, to the loader (`LOVELYCARVE_NETWORK_PERMISSION`). It is a
  declaration/consent boundary in v1, not an OS firewall; installations
  requiring network denial must add an operating-system sandbox.

## C ABI v1

The library exports `lovelycarve_plugin_v1`. The host calls it with its
function table (`call`, `free_buffer`, `log` plus an opaque context) and
receives the plugin table (`describe`, `call`, `cancel`, `free_buffer`,
`destroy` plus an opaque instance). Each table starts with its ABI version and
struct size; a host or plugin that does not support the other side's version
fails the handshake. `plugins/sdk/include/lovelycarve_plugin.h` declares the
layout for C and C++; `plugins/sdk/src/abi.rs` is the reference definition.

- A plugin call (`call(instance, call_id, method, params, out)`) runs one
  host → plugin method (`fs.*`, `analyzer.*`, `plugin.shutdown`) on the
  calling thread and fills `out` with the MessagePack result
  (`LC_STATUS_OK`) or a `{code, message}` error (`LC_STATUS_ERROR`). The
  `call_id` identifies the call for `cancel`.
- A host call (`call(context, method, params, out)`) serves one plugin → host
  method (`source.*`, `file.*`, `evidence.*`, `host.progress`) the same way.
- `describe` returns the `plugin.hello` params (protocol range, identity and
  capabilities); the host checks them against the manifest.
- A reply buffer is filled by the callee and released by the caller through
  the callee's `free_buffer`, so the two sides may use different allocators.
- Every function may be called concurrently from any thread. Calls are
  re-entrant: a plugin call may make host calls, and an in-process host call
  may in turn browse a filesystem served by the same plugin.
- The entry point may be called again after a restart to create a fresh,
  independent instance, possibly while an abandoned one is still finishing;
  instances must not share process-global state.
- Panics and other internal failures must not cross the boundary; the Rust
  SDK reports them as `PLUGIN_PANIC` errors.

## Rust SDK

`EvidenceVolume::list` prefers `evidence.list_page` (`evidence_id`, `root`, `path`,
`skip`, `limit`); replies contain `nodes` and `total`, with at most 512 nodes.
This additive LPP v1 method keeps large directories below the 8 MiB frame
limit. The SDK checks progress and a stable total, supports cancellation on
every page, and falls back to `evidence.list` on older hosts returning
`METHOD_NOT_FOUND`. The original unpaged method remains available to old DLLs.

`EvidenceVolume::find_files_by_name(names, prefixes, suffixes)` uses the additive
`evidence.find_names_page` method. Filters are literal ASCII case-insensitive
file-name matches, not glob patterns: 1..32 strings total, each nonempty and at
most 128 bytes without separators or NUL. Requests carry `evidence_id`, `root`,
the three arrays and nil/string `cursor`; replies carry `paths`, cumulative
`visited`, and nil/string `cursor`. A cursor is single-use, owner/root scoped.
Each call traverses up to 64 directories by native node ID, skips deleted nodes
and links, checks cancellation per directory/node, and reports unreadable
directories as errors. Bounds are 4 million nodes, depth 128, 262144 directories,
4096 matches, 4096 bytes per path and 2 MiB total paths; each evidence handle
allows 8 pending scans. Handle cleanup/image unload drops cursors. Both isolation
modes use the same dispatcher; process task cancellation and in-process connection
closure cancel native batches. The SDK checks cancellation between batches,
monotonic counts, cursor reuse and response budgets, and returns only completed
sorted results. Older hosts return `METHOD_NOT_FOUND`; callers may explicitly
fall back to a bounded directory walk.

`plugins/sdk` (crate `lovelycarve-plugin`) implements the ABI. Plugin authors
implement `FileSystemParser` (with a `Volume` per opened session),
`FileAnalyzer`, `EvidenceAnalyzer`, or any combination, register them on a
`Plugin` whose id, name and version match `plugin.toml`, and export it:

```rust
fn plugin() -> lovelycarve_plugin::Plugin {
    lovelycarve_plugin::Plugin::new("org.example.parser", "Example Parser", "0.1.0")
        .filesystem(ExampleParser)
        .file_analyzer(ExampleAnalyzer)
}

lovelycarve_plugin::export_plugin!(plugin);
```

The crate is a `cdylib` whose `[lib] name` matches `library.path`. Plugin
code runs on host worker threads and may block; all API calls are
synchronous. The SDK provides:

- `Source` (block device of `fs.probe` / `fs.open`) with a 64 KiB × 64-block
  LRU cache, 4 MiB host-read chunking and `readv`;
- `File` (`read`, `readv`, `materialize` with a lease released on drop);
- `Evidence` / `EvidenceVolume` (`roots`, `find_root`, `stat`, `list`,
  `read`, `read_text`, `read_lines`, `sub_items` / `wants`, `progress`);
- `Context`: every host call made through a handle bound to the running
  request fails with `CANCELLED` once the host cancels it, so analyzers that
  read evidence stop promptly; CPU-only loops should poll
  `context.check_cancelled()`;
- result types (`AnalysisResult`, `Artifact`, `TimelineEvent`, `Finding`,
  `ResultNode`, an ordered `Attributes` map) and `LogLevel` logging to the
  LovelyCarve log (target `plugin`).

The workspace in `plugins/` holds the SDK, `lc-plugin-host` and the bundled
plugins. `just build-plugins` builds it and copies each plugin library next to
its `plugin.toml`; `just build-plugins --release` does the same for bundles.
`just test-plugins` and `just clippy-plugins` test and lint the workspace.

## Transport (process mode)

LovelyCarve starts `lc-plugin-host --library <path>` from the executable's
directory (development builds also look in `plugins/target/{debug,release}`;
`LOVELYCARVE_PLUGIN_HOST` overrides the location). The loader turns host
requests into plugin calls on worker threads and plugin host calls into
requests back to LovelyCarve:

- stdin is protocol input, stdout is protocol output, and stderr is logs.
- Each frame is a four-byte big-endian payload length followed by MessagePack.
- Frames are limited to 8 MiB. `source.read` is limited to 4 MiB.
- Host request IDs are even; loader request IDs are odd.
- Binary data uses MessagePack binary values, never Base64.

The reader and writer loops remain active while an RPC awaits its response. A
plugin may therefore call `source.read` while handling an outer host request
without deadlocking the connection.

The host keeps at most 256 outbound requests pending and processes at most 16
concurrent plugin-to-host requests. Invalid versions, request-ID parity,
duplicate IDs, oversized frames, or unexpected messages close the connection
and fail pending calls. Replies larger than the 8 MiB frame limit fail with
`RESOURCE_LIMIT`; the in-process mode applies the same limit to plugin
replies, so both modes accept the same results.

## Handshake

In process mode the loader's first message is an odd-ID `plugin.hello`
request carrying the plugin's `describe` output: its protocol range,
identity, and capabilities. The host selects LPP v1 and returns its
frame/read limits. In-process instances are checked the same way. An
incompatible protocol range, an identity that differs from the manifest id,
or a declared capability the plugin does not advertise ends startup.

## Lifecycle, source, and filesystem methods

- `plugin.shutdown`: closes plugin sessions during application shutdown or a
  restart. The host waits five seconds for the reply; a loader is then given
  two seconds to exit before it is killed.
- `source.info`: returns `source_id`, size, sector size, and `read_only: true`.
- `source.read`: returns bytes from a registered logical source. Reads clamp at
  the logical source boundary.
- `source.readv`: reads at most 128 ranges and 8 MiB total; every individual
  range is also bounded by the 4 MiB single-read limit. All ranges are validated
  before evidence is read.
- `fs.probe`: identifies a logical source and returns filesystem, confidence,
  and an optional label. A confidence of 0 means "no match"; any other result
  must name a declared filesystem.
- `fs.open`: creates a cached plugin session and returns its opaque root node.
- `fs.children`, `fs.stat`, and `fs.read_range`: implement the existing host
  `Volume` operations. Large host streams are composed from bounded range reads.
- `fs.children_page`: returns up to the requested limit and an opaque
  `next_cursor`. The host requests 1,000 entries per page, rejects repeated
  cursors, and caps a directory at 1,000,000 entries. For LPP v1 compatibility,
  `METHOD_NOT_FOUND` falls back to `fs.children`.
- `host.progress`: forwards `current`, optional `total`, and optional `message`
  to the frontend as the `plugin-progress` event, together with `pluginId`.

Metadata calls time out after 15 seconds, directory calls after 60 seconds,
and open/read calls after 120 seconds. A timed-out request is cancelled: in
process mode the host removes it from the pending table and sends a `cancel`
message, which the loader turns into the plugin's `cancel`; in-process the
host calls `cancel` directly. The SDK then fails the request's host calls with
`CANCELLED`.

Opaque plugin node IDs are wrapped with the plugin and session identity before
they reach the frontend. LovelyCarve validates that an ID belongs to the target
session and never asks plugins to interpret host routing IDs.

`fs.children` and `fs.children_page` return the stable LPP v1 `FsNode` DTO.
Required fields are `id`, `name`, and `kind`; optional fields include `size`,
`parent_id`, `deleted`, `inode`, `timestamps`, `link_target`, and
namespaced `attributes`. Host-only implementation fields are not part of the
wire contract. `fs.stat` similarly uses the stable `FsStat` DTO instead of
serializing LovelyCarve's internal Rust type.

Plugins start lazily. Loader EOF, malformed output, and RPC failures fail
pending calls without crashing LovelyCarve. The manager records a `Crashed`
state (also when a library is missing or fails to start); expanding the
Unknown partition again discards its stale session and starts a fresh
instance. Unloading an image sends `fs.close`, unregisters its logical
sources, and application exit sends `plugin.shutdown`.

## File analyzer methods

The host invokes a declared analyzer through `analyzer.analyze` with its
analyzer ID and an opaque `FileHandle`. The handle contains `file_id`, a safe
display name, logical size, MIME type, and `read_only: true`; it contains no
evidence path. During that call the plugin can use:

- `file.info`: return handle metadata after owner validation.
- `file.read`: read at most 4 MiB, clamped to the logical file boundary.
- `file.readv`: read at most 64 ranges and 16 MiB total.
- `file.materialize`: create a host-owned temporary path if
  `file_materialize` was approved. One file is capped at 4 GiB; each plugin is
  capped at 16 active leases and 8 GiB total.
- `file.release`: release a lease early. The host also removes all outstanding
  handles and leases after analysis, plugin restart/crash, image unload, and
  shutdown.

Analysis has a ten-minute deadline. Its LPP v1 response is a map containing an
optional `summary` and zero or more `nodes`, `artifacts`, `timeline`, and
`findings`:

```text
node     = { type, name, columns?: [{ key, label }] }   # 结果树数据项 + 表格列
artifact = { type, title, value?, attributes? }
timeline = { timestamp, type, description, attributes? }
finding  = { severity, title, description, attributes? }
```

`nodes` declares the analysis-results tree and its columns: each entry is one data item
(`linux.user` → 用户列表) and carries the record-table column definitions
(attribute key + display label), so the frontend renders plugin output with
zero hardcoded columns. Nodes with zero matching artifacts are not shown.

Types and attribute keys are namespaced lowercase identifiers. Timeline values
must use RFC 3339. Finding severity is `info`, `low`, `medium`, `high`, or
`critical`. Binary/extension values, duplicate object keys, non-finite numbers,
excessive nesting, and oversized results are rejected. At most 10,000 total
items and 64 result nodes (64 columns each) are accepted. With an active case,
accepted results are transactionally stored in `plugin_analysis_runs`,
`plugin_result_nodes`, `plugin_artifacts`, `plugin_timeline_events`, and
`plugin_findings`; removing a task removes its stored runs, and removing evidence cascades to
all associated plugin analysis rows.

## Evidence-level analyzer methods

### Source-file navigation in analysis results

Artifacts can declare evidence-relative provenance in their existing attributes:

- `source.root`: an `EvidenceRoot.root_id`, preserving partition, LVM or Docker container identity.
- `source.path`: the exact POSIX path relative to that root; preserve case and filename whitespace.
- `source.paths`: optional additional paths in the same root (the primary path remains first).
- `source.node`: optional native `EvidenceNode.node_id`, used when no root/path address is available.

The results table locates the primary source; details expose each path separately. Live path resolution
does not require a filedb index, follows intermediate directory symlinks within the selected root,
and reveals final symlinks themselves. Node-only navigation uses the existing filedb lookup.
For file-level runs, the host also returns the persisted input node as a fallback, including older
results. Artifact IDs, titles and arbitrary values are never interpreted as file addresses.


`type = "evidence_analyzer"` capabilities declare functions that analyze the
whole evidence instead of one target file. The host invokes them
through `analyzer.analyze_evidence` with the analyzer id, an evidence handle
(`evidence_id`, `image_id`, and the task params map, whose `subItems` names the
sub-items to run; empty means all, which is what the desktop quick analysis
always sends), and a thirty-minute deadline. During
that call the plugin can additionally use:

- `evidence.roots`: the browsable volume roots of the evidence
  (`root_id` is the volume key `part:N` / `lvm:N:M` / `root`, plus `fs_prefix`,
  `kind`, `size`) — the "@rootfs" search enumerates these.
- `evidence.stat {root, path}`: node DTO (`node_id`, `name`, `kind` of
  `file`/`dir`/`link`, `size`, `deleted`, `link_target`, RFC 3339
  `timestamps`) or nil when the path does not exist. Symlinks are followed
  (`/etc/os-release` → `/usr/lib/os-release`), bounded at 16 hops.
- `evidence.list {root, path}`: children of a directory (empty path = the
  volume root); listed entries are not followed, so `/etc/localtime` keeps its
  `link_target`.
- `evidence.read {root, path, offset, length}` / `evidence.readv`: bounded like
  `file.read` (4 MiB per call, 64 ranges / 16 MiB per readv), clamped to the
  file boundary.

Evidence handles are owner-scoped and removed when the analysis ends (also when
the task is cancelled and the host stops waiting) or the image unloads. `host.progress` works the same as
for file analyzers. The result contract and `emit_artifacts` permission check
are identical to file analysis. Evidence-level functions are scheduled without
a target `nodeId`; the task params JSON (`platform`, `subItems`) is forwarded to
the plugin verbatim.

## Process boundary and resource limits

`lc-plugin-host` starts with a cleared environment and a small allowlist needed
to locate system libraries and temporary directories, and on Windows without a
console window. A Windows Job Object applies kill-on-close, a single-process
limit, and a 512 MiB per-process memory limit. All platforms use Tokio
kill-on-drop, RPC/frame/concurrency limits, timeouts, handle ownership,
logical range checks, and temporary-file quotas. The current non-Windows build
does not yet provide a native process-tree or memory sandbox; deploy untrusted
plugins inside an OS sandbox there.

An in-process plugin shares LovelyCarve's address space, environment and
memory: the handle ownership, range checks, limits and timeouts still apply,
but nothing contains a crash, and a call that ignores `cancel` keeps its
thread until it returns. The instance is destroyed only after its last call
has returned.

## Task scheduling lifecycle

Analyzer runs can be submitted as **scheduled tasks** instead of a blocking
`plugin_analyze_file` call. The scheduler (`src-tauri/src/core/extensions/task_scheduler/`)
persists task state in the case ledger (`case.db`) and creates a plugin
instance for each execution. Its main components are listed below.

| Component | Implementation |
|---|---|
| Persistence | `task` / `task_followup` tables in the case ledger `case.db` (`store.rs`, `store_evidence.rs`) |
| Functions | `FunctionRegistry` built from manifests: one function per `file_analyzer` **or** `evidence_analyzer` capability, named `<plugin id>/<analyzer>`, with `platform`/`support`/`evidence_types` metadata |
| Execution | Dispatch loop → `PluginTaskRunner` → task-specific instance → `analyzer.analyze` or `analyzer.analyze_evidence`; progress belongs to the task, and results are persisted before completion |
| Notifications | `sched-task-event` Tauri event |
| Controls | `sched_task_*` commands for create, cancel, pause, start, restart and remove |
| Offline and timeout handling | `watchdog.rs` + `handle_offline` |
| Dependencies and first tasks | `dependence.rs` |
| Completion actions | `post_action.rs` |

**State machine** (`task.status`, enforced by a CHECK constraint): `created → queued
→ running → success | failed | cancel`, with `pause` and `offline` as recoverable
intermediate states. `created` means not yet dispatched; `queued` means claimed
for a plugin worker; `running` stamps the start time; terminal states stamp the
finish time, from which `spendTimeSecs` is derived. Plugin instances are tracked
in memory only. A task's `params` JSON carries the target `nodeId` / `fileName`;
the plugin, analyzer, trigger, parent task and last error are columns of their
own, reported on the wire as `info` together with the evidence's `imageId`;
`result` JSON carries the summary, artifact / timeline / finding counts and the
run id persisted in the evidence's `analysis.db`. Timestamps are RFC 3339 UTC.

**Concurrency.** Every execution gets a plugin instance of its own
(`PluginManager::start_task_worker`): a separate `lc-plugin-host` process with
its own 512 MiB limit, or a separate instance of an in-process library. Tasks
of one plugin therefore run side by side. The dispatcher claims `created` tasks
in creation order while fewer than `LOVELYCARVE_TASK_MAX_RUNNING` (default 4,
at most 32) tasks are queued or running, across all plugins. `host.progress`
from a task's instance belongs to that task alone; it only updates the
scheduler's in-memory record (the watchdog sees it at once), and the dispatch
loop writes the latest percentage to the case ledger, so a plugin never waits
on SQLite for its progress acknowledgement. The plugin's shared instance, used
for filesystem browsing and `plugin_analyze_file`, is separate from task
instances. `single_instance` functions additionally refuse a second active task
for the same `(function, evidence)` — `sched_task_create` then returns the
existing task with `created: false`; a partial unique index on the ledger backs
the rule.

**Database writes.** The scheduler runs all ledger reads and writes on the
blocking thread pool. Status changes check the current status and write in one
`BEGIN IMMEDIATE` transaction, so a commit, a cancel and the watchdog racing on
one task cannot overwrite each other's terminal state, and progress writes only
touch `running` rows. A task-row write and its `sched-task-event` happen under
one ordering lock, so events arrive in write order and a stale "running"
progress event never follows a terminal one. Result persistence and removal in
the evidence's `analysis.db` and the timeline hook (built-and-not-rebuilding
check, replace by run, watermark) are each one `BEGIN IMMEDIATE` transaction on
that evidence's own database: tasks of one evidence finishing together queue on
its write lock, other evidence is unaffected, the event watermark only moves
forward, and WAL readers are never blocked.

**Controls.** Cancel is allowed from `created/queued/running/pause/offline`; a
running execution has its token revoked and its own plugin instance ended: a
process-mode instance's loader is killed, while an in-process instance's running calls are cancelled
cooperatively. Other tasks of the plugin keep running. A late commit from the
superseded execution is discarded. Pause requires `task.can_pause`; because LPP has no
cooperative pause yet, pause stops the execution and `start` re-queues it from
scratch. Restart of a terminal task creates a new task whose `info.parentTid`
points at the original; restart of an offline task simply re-queues it. Remove
only accepts terminal tasks. Removing evidence first cancels its unfinished
tasks; deleting the evidence row then removes its tasks by cascade and the
scheduler emits `taskRemoved` for each.

**Dependence chain.** When a task succeeds, every function whose `dependence`
equals the finished function gets a new task on the same evidence with the same
file params and `trigger = auto`. Failure or cancel blocks the chain
(`TaskFailedBlocked`). When evidence is registered in a case for the first time,
every `trigger = "auto"` function without a `dependence` gets a first task;
an evidence-level analyzer can run without a target file. A file-scoped root
task without `nodeId` fails with "任务缺少目标文件 nodeId"; use automatic
file-scoped analyzers downstream in a chain that supplies file parameters.

**Watchdog.** A periodic scan of running tasks marks a task `failed` (notice
`taskTimeout`) when no progress arrived within the update timeout, and marks it
`offline` (notice `taskOffline`) when the task's plugin instance is gone after
the offline timeout. The offline handler then re-queues `auto` tasks and fails
manual ones. On case open
the scheduler also converts any `queued`/`running` rows left by a previous
session to `offline` and runs the same split. Timeouts are read from
`LOVELYCARVE_TASK_CHECK_INTERVAL` (default 15 s), `LOVELYCARVE_TASK_OFFLINE_TIMEOUT`
(60 s) and `LOVELYCARVE_TASK_UPDATE_TIMEOUT` (900 s).

**Post actions.** The `PostActionWatcher` counts created/finished tasks per
evidence (restored from the ledger on case open). When every task of an
evidence is finished it starts a debounce (5 s); when it fires it emits an
`allTasksFinished` notice and executes the JSON list in `task_followup.actions`
through registered handlers, stamping `done_at` only after all actions succeed
so retries stay idempotent. `sched_evidence_post_actions_set` writes that row.

**Events.** `sched-task-event` carries `{ kind: "taskChanged", task }` on every
status/progress change, `{ kind: "taskRemoved", taskId, evidenceId }` on delete,
and `{ kind: "notice", notice }` for `taskOffline` / `taskTimeout` /
`allTasksFinished` / `postActionFailed`.

## Application IPC and management UI

The Tauri IPC layer exposes `plugin_list`, `plugin_scan_report`, `plugin_rescan`, `plugin_restart`,
`plugin_analyze_file`, and `plugin_analysis_list`. Frontend code calls the
typed wrappers in `src/lib/ipc/plugins.ts`. The settings plugin page shows
state, capabilities (filesystem, file and evidence analyzers), permissions,
directory, library file, isolation mode, and fingerprint, and can restart a
plugin without restarting LovelyCarve. Manual rescans run on a blocking worker
and publish `plugin-catalog-changed` with a generation number; new plugins appear
in the settings and analysis menus without replacing live instances. Changed
plugins retain their current definition and require an application restart.
Removed plugins stop accepting new calls while captured task workers and
filesystem sessions stay alive. Unknown manifest fields are rejected. Additional
package files are declared as `[library] resources = ["helper.dll", "data/map.json"]`.
Plugins are trusted on discovery —
there is no approval or disable flow.

Scheduled tasks use `sched_task_functions`, `sched_task_list`,
`sched_task_create`, `sched_task_cancel`, `sched_task_pause`, `sched_task_start`,
`sched_task_restart`, `sched_task_remove`, and
`sched_evidence_post_actions_set`, wrapped by `src/lib/ipc/tasks.ts` and consumed
through `src/features/tasks/composables/useTaskCenter.ts`. The file list's
context menu entry「插件分析任务…」submits the selected file to a chosen
function. The titlebar「任务中心」panel
groups the current evidence's tasks into running, attention and finished
sections with progress, remaining-time estimates and the cancel / pause /
start / restart / remove controls, links successful runs to their analysis
results, and lists recent scheduler notices. The hub follows the current
evidence tab; closing it leaves background tasks running. Its「快速分析」tab
groups applicable evidence analyzers by plugin and category. Selecting a
plugin submits all of its applicable functions for the current platform,
without sub-item selection; each plugin tile shows its latest task status.
File analyzers that require a target file use the context menu instead.

The「分析结果」panel (`src/features/analysis/components/AnalysisResultsPanel.vue`)
browses stored results through `analysis_result_tree`,
`analysis_records_page`, and `analysis_record_detail` (wrapped by
`src/lib/ipc/analysisResults.ts`, backed by
`src-tauri/src/core/analysis/results/analysis_query.rs`): a tree of
evidence → class → function → result item (ChildCount badges, latest run per
function), a server-paged record table whose columns come from the
plugin-submitted node declarations, and a per-record detail pane. Completing a
task only marks the tree dirty; the panel re-pulls the whole tree when
activated.

## Bundled plugin

`plugins/linuxinfo/` is the only bundled analyzer. Its process-isolated
`linuxbaseinfo` capability covers fifteen sub-items: system information,
timezone, users and groups, login records, history, network, USB, services,
software, scheduled tasks, startup entries and recent access.
See [LINUXINFO.md](LINUXINFO.md) for evidence sources and parsing limits.

The workspace also retains `plugins/sdk` and `plugins/host` as infrastructure.
Both process and in-process isolation remain supported by the SDK and host.
`just build-plugins` builds the loader and stages the platform library beside
`plugins/linuxinfo/plugin.toml`. `just pack-plugins` creates a Windows ZIP with
linuxinfo, the process host, license notices and SHA-256 checksums.

`just test-plugins` tests the SDK, process host and linuxinfo.
For the optional real-image integration test, set `LOVELYCARVE_TEST_LINUX_IMAGE`
to a local Linux image, run `just build-plugins`, then
`just test linuxinfo_plugin_analyzes_real_image -- --ignored --nocapture`.
Private evidence and generated results stay outside version control.

`just run-example probe_plugin <image> org.lovelycarve.linuxinfo linuxbaseinfo <output-json>`
checks execution, result persistence and pagination.
`just run-example probe_evidence_path <image> <root-id> <path>...`
checks the same read-only source paths.
