# Virtual Disk Image parser
This crate provides support for reading VirtualBox Virtual Disk Images (VDI).

This vendored LovelyCarve fork is read-only and supports VDI 1.1 dynamic and
fixed images. It adds checked little-endian header parsing, sparse/zero block
handling, `block_extra` support, guest-size bounds, and thread-safe positioned
reads. Differencing and undo images remain unsupported because their parent
chains cannot be resolved from one file alone.

Opened VDI files can be read using the std Read/Seek traits (writing is not supported (yet)).
Additionally, `VdiDisk` implements `ReadAt` from [positioned-io2](https://crates.io/crates/positioned-io2)

## Example
```rs
let file = File::open(&path)?;
let mut disk = VdiDisk::open(file)?;
println!("VDI header: {:X?}", disk.header);

let partitions = bootsector::list_partitions(&mut disk, &bootsector::Options::default())?;
```
