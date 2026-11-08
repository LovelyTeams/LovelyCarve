# Implementation Notes

Tricky parts encountered while implementing the HFS+ parser.

## B-tree traversal

HFS+ B-trees use variable-size keys. The node descriptor contains the number of records, but offset entries are stored at the **end** of the node (growing backwards). Each offset is a `u16 BE` pointing to the record start within the node.

```
Node layout:
+------------------+
| Node descriptor  |  14 bytes
+------------------+
| Record 0         |
+------------------+
| Record 1         |
+------------------+
| ...              |
+------------------+
| Free space       |
+------------------+
| Offset[N]        |  2 bytes, growing from end
| Offset[N-1]      |
| ...              |
| Offset[0]        |
+------------------+
```

## Extent overflow

Files with more than 8 extents store additional extent records in the extents overflow B-tree. The key is (fork type, file ID, start block). When reading a file, you must check if the inline extents cover the full logical size — if not, look up overflow records.

## Unicode key comparison

HFSX uses binary comparison (straightforward), but HFS+ uses case-insensitive comparison with Apple's custom Unicode folding. Our implementation handles HFSX correctly. HFS+ case-insensitive matching uses the standard Unicode case folding tables.

## Catalog thread records

Thread records in the catalog map a CNID back to its parent CNID and name. They're essential for path resolution — to find `/a/b/c`, we resolve each component by looking up (parent_cnid, "name") in the catalog, starting from CNID 2 (root).

## Block size alignment

Data fork reads must be aligned to the volume's block size. The logical size may be less than `total_blocks * block_size` — the last block can be partially used.

## Date conversion

HFS+ epoch is 1904-01-01 00:00:00 UTC. The offset from Unix epoch (1970-01-01) is **2082844800 seconds**. Dates can be 0 (meaning "not set").
