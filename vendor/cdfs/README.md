cdfs
====
[![builds.sr.ht status](https://builds.sr.ht/~az1/iso9660-rs/commits/master.svg)](https://builds.sr.ht/~az1/iso9660-rs/commits/master?)
[![docs.rs status](https://docs.rs/cdfs/badge.svg)](https://docs.rs/cdfs/)

ISO 9660 / ECMA-119 filesystem implementation written in Rust.  It's still very much a work-in-progress.  The project overview lives at Sourcehut:

https://sr.ht/~az1/cdfs/

Usage
-----
See the examples directory and the [documentation](https://docs.rs/cdfs/) for more information on how to use the `cdfs` library in your project.

If you're just interested in using `cdfs` to mount an ISO, there's a [FUSE](https://en.wikipedia.org/wiki/Filesystem_in_Userspace) implementation included.

To install locally:

```
$ cargo install cdfs
…
   Compiling cdfs v0.2.2
    Finished release [optimized] target(s) in 21.97s
  Installing /home/user/.cargo/bin/cdfs_fuse
   Installed package `cdfs v0.2.2` (executable `cdfs_fuse`)
$ cdfs_fuse images/rockridge.iso mountpoint/
2023-09-06T00:00:00.203Z INFO  [cdfs_fuse] NOTE: The filesystem must be manually unmounted after exit
2023-09-06T00:00:00.206Z INFO  [cdfs_fuse] Found POSIX.1 extensions with usable inodes.
2023-09-06T00:00:00.206Z INFO  [fuser::session] Mounting mountpoint/
```

Or run `cdfs` directly from the repository like so:

```
$ cargo run -- images/rockridge.iso mountpoint/
    Finished dev [unoptimized + debuginfo] target(s) in 0.15s
     Running `target/debug/cdfs_fuse`
2023-09-06T00:00:00.203Z INFO  [cdfs_fuse] NOTE: The filesystem must be manually unmounted after exit
2023-09-06T00:00:00.206Z INFO  [cdfs_fuse] Found POSIX.1 extensions with usable inodes.
2023-09-06T00:00:00.206Z INFO  [fuser::session] Mounting mountpoint/
…
```

Supported ISO 9660 Extensions
-----------------------------
| System Use Sharing Protocol  |     |
| ---------------------------- | --- |
| `CE` – continuation area     | yes |
| `PD` – padding field         | no  |
| `SP` – SUSP start            | *not enforced* |
| `ST` – SUSP end              | *ignored*      |
| `ER` – extensions reference  | yes |
| `ES` – extensions selector   | no  |

| Rock Ridge Interchange       |     |
| ---------------------------- |---- |
| `PX` – POSIX attributes      | yes |
| `PN` – POSIX inodes          | yes |
| `SL` – symbolic links        | yes |
| `NM` – long file names       | yes |
| `CL` – child links           | yes |
| `PL` – parent links          | N/A |
| `RE` – relocated directories | yes |
| `TF` – file timestamps       | yes |
| `SF` – sparse files          | no  |

References
----------
* [ECMA-119 standard](https://www.ecma-international.org/publications/standards/Ecma-119.htm)
* [ISO 9660 on the OSDev wiki](https://wiki.osdev.org/ISO_9660)
* [Joliet](http://web.archive.org/web/20230604235448/http://littlesvr.ca/isomaster/resources/JolietSpecification.html)
* [Linux kernel isofs module](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/fs/isofs)
* [Rock Ridge v1.12](http://web.archive.org/web/20230107022048/https://aminet.net/package/docs/misc/RRIP)
* [Rock Ridge v1.10](http://web.archive.org/web/20220924230749/https://ia800605.us.archive.org/16/items/enf_pobox_Rrip/rrip.pdf)
* [Rock Ridge v1.09](http://web.archive.org/web/20230826015627/http://ftpmirror.your.org/pub/misc/bitsavers/projects/cdrom/Rock_Ridge_CD_Proposal_199108.pdf)
* [Wikipedia article on ISO 9660](https://en.wikipedia.org/wiki/ISO_9660)

Licensing
---------
This project is available under the Apache 2.0 or MIT license at your choosing.
