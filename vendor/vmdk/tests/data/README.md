# tests/data — VMDK Real-Image Corpus

Integration test fixtures and fuzz seed corpus for the `vmdk-forensic`
workspace. This is the single repo-root `tests/data/` directory shared by every
member (`core`, `forensic`, `cli`); each member's integration tests reach these
fixtures with a relative path (`../tests/data/...` from a member directory).

`fuzz/corpus/fuzz_open/` symlinks the four seed images here
(`flat.vmdk`, `flat-f001.vmdk`, `minimal.vmdk`, `stream_opt.vmdk`); the files
are not duplicated.

The fleet-wide machine index for this corpus is
[`issen/docs/corpus-catalog.md`](https://github.com/SecurityRonin/issen);
this README is the co-located human-facing detail. Cross-reference, do not
duplicate.

## Files

| File | Subformat | Virtual size | Supported | Origin | Notes |
|------|-----------|-------------|-----------|--------|-------|
| `minimal.vmdk` | monolithicSparse (v1) | 1 MiB | Yes | qemu-img 11.0.0 (macOS/ARM) | Primary integration test seed |
| `dfvfs_ext2.vmdk` | monolithicSparse (v1) | 4 MiB | Yes | dfvfs test corpus (log2timeline) | ext2 filesystem; VMware4 origin |
| `plaso_image.vmdk` | monolithicSparse (v1) | 100 KiB | Yes | plaso test corpus (log2timeline) | Real VMware Workstation 4 image; non-zero grain data at virtual offset 1024 |
| `stream_opt.vmdk` | streamOptimized (v3) | 1 MiB | Yes | qemu-img 11.0.0 (macOS/ARM) | All-sparse empty disk; GD/GT layout identical to v1 |
| `compressed_stream_opt.vmdk` | streamOptimized (v3) | 64 KiB | Yes | qemu-img 11.0.0 (macOS/ARM) | One allocated compressed grain; decompresses to the `i%64` source pattern |
| `flat.vmdk` | twoGbMaxExtentFlat (descriptor) | 1 MiB | Yes (open_path only) | qemu-img 11.0.0 (macOS/ARM) | Text descriptor; `open()` returns Err, `open_path()` succeeds |
| `flat-f001.vmdk` | (raw FLAT extent for `flat.vmdk`) | — | No (by design) | qemu-img 11.0.0 (macOS/ARM) | Raw extent data, no VMDK header; `open()` returns BadMagic |
| `mono_flat.vmdk` | monolithicFlat (descriptor) | 1 MiB | Yes (open_path only) | qemu-img 11.0.0 (macOS/ARM) | Descriptor referencing `mono_flat-flat.vmdk`; reads return zeros |
| `mono_flat-flat.vmdk` | (raw FLAT extent for `mono_flat.vmdk`) | — | No (by design) | qemu-img 11.0.0 (macOS/ARM) | Raw 1 MiB of zeros, no VMDK header |
| `tw_sparse.vmdk` | twoGbMaxExtentSparse (descriptor) | 4 MiB | Yes (open_path only) | qemu-img 11.0.0 (macOS/ARM) | All-sparse; references `tw_sparse-s001.vmdk`; reads return zeros |
| `tw_sparse-s001.vmdk` | (SPARSE extent for `tw_sparse.vmdk`) | — | No (by design) | qemu-img 11.0.0 (macOS/ARM) | Empty sparse extent; opened via its descriptor |
| `tw_sparse_data.vmdk` | twoGbMaxExtentSparse (descriptor) | 4 MiB | Yes (open_path only) | qemu-img 11.0.0 (macOS/ARM) | References `tw_sparse_data-s001.vmdk`; grain 0 carries the `i%256` source pattern |
| `tw_sparse_data-s001.vmdk` | (SPARSE extent for `tw_sparse_data.vmdk`) | — | No (by design) | qemu-img 11.0.0 (macOS/ARM) | Real grain data (GTE[0]=128); opened via its descriptor |
| `ms3-win.vmdk` | twoGbMaxExtentSparse (descriptor only) | — | No (extents not committed) | Rapid7 Metasploitable3 (win2k8) | 1 KB descriptor referencing 16 uncommitted `disk-sNNN.vmdk` extents; `open()`/`open_path()` both return Err |

"Not supported" means `VmdkReader::open` returns `Err`, not that it panics.
These files serve as regression seeds: the reader must not panic on any of them.

## Provenance

### Real third-party corpora (committed)

- **dfvfs_ext2.vmdk** — from [log2timeline/dfvfs](https://github.com/log2timeline/dfvfs)
  `test_data/ext2.vmdk` (Apache-2.0). ext2 filesystem, VMware4 origin.
  Download: <https://github.com/log2timeline/dfvfs/raw/main/test_data/ext2.vmdk>
- **plaso_image.vmdk** — from [log2timeline/plaso](https://github.com/log2timeline/plaso)
  `test_data/image.vmdk` (Apache-2.0). VMware Workstation 4 era
  (`virtualHWVersion=4`, `adapterType=ide`), 200-sector disk with real
  filesystem data. Download:
  <https://github.com/log2timeline/plaso/raw/main/test_data/image.vmdk>
- **ms3-win.vmdk** — descriptor-only file (1 KB) from the Rapid7
  Metasploitable3 VMware Vagrant box (`virtualHWVersion=13`, built with Packer
  `vmware-iso`). References 16 × `disk-sNNN.vmdk` SPARSE extents which are not
  committed (total ~60 GB). Apache-2.0 (Metasploitable3 license).
  Source: <https://app.vagrantup.com/rapid7/boxes/metasploitable3-win2k8>
  (`vmware_desktop` provider).

### Synthetic fixtures (generated locally with qemu-img 11.0.0 on macOS / Apple Silicon)

```sh
# monolithicSparse / streamOptimized / twoGbMaxExtentFlat empty disks
qemu-img create -f vmdk tests/data/minimal.vmdk 1M
qemu-img create -f vmdk -o subformat=streamOptimized tests/data/stream_opt.vmdk 1M
qemu-img create -f vmdk -o subformat=twoGbMaxExtentFlat tests/data/flat.vmdk 1M

# monolithicFlat (descriptor + raw flat extent, 1 MiB of zeros)
qemu-img create -f vmdk -o subformat=monolithicFlat tests/data/mono_flat.vmdk 1M

# twoGbMaxExtentSparse empty disk (descriptor + tw_sparse-s001.vmdk extent)
qemu-img create -f vmdk -o subformat=twoGbMaxExtentSparse tests/data/tw_sparse.vmdk 4M

# twoGbMaxExtentSparse with real grain data — source: bytes(i%256 for i in range(4*1024*1024))
#   python3 -c "import sys; sys.stdout.buffer.write(bytes(i%256 for i in range(4*1024*1024)))" > pat4m.raw
qemu-img convert -f raw -O vmdk -o subformat=twoGbMaxExtentSparse pat4m.raw tests/data/tw_sparse_data.vmdk

# streamOptimized with one compressed allocated grain — source: bytes(i%64 for i in range(65536))
#   python3 -c "import sys; sys.stdout.buffer.write(bytes(i%64 for i in range(65536)))" > pat64k.raw
qemu-img convert -f raw -O vmdk -o subformat=streamOptimized pat64k.raw tests/data/compressed_stream_opt.vmdk
```

The `-s001` / `-flat` sibling extent files (`tw_sparse-s001.vmdk`,
`tw_sparse_data-s001.vmdk`, `mono_flat-flat.vmdk`, `flat-f001.vmdk`) are emitted
by the `qemu-img` commands above alongside their descriptors.

## External validation (not committed)

These real-world VMDKs were validated against the reader but are too large to commit:

| File | Source | Size | Virtual size | Result |
|------|--------|------|-------------|--------|
| `Ubuntu Server v11.04 64-bit-cl1.vmdk` | pWnOS v2.0, VulnHub | 1.3 GB | 40 GiB | Opens OK; GD at sector 5151; MBR boot sector read from grain |

pWnOS v2.0 download: <https://download.vulnhub.com/pwnos/pWnOS_v2.0.7z>
Validation: `cargo run -p vmdk-cli -- info "<path>"` reported `monolithicSparse`,
42,949,672,960 bytes. Grain lookup navigated GD→GT→grain at sector 10368, read
414 non-zero bytes (x86 MBR boot code).

## MD5 manifest

`tests/data/` is committed (each fixture ≲ a few MB), so these hashes are the
per-file integrity manifest.

| File | MD5 |
|------|-----|
| `compressed_stream_opt.vmdk` | `ad8af51633ddfaa8c928a04e29194de8` |
| `dfvfs_ext2.vmdk` | `bf9ea4b00b3bbe40e2670159d5a83a25` |
| `flat-f001.vmdk` | `b6d81b360a5672d80c27430f39153e2c` |
| `flat.vmdk` | `5df149dcafe6f541af365cb765d6255f` |
| `minimal.vmdk` | `a17913b3a0e118826c0535cd86fa47b8` |
| `mono_flat-flat.vmdk` | `b6d81b360a5672d80c27430f39153e2c` |
| `mono_flat.vmdk` | `79c7c3a8b475aae0dbdf04b005947ebf` |
| `ms3-win.vmdk` | `aaed1798e1516e14fb3bb61d9d7fd5cf` |
| `plaso_image.vmdk` | `be0ed84663acb454cd3fce8d249fa319` |
| `stream_opt.vmdk` | `39ced140061a52510df2d6c755d943cb` |
| `tw_sparse-s001.vmdk` | `aebb8424c5fbb11f74a0dcef78b5d96a` |
| `tw_sparse.vmdk` | `bface0694079772558a3c48b4aefdb8f` |
| `tw_sparse_data-s001.vmdk` | `b9f44a917d6efeb68db4013cd1df31e9` |
| `tw_sparse_data.vmdk` | `b0c6660578fa1e4beddfdc92d066e2dd` |
