# LovelyCarve integration

Source: crates.io `lamfold 0.1.5`, package VCS commit
`fabaad6af0c28b80535307b339b84110f3a3c12e` from
https://github.com/lamco-admin/lamfold. MIT OR Apache-2.0; original notices kept.

Local changes: `FoldError::Source(String)` preserves backing evidence I/O context.
Host dependencies disable default features. CramFS uses `codec-deflate`; EROFS
also enables the pure Rust LZ4, Zstandard and MicroLZMA codecs. The verifier is
not enabled. `FoldFrontend::timestamps` has a default implementation so readers
can preserve optional POSIX timestamp roles without inventing missing values.
The source, codec and frontend contracts are shared by CramFS, ROMFS, EROFS and
F2FS and QNX4/QNX6; format-specific hardening is recorded in their sibling directories. See
docs/formats/ROM_IMAGES.md, EROFS.md and F2FS.md.
