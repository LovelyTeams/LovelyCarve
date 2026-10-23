# LovelyCarve integration

Source: crates.io `lamfold 0.1.5`, package VCS commit
`fabaad6af0c28b80535307b339b84110f3a3c12e` from
https://github.com/lamco-admin/lamfold. MIT OR Apache-2.0; original notices kept.

Local changes: `FoldError::Source(String)` preserves backing evidence I/O context.
Host dependencies disable default features; only `std` and `codec-deflate` are
enabled. No optional C codec or verifier is needed. The source, codec and frontend
contracts are reused by the two ROM filesystem readers; format-specific hardening
is recorded in their sibling directories. See docs/formats/ROM_IMAGES.md.
