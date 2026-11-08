# LovelyCarve integration

APFS 0.4.1 from Dil4rd/dpp at 3a73ee28fdab192fcdfbe8edcad7b520867f2417 (MIT).

Preserved local APIs: open_index/list_volumes and indexed VolumeInfo, access/change times, cancellable streaming catalog walk and OMAP cache, and boxed streaming reads for compressed files and symlinks. The walk validates object checksums like upstream lookups. The old local decmpfs/xattr modules are replaced by upstream cmpfs and catalog attribute parsing.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

## 2026-10-13 libfsapfs capability audit

Reference: libyal/libfsapfs cc122619a0fa5cecf7c077742ac62251fb85fb6c;
[complete 110-function comparison](../../docs/formats/APFS.md).
No libfsapfs sources or binaries are incorporated.

Local changes: committed-XID OMAP predecessor/cache and tombstones; checked
NXSB block geometry/checkpoints; corrected object storage flags; bounded
container reads; checked B-tree offsets/levels; plain/hashed directory keys;
volume name lookup policy; strict inode xfields/xattr lengths; unified extent
reads/seek/range with explicit holes, bounds and overlap errors; OID access,
container/volume identity, RDEV and seekable xattr metadata; snapshot metadata.
The host now exposes direct ranges/source runs and symlink targets and keeps
compressed/symlink snapshot metadata aligned with live listings.

APIs changed: ApfsForkReader::new returns Result; open_file returns a ReadSeek
trait object; catalog OMAP parameters accept CatalogContext (legacy u64
converts to the prior hashed policy). New metadata fields preserve source
values. Encryption and historical snapshot browsing remain unsupported.

Dependencies: unicode-casefold 0.2.0 (Chris Wong, MIT OR Apache-2.0) and
unicode-normalization 0.1.25 (Rust Project Developers, MIT OR Apache-2.0),
unmodified crates.io packages pinned in both lockfiles. Source notices are
recorded in THIRD_PARTY_NOTICES.md. Synthetic fixtures have separate project
provenance at tests/data/README.md.

Verification: just test-apfs, just clippy-apfs, just test-vendor cmpfs,
and host Rust recipes. Optional real-image tests remain optional and skipped
when no image is configured; this is not a real-image libfsapfs oracle run.
