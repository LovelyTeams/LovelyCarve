zfs-rs
======

A ZFS implementation and file recovery tool written in Rust.

Automatic recovery from
-----------------------
 - Unreadable/corrupted MOS config by scanning MOS for root DSL dir

Supported Features
------------------
 - Reading files from datasets on a single drive
 - ZFS System Attributes
 - ashift=11 is hardcoded

Unsupported Features
--------------------
 - Large DNodes
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
