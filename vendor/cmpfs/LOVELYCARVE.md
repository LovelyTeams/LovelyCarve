# LovelyCarve integration

cmpfs 0.1.0 from Dil4rd/dpp at 3a73ee28fdab192fcdfbe8edcad7b520867f2417 (MIT).

This shared Apple compression decoder is used by APFS and HFS+. Local changes and regressions are recorded below.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

2026-10-13 libfsapfs comparison: accept the 0xff stored marker for LZFSE
methods 11/12, require the 0xcc RAW marker for method 10, and reject resource
payloads overlapping their offset table. Regression tests cover compressed
LZVN resource blocks, stored LZFSE and multi-block RAW. No C code copied.

2026-10-13 HFS integration additionally bounds decoded output to 512 MiB and limits zlib/LZFSE decoder output to the declared size plus one, preventing expansion beyond the logical-file contract.
