# Vendored NTFS reader

Source: https://github.com/ColinFinck/ntfs (post-0.4.0 upstream master).
License: MIT OR Apache-2.0; see the vendored license files.

Local compatibility changes:

- Expose validated non-resident storage geometry and a raw mapped-span reader
  for bounded LZNT1 decoding in the host filesystem adapter. The standard
  `value()` method retains its logical-length contract.
- Preserve the first extent of a connected attribute when it lives in the base
  MFT record, and suppress duplicate base attributes after the list traversal.
  This allows sparse compression padding to continue in another MFT record.
- Existing local attribute-list corruption guards terminate malformed lists.

The host owns the pure Rust decoder and its regression fixtures. Validate with
`just test core::filesystem::volumes::ntfs --lib`; format through `just fmt`.

- Join connected attributes by raw UTF-16 name, type and consecutive VCNs;
  attribute instance IDs are local to each physical record, not stream IDs.
  Validate list name offsets, referenced names/VCNs and nonzero sequence numbers.
- Bootstrap and cache the complete MFT map without recursive attribute-list
  reads; support records crossing physical runs. Reject incomplete/cyclic maps.
- A run with no LCN field is sparse; an explicit zero LCN delta is allocated.
  Physical short reads fail, and malformed run/list iterators terminate once.
- Expose raw attribute type/flags, all storage extents, record base reference
  and LSN. Validate full resident/nonresident headers before field access.
- Named streams still work by exact name when a damaged $UpCase cannot load.
- `NtfsAttributes::of_type` filters attribute-list entries before dereferencing
  them. Reparse lookup no longer opens stale references for unrelated deleted
  attributes; matching references retain sequence/name/VCN validation and the
  connected non-resident reader.

Reader-level regression: `just test-vendor ntfs`; host capability regressions:
`just test ntfs --lib`. Full comparison: `docs/formats/NTFS.md`.
