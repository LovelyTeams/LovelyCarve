zfs-rs
======

A ZFS implementation and file recovery tool written in Rust.

Automatic recovery from
-----------------------
 - Unreadable/corrupted MOS config by scanning MOS for root DSL dir

Supported Features
------------------
 - Reading files from datasets on a single drive
 - ZFS system attributes in the dnode bonus area and spill blocks
 - Standard and large dnodes up to 16 KiB
 - Four vdev label copies with verified 1–8 KiB uberblock slots
 - Seekable reads with sparse block handling, LZ4 and SHA-256 data checksums

Unsupported Features
--------------------
 - raidz
 - mirrors
 - Big Endian
 - zvol
 - ZFS encryption
 - Normalization support

If you require support for any failure mode or ZFS feature not supported please file an issue and I will consider implementing it.

License
-------
Licensed under GPLv2 or later version at your option
