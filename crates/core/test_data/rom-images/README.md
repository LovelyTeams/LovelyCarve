# Synthetic ROM image fixtures

Original, invented CramFS v2 LE/BE and Linux ROMFS fixtures and their independent
Python encoder `generate.py` are CC0-1.0. Run with Python 3 (standard library only).
No firmware or third-party filesystem image is included.

`payload.bin` is the uncompressed oracle: a byte ramp, a zero-filled page and a
partial last page. Images also contain a nested note, an empty file, a symlink,
a FIFO, and (ROMFS) a hard link. CramFS files deliberately do not have disk-sector
padding. Host tests independently mutate checksums, extents and chains.
