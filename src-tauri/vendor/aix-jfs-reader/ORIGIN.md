# AIX JFS1 reader origin

The original algorithms were adapted from Dani Sarfati's MIT `fs-unix` 0.2.1 AIX JFS reader, [revision `4969de813a1ec4439ccce69a1247c2ecf3f50cb4`](https://github.com/danifunker/rusty-backup/blob/4969de813a1ec4439ccce69a1247c2ecf3f50cb4/crates/fs-unix/src/jfs1.rs). The official crates.io archive has SHA-256 `86b61279b0d3904552f0e900919d79be30cd0d151a7bf2e50437c25233c56a3e`. `LICENSE-MIT` preserves upstream terms. The local crate is AGPL-3.0-only and adds mirrored-superblock selection, packed fragment validation, bounded seeks and POSIX timestamp roles.

Disk layout details were confirmed with IBM's [JFS file system layout](https://www.ibm.com/docs/en/aix/7.1.0?topic=volumes-jfs-file-system-layout), [fragment address documentation](https://www.ibm.com/docs/en/aix/7.1.0?topic=volumes-allocating-jfs-file-space), [file times](https://www.ibm.com/support/pages/file-times-aix) and [`inode.h` reference](https://www.ibm.com/docs/en/aix/7.2.0?topic=files-inodeh-file). The pinned upstream crate was developed against an AIX 4.3.3 rootvg; the native images are not published in its crate or Git repository. The local test corpus is synthetic and self-contained.

`python scripts/make-aix-jfs-fixtures.py` recreates the 1,048,576-byte images, compressed with zlib for version control. Each fixture has directory records, native numeric inode identities, packed tails and the expected data segments explicitly set by the generator. It does not call the Rust library under test.

All five synthetic raw images were also opened with the unmodified pinned upstream reader. The root and nested entries, links, data in a later allocation group, and the results across both indirect addressing levels agreed. Local tests also cover behavior added by this adapter, including fallback to the standby superblock and errors for short allocated fragments.

| Raw image | SHA-256 |
| --- | --- |
| `fsv3` | `21f3a7e06ce12c6377a44c19e0ff1db075b2d52b411eb62a2b899efab493a7e6` |
| `fsv3p-512` | `58b384b6ced55bf512804c2e08e3c2db5da1bf25f09b1c30dd86cf588f31f687` |
| `fsv3p-1024` | `35295677e70ae596c3f92521347c64607f0142e7a107db59193f1430fe783baf` |
| `fsv3p-2048` | `ceda68d0f66d468ac5f2d73d1b4df4fb10caff39be47885d65fef957622eb433` |
| `fsv3p-4096` | `fc37eac1ac6ac6f0bbef209040dd6d84d3520135a2a18cd2cd49ab2f1954a2b9` |
