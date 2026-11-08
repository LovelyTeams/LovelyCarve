# Explicitly unsupported capabilities

- QCOW LUKS payloads and crypt-header indirection.
- Zstd compressed clusters and extended L2/subclusters.
- External data-file layouts.
- Historical internal snapshot selection (count is available).
- Writes, refcount repair, compaction, merging and resizing.
- Automatic path-open cancellation and arbitrary legacy filename codepages.

The current supported scope and complete libqcow API comparison are documented
in LOVELYCARVE.md and ../../../docs/formats/QCOW.md. Unsupported layouts fail
explicitly instead of being interpreted as ordinary clusters.
