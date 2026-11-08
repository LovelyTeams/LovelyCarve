//! Minimal valid sparse VMDK builder for use in tests and downstream crates.

use std::path::Path;

use super::header::{MAGIC, SECTOR_SIZE, VERSION, VERSION_STREAM_OPT};

// Layout constants (all in sectors unless noted):
const DESCRIPTOR_OFFSET: u64 = 1;
const DESCRIPTOR_SECTORS: u64 = 20;
const GD_SECTOR: u64 = DESCRIPTOR_OFFSET + DESCRIPTOR_SECTORS; // 21
const RGD_SECTOR: u64 = GD_SECTOR + 1; // 22
const GT_SECTOR: u64 = RGD_SECTOR + 1; // 23
const GT_SECTORS: u64 = 4; // 512 GTEs × 4 B = 2048 B
const GRAIN_SECTOR: u64 = GT_SECTOR + GT_SECTORS; // 27

pub const GRAIN_SIZE_SECTORS: u64 = 8;
pub const GRAIN_SIZE_BYTES: usize = GRAIN_SIZE_SECTORS as usize * SECTOR_SIZE as usize;
const NUM_GTES_PER_GT: u32 = 512;

/// Build a minimal valid monolithic sparse VMDK containing `sector_data` in grain 0.
///
/// `sector_data` is zero-padded or truncated to [`GRAIN_SIZE_BYTES`].
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn test_sparse_vmdk(sector_data: &[u8]) -> Vec<u8> {
    // ── Grain data ────────────────────────────────────────────────────────────
    let mut grain = vec![0u8; GRAIN_SIZE_BYTES];
    let copy_len = sector_data.len().min(GRAIN_SIZE_BYTES);
    grain[..copy_len].copy_from_slice(&sector_data[..copy_len]);

    // ── Header (512 bytes) ────────────────────────────────────────────────────
    let mut hdr = vec![0u8; 512];
    hdr[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    hdr[4..8].copy_from_slice(&VERSION.to_le_bytes());
    hdr[8..12].copy_from_slice(&0u32.to_le_bytes()); // flags
    hdr[12..20].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes()); // capacity = 1 grain
    hdr[20..28].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes()); // grainSize
    hdr[28..36].copy_from_slice(&DESCRIPTOR_OFFSET.to_le_bytes());
    hdr[36..44].copy_from_slice(&DESCRIPTOR_SECTORS.to_le_bytes());
    hdr[44..48].copy_from_slice(&NUM_GTES_PER_GT.to_le_bytes());
    hdr[48..56].copy_from_slice(&RGD_SECTOR.to_le_bytes()); // rgdOffset
    hdr[56..64].copy_from_slice(&GD_SECTOR.to_le_bytes()); // gdOffset
    hdr[64..72].copy_from_slice(&GRAIN_SECTOR.to_le_bytes()); // overHead
    hdr[72] = 0; // uncleanShutdown
    hdr[73] = b'\n';
    hdr[74] = b' ';
    hdr[75] = b'\r';
    hdr[76] = b'\n';
    hdr[77..79].copy_from_slice(&0u16.to_le_bytes()); // compressAlgorithm = 0

    // ── Descriptor (20 sectors) ───────────────────────────────────────────────
    let mut desc = vec![0u8; DESCRIPTOR_SECTORS as usize * SECTOR_SIZE as usize];
    let s = "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n";
    let n = s.len().min(desc.len());
    desc[..n].copy_from_slice(&s.as_bytes()[..n]);

    // ── Grain Directory (1 sector, first entry → GT) ──────────────────────────
    let mut gd = vec![0u8; SECTOR_SIZE as usize];
    gd[0..4].copy_from_slice(&(GT_SECTOR as u32).to_le_bytes());
    let rgd = gd.clone(); // redundant GD

    // ── Grain Table (4 sectors, first GTE → grain data) ──────────────────────
    let mut gt = vec![0u8; GT_SECTORS as usize * SECTOR_SIZE as usize];
    gt[0..4].copy_from_slice(&(GRAIN_SECTOR as u32).to_le_bytes());

    // ── Assemble ──────────────────────────────────────────────────────────────
    let mut vmdk = Vec::new();
    vmdk.extend_from_slice(&hdr);
    vmdk.extend_from_slice(&desc);
    vmdk.extend_from_slice(&gd);
    vmdk.extend_from_slice(&rgd);
    vmdk.extend_from_slice(&gt);
    vmdk.extend_from_slice(&grain);
    vmdk
}

/// Build a 2-grain monolithic sparse VMDK where **grain 0 is sparse** and
/// **grain 1 holds `grain1_data`**. Used to test that a read spanning both
/// grains does not let the sparse grain 0 zero-mask the allocated grain 1.
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn test_sparse_vmdk_sparse_then_allocated(grain1_data: &[u8]) -> Vec<u8> {
    let mut grain = vec![0u8; GRAIN_SIZE_BYTES];
    let copy_len = grain1_data.len().min(GRAIN_SIZE_BYTES);
    grain[..copy_len].copy_from_slice(&grain1_data[..copy_len]);

    let mut hdr = vec![0u8; 512];
    hdr[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    hdr[4..8].copy_from_slice(&VERSION.to_le_bytes());
    hdr[12..20].copy_from_slice(&(2 * GRAIN_SIZE_SECTORS).to_le_bytes()); // capacity = 2 grains
    hdr[20..28].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes()); // grainSize
    hdr[28..36].copy_from_slice(&DESCRIPTOR_OFFSET.to_le_bytes());
    hdr[36..44].copy_from_slice(&DESCRIPTOR_SECTORS.to_le_bytes());
    hdr[44..48].copy_from_slice(&NUM_GTES_PER_GT.to_le_bytes());
    hdr[48..56].copy_from_slice(&RGD_SECTOR.to_le_bytes());
    hdr[56..64].copy_from_slice(&GD_SECTOR.to_le_bytes());
    hdr[64..72].copy_from_slice(&GRAIN_SECTOR.to_le_bytes());
    hdr[73] = b'\n';
    hdr[74] = b' ';
    hdr[75] = b'\r';
    hdr[76] = b'\n';

    let mut desc = vec![0u8; DESCRIPTOR_SECTORS as usize * SECTOR_SIZE as usize];
    let s = "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n";
    let n = s.len().min(desc.len());
    desc[..n].copy_from_slice(&s.as_bytes()[..n]);

    let mut gd = vec![0u8; SECTOR_SIZE as usize];
    gd[0..4].copy_from_slice(&(GT_SECTOR as u32).to_le_bytes());
    let rgd = gd.clone();

    // GTE[0] = 0 (grain 0 sparse); GTE[1] → the single grain (grain 1 allocated).
    let mut gt = vec![0u8; GT_SECTORS as usize * SECTOR_SIZE as usize];
    gt[4..8].copy_from_slice(&(GRAIN_SECTOR as u32).to_le_bytes());

    let mut vmdk = Vec::new();
    vmdk.extend_from_slice(&hdr);
    vmdk.extend_from_slice(&desc);
    vmdk.extend_from_slice(&gd);
    vmdk.extend_from_slice(&rgd);
    vmdk.extend_from_slice(&gt);
    vmdk.extend_from_slice(&grain);
    vmdk
}

// ── seSparse test helpers ─────────────────────────────────────────────────────

/// Build a minimal seSparse extent file with grain 0 containing `sector_data`.
///
/// Layout:
/// Sector 0:    constant header (magic CAFEBABE, version, capacity, etc.)
/// Sector 1:    volatile header (magic CAFECAFE) — unused but structurally required
/// Sectors 2-9: padding to sector 10 (`gd_offset`)
/// Sector 10:   GD (one u64 entry = 1, pointing to GT table index 1)
/// Sectors 11-74: GT (index 1 = sectors 11 to 74; 64 sectors = 4096 × 8-byte GTEs)
///              `GTE[0]` = sector 75 (grain data)
/// Sector 75:   grain data (8 sectors = 4 KiB)
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn test_sesparse_vmdk(sector_data: &[u8]) -> Vec<u8> {
    use super::sesparse::{SE_CONST_MAGIC, SE_GTES_PER_GT, SE_GT_SECTORS, SE_VERSION};

    const GRAIN_SECTORS: u64 = 8;
    const GRAIN_BYTES: usize = GRAIN_SECTORS as usize * SECTOR_SIZE as usize;
    // Layout (QEMU-compatible — see sesparse-encoding memory):
    //   sector 0      : const header
    //   sector 1      : volatile header (magic CAFECAFE)
    //   sector 2      : grain directory (GD)
    //   sectors 3..66 : grain table 0 (64 sectors)
    //   sectors 67..74: grain 0 data (8 sectors)
    const VOL_SECTOR: u64 = 1;
    const GD_SECTOR: u64 = 2;
    const GT_OFFSET: u64 = 3;
    const GRAIN_SECTOR: u64 = GT_OFFSET + SE_GT_SECTORS; // = 67
    const CAPACITY: u64 = GRAIN_SECTORS; // 1 grain

    // seSparse grain-entry encoding (top nibble = type):
    //   GD entry: high32 must be 0x10000000, low32 = grain-table index
    //   GT entry (allocated): 0x3 nibble + bit-rotated grain index
    const GD_ALLOCATED: u64 = 0x1000_0000_0000_0000; // table index 0
    const GT_ALLOCATED_GRAIN0: u64 = 0x3000_0000_0000_0000; // grain index 0

    let mut grain = vec![0u8; GRAIN_BYTES];
    let copy_len = sector_data.len().min(GRAIN_BYTES);
    grain[..copy_len].copy_from_slice(&sector_data[..copy_len]);

    // Constant header (512 bytes). flags / reserved / pad must stay zero (QEMU checks).
    let mut const_hdr = vec![0u8; 512];
    const_hdr[0..8].copy_from_slice(&SE_CONST_MAGIC.to_le_bytes());
    const_hdr[8..16].copy_from_slice(&SE_VERSION.to_le_bytes());
    const_hdr[16..24].copy_from_slice(&CAPACITY.to_le_bytes());
    const_hdr[24..32].copy_from_slice(&GRAIN_SECTORS.to_le_bytes()); // grain_size
    const_hdr[32..40].copy_from_slice(&SE_GT_SECTORS.to_le_bytes()); // grain_table_size
    const_hdr[80..88].copy_from_slice(&VOL_SECTOR.to_le_bytes()); // volatile hdr offset
    const_hdr[88..96].copy_from_slice(&1u64.to_le_bytes()); // volatile hdr size
    const_hdr[128..136].copy_from_slice(&GD_SECTOR.to_le_bytes()); // grain_dir_offset
    const_hdr[136..144].copy_from_slice(&1u64.to_le_bytes()); // grain_dir_size
    const_hdr[144..152].copy_from_slice(&GT_OFFSET.to_le_bytes()); // grain_tables_offset
    const_hdr[152..160].copy_from_slice(&SE_GT_SECTORS.to_le_bytes()); // grain_tables_size
    const_hdr[192..200].copy_from_slice(&GRAIN_SECTOR.to_le_bytes()); // grains_offset
    const_hdr[200..208].copy_from_slice(&GRAIN_SECTORS.to_le_bytes()); // grains_size

    // Volatile header (sector 1): magic CAFECAFE, replay_journal=0, pad=0.
    let mut vol_hdr = vec![0u8; 512];
    vol_hdr[0..8].copy_from_slice(&0x0000_0000_CAFE_CAFEu64.to_le_bytes());

    // Grain directory (sector 2): GD[0] points to grain-table index 0, allocated.
    let mut gd = vec![0u8; SECTOR_SIZE as usize];
    gd[0..8].copy_from_slice(&GD_ALLOCATED.to_le_bytes());

    // Grain table 0 (sectors 3..66): GTE[0] = allocated grain index 0.
    let mut gt = vec![0u8; SE_GTES_PER_GT as usize * 8];
    gt[0..8].copy_from_slice(&GT_ALLOCATED_GRAIN0.to_le_bytes());

    let mut vmdk = Vec::new();
    vmdk.extend_from_slice(&const_hdr); // sector 0
    vmdk.extend_from_slice(&vol_hdr); // sector 1
    vmdk.extend_from_slice(&gd); // sector 2
    vmdk.extend_from_slice(&gt); // sectors 3..66
    vmdk.extend_from_slice(&grain); // sectors 67..74
    vmdk
}

/// Build a monolithicSparse VMDK with a custom descriptor string embedded.
///
/// Used to construct snapshot chains with `parentCID` and `parentFileNameHint`.
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn test_sparse_vmdk_with_descriptor(sector_data: &[u8], descriptor_text: &str) -> Vec<u8> {
    let mut grain = vec![0u8; GRAIN_SIZE_BYTES];
    let copy_len = sector_data.len().min(GRAIN_SIZE_BYTES);
    grain[..copy_len].copy_from_slice(&sector_data[..copy_len]);

    let mut hdr = vec![0u8; 512];
    hdr[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    hdr[4..8].copy_from_slice(&VERSION.to_le_bytes());
    hdr[12..20].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes());
    hdr[20..28].copy_from_slice(&GRAIN_SIZE_SECTORS.to_le_bytes());
    hdr[28..36].copy_from_slice(&DESCRIPTOR_OFFSET.to_le_bytes());
    hdr[36..44].copy_from_slice(&DESCRIPTOR_SECTORS.to_le_bytes());
    hdr[44..48].copy_from_slice(&NUM_GTES_PER_GT.to_le_bytes());
    hdr[48..56].copy_from_slice(&RGD_SECTOR.to_le_bytes());
    hdr[56..64].copy_from_slice(&GD_SECTOR.to_le_bytes());
    hdr[64..72].copy_from_slice(&GRAIN_SECTOR.to_le_bytes());
    hdr[72] = 0;
    hdr[73] = b'\n';
    hdr[74] = b' ';
    hdr[75] = b'\r';
    hdr[76] = b'\n';
    hdr[77..79].copy_from_slice(&0u16.to_le_bytes());

    let mut desc = vec![0u8; DESCRIPTOR_SECTORS as usize * SECTOR_SIZE as usize];
    let n = descriptor_text.len().min(desc.len());
    desc[..n].copy_from_slice(&descriptor_text.as_bytes()[..n]);

    let mut gd = vec![0u8; SECTOR_SIZE as usize];
    gd[0..4].copy_from_slice(&(GT_SECTOR as u32).to_le_bytes());
    let rgd = gd.clone();

    let mut gt = vec![0u8; GT_SECTORS as usize * SECTOR_SIZE as usize];
    gt[0..4].copy_from_slice(&(GRAIN_SECTOR as u32).to_le_bytes());

    let mut vmdk = Vec::new();
    vmdk.extend_from_slice(&hdr);
    vmdk.extend_from_slice(&desc);
    vmdk.extend_from_slice(&gd);
    vmdk.extend_from_slice(&rgd);
    vmdk.extend_from_slice(&gt);
    vmdk.extend_from_slice(&grain);
    vmdk
}

/// Write a base VMDK and delta VMDK to `dir`, returning `(base_path, delta_path)`.
///
/// The base has `CID=00000001` (grain 0 = `base_data`); the delta has `CID=00000002`,
/// `parentCID=00000001`, `parentFileNameHint="base.vmdk"` (grain 0 sparse → read from base).
///
/// Reading grain 0 from the chain should yield `base_data`.
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn write_chain_to_dir(
    dir: &Path,
    base_data: &[u8],
) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::io::Write as _;

    let base_desc = "# Disk DescriptorFile\nversion=1\nCID=00000001\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n";
    let base_bytes = test_sparse_vmdk_with_descriptor(base_data, base_desc);
    let base_path = dir.join("base.vmdk");
    std::fs::File::create(&base_path)
        .expect("create base.vmdk")
        .write_all(&base_bytes)
        .expect("write base.vmdk");

    // Delta has grain 0 sparse (all-zeros grain table) so reads fall through to base.
    let delta_desc = "# Disk DescriptorFile\nversion=1\nCID=00000002\nparentCID=00000001\nparentFileNameHint=\"base.vmdk\"\ncreateType=\"monolithicSparse\"\n";
    // Build a delta where grain 0 is sparse (GTE=0).
    let mut delta_bytes = test_sparse_vmdk_with_descriptor(&[], delta_desc);
    // Patch GT[0] to 0 (sparse) — already zero by default in test_sparse_vmdk_with_descriptor
    // since sector_data is empty. But test_sparse_vmdk_with_descriptor always sets GTE[0]=GRAIN_SECTOR.
    // We need to zero out GTE[0] so it's sparse in the delta.
    let gt_offset = (GT_SECTOR as usize) * SECTOR_SIZE as usize;
    delta_bytes[gt_offset..gt_offset + 4].copy_from_slice(&0u32.to_le_bytes());
    let delta_path = dir.join("delta.vmdk");
    std::fs::File::create(&delta_path)
        .expect("create delta.vmdk")
        .write_all(&delta_bytes)
        .expect("write delta.vmdk");

    (base_path, delta_path)
}

// ── COWD test helpers ─────────────────────────────────────────────────────────

/// Build a minimal COWD extent file with grain 0 containing `sector_data`.
///
/// Layout: header (sector 0) | padding (sectors 1-3) | GD (sector 4) | GT (sector 5) | grain
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn test_cowd_vmdk(sector_data: &[u8]) -> Vec<u8> {
    const COWD_MAGIC: u32 = 0x434F_5744;
    const GRAIN_SIZE: u32 = 8; // 4 KiB grains
    const GTES_PER_GT: u32 = 4096;
    const GD_SECTOR: u32 = 4;
    const GT_SECTOR: u32 = 5;
    // Grain data starts at sector 5 + ceil(GTES_PER_GT*4/512) = 5 + 32 = 37
    const GRAIN_SECTOR: u32 = GT_SECTOR + GTES_PER_GT * 4 / 512; // = 37

    let grain_size_bytes = GRAIN_SIZE as usize * SECTOR_SIZE as usize;
    let mut grain = vec![0u8; grain_size_bytes];
    let copy_len = sector_data.len().min(grain_size_bytes);
    grain[..copy_len].copy_from_slice(&sector_data[..copy_len]);

    // Header: 512 bytes
    let mut hdr = vec![0u8; 512];
    hdr[0..4].copy_from_slice(&COWD_MAGIC.to_be_bytes()); // big-endian magic
    hdr[4..8].copy_from_slice(&1u32.to_le_bytes()); // version = 1
    hdr[8..12].copy_from_slice(&3u32.to_le_bytes()); // flags
    hdr[12..16].copy_from_slice(&GRAIN_SIZE.to_le_bytes()); // capacity = 1 grain
    hdr[16..20].copy_from_slice(&GRAIN_SIZE.to_le_bytes()); // grain_size
    hdr[20..24].copy_from_slice(&GD_SECTOR.to_le_bytes()); // GD sector
    hdr[24..28].copy_from_slice(&1u32.to_le_bytes()); // gd_entries = 1
    hdr[28..32].copy_from_slice(&(GRAIN_SECTOR + GRAIN_SIZE).to_le_bytes()); // next_free

    // Sectors 1-3: padding
    let padding = vec![0u8; 3 * SECTOR_SIZE as usize];

    // Sector 4: GD — one entry pointing to GT at sector 5
    let mut gd = vec![0u8; SECTOR_SIZE as usize];
    gd[0..4].copy_from_slice(&GT_SECTOR.to_le_bytes());

    // Sectors 5-36: GT (4096 × 4 bytes = 16384 bytes = 32 sectors), GTE[0] → grain
    let mut gt = vec![0u8; GTES_PER_GT as usize * 4];
    gt[0..4].copy_from_slice(&GRAIN_SECTOR.to_le_bytes());

    let mut cowd = Vec::new();
    cowd.extend_from_slice(&hdr);
    cowd.extend_from_slice(&padding);
    cowd.extend_from_slice(&gd);
    cowd.extend_from_slice(&gt);
    cowd.extend_from_slice(&grain);
    cowd
}

// ── streamOptimized GD_AT_END layout constants ────────────────────────────────
// Sector 0       : primary header (gdOffset = u64::MAX sentinel)
// Sectors 1–20   : descriptor (createType="streamOptimized")
// Sectors 21–24  : GT (512 GTEs, all zero → all-sparse)
// Sector  25      : GD (1 entry → GT sector 21)
// Sector  26      : footer header (real gdOffset = 25)
// Sector  27      : EOS marker (all zeros)
// Total: 28 sectors = 14 336 bytes; 1 MiB virtual disk
const GAE_CAPACITY: u64 = 2048; // 1 MiB in sectors
const GAE_GRAIN_SIZE: u64 = 128; // 64 KiB grain
const GAE_NUM_GTES: u32 = 512;
const GAE_DESC_OFFSET: u64 = 1;
const GAE_DESC_SIZE: u64 = 20;
const GAE_GT_SECTOR: u64 = 21;
const GAE_GD_SECTOR: u64 = 25; // GAE_GT_SECTOR + 4 GT sectors
const GAE_TOTAL_SECTORS: u64 = 28;

// Writes a streamOptimized `SparseExtentHeader` into `h`, varying only `gd_off`.
fn write_stream_opt_hdr(h: &mut [u8; 512], gd_off: u64) {
    h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    h[4..8].copy_from_slice(&VERSION_STREAM_OPT.to_le_bytes());
    h[8..12].copy_from_slice(&0u32.to_le_bytes()); // flags
    h[12..20].copy_from_slice(&GAE_CAPACITY.to_le_bytes());
    h[20..28].copy_from_slice(&GAE_GRAIN_SIZE.to_le_bytes());
    h[28..36].copy_from_slice(&GAE_DESC_OFFSET.to_le_bytes());
    h[36..44].copy_from_slice(&GAE_DESC_SIZE.to_le_bytes());
    h[44..48].copy_from_slice(&GAE_NUM_GTES.to_le_bytes());
    h[48..56].copy_from_slice(&0u64.to_le_bytes()); // rgdOffset = 0
    h[56..64].copy_from_slice(&gd_off.to_le_bytes());
    h[64..72].copy_from_slice(&GAE_GD_SECTOR.to_le_bytes()); // overHead
    h[72] = 0; // uncleanShutdown
    h[73] = b'\n';
    h[74] = b' ';
    h[75] = b'\r';
    h[76] = b'\n';
    h[77..79].copy_from_slice(&1u16.to_le_bytes()); // compressAlgorithm = 1
}

// ── streamOptimized with crafted GrainMarker (fuzz-defense helper) ───────────
// Layout:
// Sector 0    : v3 header (compress=1, capacity=128, grain_size=128, gd_offset=26)
//               descriptor_offset=0 (no embedded descriptor)
// Sector 26   : GD[0] = 27
// Sector 27   : GT[0] = 128
// Sector 128  : GrainMarker { lba=0 (8 B), data_size (4 B) }  — no payload follows
const COM_CAPACITY: u64 = 128; // virtual disk sectors (64 KiB)
const COM_GRAIN_SIZE: u64 = 128; // grain_size sectors   (64 KiB)
const COM_NUM_GTES: u32 = 512;
const COM_GD_SECTOR: u64 = 26;
const COM_GT_SECTOR: u64 = 27;
const COM_GRAIN_SECTOR: u64 = 128;
const COM_MARKER_BYTES: usize = 12; // 8-byte LBA + 4-byte dataSize

/// Build a streamOptimized VMDK with `GTE[0]` pointing to a `GrainMarker` whose
/// `data_size` field is set to `marker_data_size`.
///
/// No compressed payload is present after the 12-byte marker.  Any attempt to
/// read `marker_data_size` bytes will hit EOF — the cap check must fire first.
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn compressed_vmdk_with_oversized_marker(marker_data_size: u32) -> Vec<u8> {
    let total = COM_GRAIN_SECTOR as usize * SECTOR_SIZE as usize + COM_MARKER_BYTES;
    let mut vmdk = vec![0u8; total];

    // Sector 0: streamOptimized header.
    {
        let h = &mut vmdk[0..512];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&VERSION_STREAM_OPT.to_le_bytes());
        h[12..20].copy_from_slice(&COM_CAPACITY.to_le_bytes());
        h[20..28].copy_from_slice(&COM_GRAIN_SIZE.to_le_bytes());
        // descriptor_offset = 0, descriptor_size = 0 (leave as zeros)
        h[44..48].copy_from_slice(&COM_NUM_GTES.to_le_bytes());
        // rgd_offset = 0
        h[56..64].copy_from_slice(&COM_GD_SECTOR.to_le_bytes()); // gd_offset = 26
        h[64..72].copy_from_slice(&COM_GRAIN_SECTOR.to_le_bytes()); // overHead
        h[73] = b'\n';
        h[74] = b' ';
        h[75] = b'\r';
        h[76] = b'\n';
        h[77..79].copy_from_slice(&1u16.to_le_bytes()); // compress_algorithm = 1
    }

    // Sector 26: GD[0] → GT at sector 27.
    let gd = COM_GD_SECTOR as usize * SECTOR_SIZE as usize;
    vmdk[gd..gd + 4].copy_from_slice(&(COM_GT_SECTOR as u32).to_le_bytes());

    // Sector 27: GT[0] → grain at sector 128.
    let gt = COM_GT_SECTOR as usize * SECTOR_SIZE as usize;
    vmdk[gt..gt + 4].copy_from_slice(&(COM_GRAIN_SECTOR as u32).to_le_bytes());

    // Sector 128: GrainMarker — lba=0, data_size=<param>.
    let marker = COM_GRAIN_SECTOR as usize * SECTOR_SIZE as usize;
    // lba bytes 0–7 already zero
    vmdk[marker + 8..marker + 12].copy_from_slice(&marker_data_size.to_le_bytes());

    vmdk
}

/// Build a streamOptimized VMDK whose single grain's zlib payload decompresses
/// to `decompressed_len` bytes. When that exceeds the 64 KiB grain size it is a
/// decompression bomb: a correct reader must refuse it rather than allocate the
/// full expansion. The compressed payload (zlib of zeros) is tiny, so it clears
/// any compressed-size cap and the defense must fire on the *decompressed* side.
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn compressed_vmdk_with_bomb_grain(decompressed_len: usize) -> Vec<u8> {
    use std::io::Write as _;
    let payload = {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&vec![0u8; decompressed_len])
            .expect("compress");
        enc.finish().expect("finish")
    };

    let marker = COM_GRAIN_SECTOR as usize * SECTOR_SIZE as usize;
    let total = marker + COM_MARKER_BYTES + payload.len();
    let mut vmdk = vec![0u8; total];

    {
        let h = &mut vmdk[0..512];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&VERSION_STREAM_OPT.to_le_bytes());
        h[12..20].copy_from_slice(&COM_CAPACITY.to_le_bytes());
        h[20..28].copy_from_slice(&COM_GRAIN_SIZE.to_le_bytes());
        h[44..48].copy_from_slice(&COM_NUM_GTES.to_le_bytes());
        h[56..64].copy_from_slice(&COM_GD_SECTOR.to_le_bytes());
        h[64..72].copy_from_slice(&COM_GRAIN_SECTOR.to_le_bytes());
        h[73] = b'\n';
        h[74] = b' ';
        h[75] = b'\r';
        h[76] = b'\n';
        h[77..79].copy_from_slice(&1u16.to_le_bytes());
    }

    let gd = COM_GD_SECTOR as usize * SECTOR_SIZE as usize;
    vmdk[gd..gd + 4].copy_from_slice(&(COM_GT_SECTOR as u32).to_le_bytes());
    let gt = COM_GT_SECTOR as usize * SECTOR_SIZE as usize;
    vmdk[gt..gt + 4].copy_from_slice(&(COM_GRAIN_SECTOR as u32).to_le_bytes());

    // GrainMarker: lba=0, data_size = compressed length; the zlib payload follows.
    vmdk[marker + 8..marker + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    vmdk[marker + COM_MARKER_BYTES..marker + COM_MARKER_BYTES + payload.len()]
        .copy_from_slice(&payload);

    vmdk
}

/// Build a streamOptimized VMDK where the primary header carries `GD_AT_END`
/// (`gdOffset = u64::MAX`) and the real GD is referenced by the footer header
/// pinned at `file_end − 1024`.
///
/// Virtual size is 1 MiB, all grains are sparse (reads return zeros).
#[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
pub fn gd_at_end_stream_opt_vmdk() -> Vec<u8> {
    let total_bytes = GAE_TOTAL_SECTORS * SECTOR_SIZE;
    let mut vmdk = vec![0u8; total_bytes as usize];

    // Sector 0: primary header with GD_AT_END sentinel.
    let mut hdr = [0u8; 512];
    write_stream_opt_hdr(&mut hdr, u64::MAX);
    vmdk[0..512].copy_from_slice(&hdr);

    // Sectors 1–20: descriptor.
    let desc = b"# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"streamOptimized\"\n";
    let desc_start = GAE_DESC_OFFSET as usize * SECTOR_SIZE as usize;
    let copy_len = desc
        .len()
        .min(GAE_DESC_SIZE as usize * SECTOR_SIZE as usize);
    vmdk[desc_start..desc_start + copy_len].copy_from_slice(&desc[..copy_len]);

    // Sectors 21–24: GT (all zeros → all-sparse; already zeroed).

    // Sector 25: GD — single entry pointing to GT at sector 21.
    let gd_start = GAE_GD_SECTOR as usize * SECTOR_SIZE as usize;
    vmdk[gd_start..gd_start + 4].copy_from_slice(&(GAE_GT_SECTOR as u32).to_le_bytes());

    // Sector 26: footer header with real gdOffset = 25.
    let footer_start = (GAE_TOTAL_SECTORS - 2) as usize * SECTOR_SIZE as usize;
    let mut footer = [0u8; 512];
    write_stream_opt_hdr(&mut footer, GAE_GD_SECTOR);
    vmdk[footer_start..footer_start + 512].copy_from_slice(&footer);

    // Sector 27: EOS marker (already all zeros).

    vmdk
}
