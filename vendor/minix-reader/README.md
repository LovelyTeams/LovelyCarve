# minix-reader

Pure Rust, read-only Minix V1/V2/V3 frontend over `lamfold::BlockSource`.
No writes, repair, mounting, FFI or external extraction. Adapted metadata code
is attributed to Dani / rusty-backup; see [ORIGIN.md](ORIGIN.md).

The reader supports 14/30/60-byte UTF-8 names, little/big-endian metadata with
Linux/util-linux little-endian bitmap indexing, direct/single/double/triple
addressing, multi-block zones, holes, hard links and unfollowed symlinks.
Source ranges, geometry, allocation bits and logical lengths are checked.
Metadata and inode caches are bounded and private to a reader instance.

```rust,ignore
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use minix_reader::Minix;
let mut cache = BlockCache::new(0);
let mut context = SubstrateCtx { cache: &mut cache, verifier: &NoVerifier };
let mut reader = Minix::open(SliceSource::new(&image), &mut context)?;
let entries = reader.read_dir(reader.root(), &mut context)?;
```

Host support, limits and validation are documented in
[docs/formats/MINIX.md](../../docs/formats/MINIX.md).
