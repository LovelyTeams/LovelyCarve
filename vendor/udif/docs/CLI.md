# CLI Tool

The `udif-tool` example provides a command-line interface for working with DMG files.

## Building

```bash
cargo build --example udif-tool --release
```

## Commands

### info

Display DMG metadata and statistics.

```bash
udif-tool info <dmg-file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- info image.dmg

DMG Information: image.dmg
============================================================

Header:
  Version:          4
  Sector count:     2351263
  Data fork length: 1023271318 bytes
  Segment:          1/1

Size:
  Uncompressed:     1203845632 bytes (1148.08 MB)
  Compressed:       1023271318 bytes (975.87 MB)
  Compression:      15.0%

Partitions:         7

Block types used:
  LZFSE:            988 blocks
```

### list

List all partitions in the DMG.

```bash
udif-tool list <dmg-file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- list image.dmg

Partitions in image.dmg:
================================================================================
  ID       Sectors          Size     Ratio  Name
--------------------------------------------------------------------------------
  -1             1         512 B     89.6%  Protective Master Boot Record (MBR : 0)
   0             1         512 B     78.9%  GPT Header (Primary GPT Header : 1)
   1            32      16.00 KB     98.7%  GPT Partition Data (Primary GPT Table : 2)
   2       2089050    1020.04 MB      4.3%  Untitled 1 (Apple_HFSX : 3)
   3        262144     128.00 MB    100.0%   (Apple_Free : 4)
   4            32      16.00 KB     98.7%  GPT Partition Data (Backup GPT Table : 5)
   5             1         512 B     78.9%  GPT Header (Backup GPT Header : 6)
```

### extract

Extract the main HFS+/APFS partition to a raw disk image.

```bash
udif-tool extract <dmg-file> <output-file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- extract image.dmg partition.raw

Opening image.dmg...
Extracting main partition...
Writing 1069593600 bytes to partition.raw...
Done!
```

### extract-all

Extract the complete disk image (all partitions).

```bash
udif-tool extract-all <dmg-file> <output-file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- extract-all image.dmg full_disk.raw
```

### extract-partition

Extract a specific partition by ID.

```bash
udif-tool extract-partition <dmg-file> <partition-id> <output-file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- extract-partition image.dmg 2 hfsx.raw

Opening image.dmg...
Extracting partition 2...
Writing 1069593600 bytes to hfsx.raw...
Done!
```

### create

Create a DMG from a raw disk image.

```bash
udif-tool create <output.dmg> <input-file> [--compression <method>]
```

**Compression methods:**
- `raw` - No compression
- `zlib` - Zlib compression (default)
- `bzip2` - Bzip2 compression
- `lzfse` - LZFSE compression

**Examples:**

```bash
# Default (zlib)
$ cargo run --example udif-tool -- create output.dmg input.raw

# With LZFSE
$ cargo run --example udif-tool -- create output.dmg input.raw --compression lzfse

# No compression
$ cargo run --example udif-tool -- create output.dmg input.raw --compression raw
```

### check

Check if a file is a valid DMG.

```bash
udif-tool check <file>
```

**Example:**

```bash
$ cargo run --example udif-tool -- check image.dmg
image.dmg is a valid DMG file

$ cargo run --example udif-tool -- check random.bin
random.bin is NOT a valid DMG file
```

## Usage Examples

### Extract and Access HFS+ Partition

First, extract the partition:

```bash
cargo run --example udif-tool --release -- extract image.dmg partition.raw
```

Then mount/access it based on your platform:

#### macOS

```bash
# Attach the raw disk image
hdiutil attach -imagekey diskimage-class=CRawDiskImage partition.raw

# Or create a DMG from raw and open it
hdiutil convert partition.raw -format UDZO -o converted.dmg
open converted.dmg
```

#### Linux

```bash
# Install HFS+ support
sudo apt install hfsprogs    # Debian/Ubuntu
sudo dnf install hfsplus-tools  # Fedora

# Mount the partition (read-only recommended)
sudo mount -t hfsplus -o ro,loop partition.raw /mnt/hfs

# For read-write (use with caution)
sudo mount -t hfsplus -o loop,force partition.raw /mnt/hfs
```

#### Windows

Windows doesn't natively support HFS+. Options:

| Tool | Type | Notes |
|------|------|-------|
| [HFSExplorer](https://www.catacombae.org/hfsexplorer/) | Free | Read-only, Java-based GUI |
| [Paragon HFS+](https://www.paragon-software.com/home/hfs-windows/) | Paid | Full read-write support |
| [TransMac](https://www.acutesystems.com/scrtm.htm) | Paid | Can also create DMGs |

With HFSExplorer (free):
```
1. Download and install HFSExplorer
2. File → Load file system from file
3. Select partition.raw
4. Browse and extract files
```

### Convert DMG to raw disk image

```bash
cargo run --example udif-tool --release -- extract-all installer.dmg installer.raw
```

### Inspect Kernel Debug Kit

```bash
# Get info
cargo run --example udif-tool -- info Kernel_Debug_Kit_*.dmg

# List partitions
cargo run --example udif-tool -- list Kernel_Debug_Kit_*.dmg

# Extract HFSX partition (usually ID 2)
cargo run --example udif-tool -- extract-partition Kernel_Debug_Kit_*.dmg 2 kdk.raw
```

### Create compressed DMG

```bash
# Create from raw disk image with LZFSE
cargo run --example udif-tool --release -- create backup.dmg disk.raw -c lzfse
```
