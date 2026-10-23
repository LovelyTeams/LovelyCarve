# ocfs2-reader 0.1.0

Original immutable Rust reader, AGPL-3.0-only. Existing Rust libraries were
searched before implementation. `unearth` supplies OCFS2 detection, without
directory or file reading; `reflicate` uses mounted-filesystem reflink ioctls.
The superblock positions and prefix layout are adapted from unearth, MIT:

https://github.com/MarcelRoozekrans/unearth/blob/bc891261221f5f5134269780d6c206725b21d52e/src/ocfs2.rs

Its original MIT notice is retained as LICENSE-UNEARTH. New reading code uses
format facts from Linux revision `7a78f7ba9f24962c8033e0152bf34a7903ff61ba`,
`fs/ocfs2/ocfs2_fs.h`, with kernel OCFS2 interpretation used for comparison.
No C implementation is compiled, linked, or required by the product.
`crc32fast` and `lamfold` are pure Rust runtime dependencies.

Native test inputs are produced by `scripts/ocfs2-native-fixtures.py` on a
disposable Linux host, using mkfs.ocfs2, the kernel, and fsck.ocfs2 -fn. The
invented payload generator is separate from Rust. Native inputs remain in an
external cache; they are not forensic evidence or bundled sample user data.
Capabilities and explicit limits are recorded in
[OCFS2.md](../../../docs/formats/OCFS2.md).

## Native, independent verification

Debian Linux 6.1.0-54-amd64 and ocfs2-tools 1.8.7-1+b1 generated and fsck-checked
four 512 MiB images. The external manifests record kernel names, file hashes,
modes, sizes, nanosecond timestamps, symlink targets and all attribute bytes.
The Rust audit checks every name and full/ranged content (1517 names in the
first case, 1519 in each refcount-enabled case). Advanced inputs include 1000
fragmented extents, alternating reflink/COW and a multi-leaf refcount tree.
These invented CC0 inputs remain external; the compact self-encoded fixtures
and their generator are separate and never serve as a native formatter oracle.

Decoded image SHA-256:

| Block / cluster size | SHA-256 |
| --- | --- |
| 512 / 4096 | e27097c18d6d569f3062ba989f7e273c2f2faf3b9fa65350918d15649cf793f3 |
| 1024 / 4096 | 8c4757354aadcc270b4761d9015a3329ab532f511f492533b08e43a7cb917999 |
| 2048 / 16384 | b080aee4eadebd1ae75e5471058f0ca09a089de7ad688f6b2a49c933ef331a0c |
| 4096 / 65536 | b3e22f3789261f69c761a219c84411aa66ed3316960754db8ad93390bd038796 |

Verification does not prove online cluster behavior, all cluster sizes,
multi-slot journals or recovery. Actual discontiguous suballocator layouts
are covered by independent invented inputs; native images enable the feature
without claiming their allocated groups are fragmented.
