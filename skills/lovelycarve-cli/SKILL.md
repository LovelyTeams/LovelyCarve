---
name: lovelycarve-cli
description: Inspect forensic evidence with lovelycarve-cli, using small structured results and persistent sessions. Use for evidence browsing, text extraction, hashes and exports through this CLI.
---

Discover commands by intent; keep search queries anonymous (action and resource type only, without evidence identifiers or secrets):

```text
lovelycarve-cli cli search "list partitions"
lovelycarve-cli schema files list
lovelycarve-cli docs search "file export read faults"
```

Reuse an opened image across calls:

```text
lovelycarve-cli -q sessions open exam --image "<evidence-path>"
lovelycarve-cli -q --session exam images get
lovelycarve-cli -q --session exam files list --path / --page-size 50 --fields name,id,kind,size,timestamps.modified
lovelycarve-cli -q --session exam context set --node-id "<directory-node-id>"
lovelycarve-cli -q sessions close exam
```

Use a unique session name; close it when finished. `-q` suppresses progress; failures still return nonzero. Redirected/agent JSON is compact. Follow `nextCursor` until null, passing it with `--cursor`; never infer completeness from a truncated tool result. Request small pages and only needed fields. To inspect original times, include `timestampsOriginal`; it is absent when identical. Timezone provenance is response-level; unknown-offset times stay unchanged.

Use returned node IDs (`--parent-id` / `--node-id`) for duplicate names, including duplicate volume names. IDs belong to the opened evidence/session; avoid reconstructing them. `context set --node-id` changes the working directory unambiguously.

For batched JSONL, run `lovelycarve-cli -q session`; use camelCase keys from `schema`, and top-level `fields` (a comma-separated string) for projection. Use `files text get` for decoded text in JSONL; `files read` returns hex there. JSONL can keep large results in a new host file with top-level `output`.

```json
{"id":"text","command":"files text get","args":{"path":"<image-path>"}}
```

Exports create host files and must not overwrite existing paths. When `readFaultsDetected` is true, `badRegions` is nonempty, or `syntheticBytes` is nonzero, hash/export outputs include synthesized zeroes; do not claim those bytes or digests authenticate the source. A failed raw read may already have written partial stdout; discard it. Check full logical length/byte counts and exit status before using outputs. Use `schema` and `docs search` for format and unlock limits; do not treat unsupported or incomplete evidence as empty.
