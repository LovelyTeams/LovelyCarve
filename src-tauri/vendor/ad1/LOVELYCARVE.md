# LovelyCarve integration

Reader crate `ad1-core` 0.1.2 (library name `ad1`) from
<https://github.com/SecurityRonin/ad1-forensic>, commit
`afc3963659a13acf10082e7d76194d3ab789583e`. Apache-2.0; see LICENSE.
Workspace manifest fields are expanded for a standalone crate. The separate
`ad1-forensic` auditor, fuzz targets, hosted CI and the optional
`forensic-vfs` adapter (`vfs` feature) are not included. `docs/format.md` and
`docs/validation.md` are the upstream format notes.

The application uses `core/evidence/readers/ad1_image.rs` and
`core/filesystem/volumes/ad1.rs` to open an
AD1 image as logical evidence: the item tree is browsed, read, searched,
exported and mounted through the normal `fs_tree` paths.

Local changes:

- Segment files are read with positioned reads (`pread` / `ReadFile` at an
  offset) instead of seek-then-read on a `RefCell<File>`, so `Ad1Reader` is
  `Send + Sync` and one reader serves concurrent callers without a lock.
- `read_at` reads only the chunk-table addresses of the chunks the range
  overlaps. Upstream read the whole table on every call, which made a
  sequential stream of a large file quadratic.
- The tree walk reads item headers, names and metadata through two 8 KiB
  read-ahead windows (least recently used replaced) instead of one read per
  field; `SegmentSet::readable_from` keeps the windows inside the data that
  `read` can deliver.
- `Ad1Entry::parent` is the index of the parent folder, filled during the walk,
  so a caller can build the directory tree without re-splitting paths.
- `Ad1Reader::segment_paths` lists every declared segment.
- The header window is read with `read_to_end` rather than one `read` call,
  which may legally return fewer bytes.

Tests: `just test-ad1` runs the upstream suites (crafted fixtures from the
independent `testfix` encoder, malformed input) and `tests/lovelycarve.rs`
for the changes above; `AD1_USERBSS=<userbss.ad1> just test-ad1` adds the
upstream stored-hash oracle on a real FTK image (see `docs/validation.md`).
`just test ad1` covers the application loader, adapter and the end-to-end
`DiskImage::open` path. No real AD1 sample is committed.
