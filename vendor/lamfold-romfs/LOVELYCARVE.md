# LovelyCarve integration

Source: crates.io `lamfold-romfs 0.1.1`, package VCS commit
`fabaad6af0c28b80535307b339b84110f3a3c12e` from
https://github.com/lamco-admin/lamfold. MIT OR Apache-2.0; original notices kept.

The lamfold dependency points to `../lamfold`. The original Linux ROMFS reader's
header/directory/hardlink/random-read implementation is reused. Local patches:

- Validate the filesystem size and superblock checksum, file-header checksum
  (including name/padding), aligned header bounds and complete file extents.
- Strict UTF-8 names, duplicate-name and next-chain cycle/limit errors; do not
  silently stop traversal at a count limit. Preserve executable permission bits.
- Reject reads from special nodes; retain the upstream bounded hardlink resolver.

The host additionally rejects directory ancestry cycles, separates hardlink path
identities and retains I/O errors. No checksum can validate arbitrary file data
beyond the format-defined covered regions. See docs/formats/ROM_IMAGES.md and the
independent synthetic encoder in test_data/rom-images/generate.py.
