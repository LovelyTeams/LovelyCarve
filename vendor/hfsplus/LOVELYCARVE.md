# LovelyCarve integration

HFS+ 0.4.0 from Dil4rd/dpp at 3a73ee28fdab192fcdfbe8edcad7b520867f2417 (MIT).

Preserved local access/attribute-modification times and permissions admin_flags. Upstream cmpfs provides compressed file reads and attributes.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).


## 2026-10-13 libfshfs comparison

Reference: libfshfs 78321de1eda54b30261adf59b938781128591b2e; all 95 public APIs
are mapped in [HFS.md](../../docs/formats/HFS.md). Complete allocation plans
cover data/resource overflow and attribute continuations; logical full/stream/seek
reads agree. Added bounded B-tree traversal, geometry/source checks, alternate
header provenance, CNID/raw UTF-16 access, hardlinks and BSD/Finder metadata.
The host now reads compressed files and exposes direct ranges and symlink targets.
Decoded files/resource inputs are capped at 512 MiB and fork xattrs at 16 MiB.

`src/unicode/folding.rs` is a Rust data transformation of Apple's fixed tables,
from hfs d1bac2f062e6e9c0dfcce302d9aacb10173d0eea, core/UCStringCompareData.h.
The original copyright and APSL header are preserved; APPLE_LICENSE contains the
full license. LovelyCarve changed the representation to two Rust arrays; it did
not change table values. This generated file alone is APSL-2.0, not MIT.
`src/unicode/decomposition.rs` comes from Python unicodedata.ucd_3_2_0 using NFD,
excluding U+2000–2FFF, U+F900–FAFF and U+2F800–2FAFF; Hangul is algorithmic.
Combining classes also come from the same fixed data. UNICODE_LICENSE is retained.
Generated data modules can exceed the ordinary hand-written module size target.

Checks: just test-vendor hfsplus (41); just clippy-hfsplus; just test-vendor cmpfs.
The FileVault oracle also exercises actual HFS catalog/full/stream/seek after
CoreStorage decrypt. No native libfshfs binary comparison was run.
