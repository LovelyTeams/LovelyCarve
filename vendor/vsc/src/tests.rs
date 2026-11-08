//! Synthetic (Tier-3) unit fixtures for the VSS reader.
//!
//! These build a minimal valid VSS byte image in-code and assert the parser
//! round-trips known values. They prove parser MECHANICS only — the authoritative
//! correctness check is the Tier-1 env-gated oracle test in
//! `tests/oracle_dfvfs.rs`, which validates against existing `pyvshadow` hashes on a real
//! disk image.

use std::io::Cursor;

use crate::block::{BlockDescriptor, BlockDescriptorFlags, StoreBlockRange};
use crate::bytes::utf16le_string;
use crate::catalog::{
    catalog_next_block_offset, is_catalog_block, parse_catalog_entry, CatalogEntry,
    StoreDescriptor, VolumeHeader, BLOCK_SIZE, CATALOG_ENTRY_LEN,
};
use crate::guid::{format_guid, VSS_IDENTIFIER};
use crate::store::{AttributeFlags, StoreBlockHeader, StoreInfo};
use crate::{VssError, VssVolume};

#[path = "parity_tests.rs"]
mod parity;

// ---------------------------------------------------------------------------
// Known synthetic values (Tier-3: we author both fixture and expected answer).
// ---------------------------------------------------------------------------

/// On-disk (mixed-endian) bytes of GUID `11223344-5566-7788-99aa-bbccddeeff00`.
const STORE_ID: [u8; 16] = [
    0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
];
const STORE_ID_STR: &str = "11223344-5566-7788-99aa-bbccddeeff00";

/// On-disk bytes of GUID `aaaabbbb-cccc-dddd-eeee-ffff00001111`.
const SHADOW_ID: [u8; 16] = [
    0xbb, 0xbb, 0xaa, 0xaa, 0xcc, 0xcc, 0xdd, 0xdd, 0xee, 0xee, 0xff, 0xff, 0x00, 0x00, 0x11, 0x11,
];
const SHADOW_ID_STR: &str = "aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// On-disk bytes of GUID `22223333-4444-5555-6666-777788889999`.
const SET_ID: [u8; 16] = [
    0x33, 0x33, 0x22, 0x22, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77, 0x88, 0x88, 0x99, 0x99,
];
const SET_ID_STR: &str = "22223333-4444-5555-6666-777788889999";

const VOL_SIZE: u64 = 255_136_931_328;
const SEQ: u64 = 1;
const FLAGS: u64 = 0x40;
const CTIME: u64 = 130_000_000_000_000_000;
const CTX: u32 = 0x0000_0001;
const ATTR: u32 =
    AttributeFlags::PERSISTENT | AttributeFlags::CLIENT_ACCESSIBLE | AttributeFlags::DIFFERENTIAL;

const CATALOG_OFF: u64 = 0x4000;
const STORE_HDR_OFF: u64 = 0x8000;
const STORE_BITMAP_OFF: u64 = 0xC000;
/// Parse-only: the block list is never walked in the enumeration fixture.
const STORE_LIST_OFF: u64 = 0x8_C000;
/// Parse-only: a previous-bitmap pointer.
const STORE_PREVIOUS_BITMAP_OFF: u64 = 0x9_0000;
const STORE_INFO_SIZE: u64 = 300;
const IMG_LEN: usize = 0x10000;
const OP_MACHINE: &str = "HOST-A";
const SVC_MACHINE: &str = "SVC-B";

fn wr(buf: &mut [u8], off: usize, data: &[u8]) {
    buf[off..off + data.len()].copy_from_slice(data);
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Build a minimal valid VSS volume image. `with_type3` controls whether the
/// snapshot's type-0x03 store pointer (and its store block/info) are present.
fn build_image(with_type3: bool) -> Vec<u8> {
    let mut b = vec![0u8; IMG_LEN];

    // Volume header @ 0x1E00.
    wr(&mut b, 0x1E00, &VSS_IDENTIFIER);
    wr(&mut b, 0x1E00 + 16, &1u32.to_le_bytes());
    wr(&mut b, 0x1E00 + 20, &1u32.to_le_bytes());
    wr(&mut b, 0x1E00 + 24, &0x1E00u64.to_le_bytes());
    wr(&mut b, 0x1E00 + 48, &CATALOG_OFF.to_le_bytes());

    // Catalog block @ CATALOG_OFF.
    let c = CATALOG_OFF as usize;
    wr(&mut b, c, &VSS_IDENTIFIER);
    wr(&mut b, c + 16, &1u32.to_le_bytes());
    wr(&mut b, c + 20, &2u32.to_le_bytes());
    wr(&mut b, c + 40, &0u64.to_le_bytes()); // next block offset = 0 (last)

    // Catalog entry 0: type 0x02 snapshot descriptor.
    let e0 = c + 128;
    wr(&mut b, e0, &2u64.to_le_bytes());
    wr(&mut b, e0 + 8, &VOL_SIZE.to_le_bytes());
    wr(&mut b, e0 + 16, &STORE_ID);
    wr(&mut b, e0 + 32, &SEQ.to_le_bytes());
    wr(&mut b, e0 + 40, &FLAGS.to_le_bytes());
    wr(&mut b, e0 + 48, &CTIME.to_le_bytes());

    if with_type3 {
        // Catalog entry 1: type 0x03 store pointer.
        let e1 = e0 + 128;
        wr(&mut b, e1, &3u64.to_le_bytes());
        wr(&mut b, e1 + 8, &STORE_LIST_OFF.to_le_bytes());
        wr(&mut b, e1 + 16, &STORE_ID);
        wr(&mut b, e1 + 32, &STORE_HDR_OFF.to_le_bytes());
        wr(&mut b, e1 + 48, &STORE_BITMAP_OFF.to_le_bytes());
        wr(&mut b, e1 + 72, &STORE_PREVIOUS_BITMAP_OFF.to_le_bytes());

        // Store block header @ STORE_HDR_OFF.
        let s = STORE_HDR_OFF as usize;
        wr(&mut b, s, &VSS_IDENTIFIER);
        wr(&mut b, s + 16, &1u32.to_le_bytes());
        wr(&mut b, s + 20, &4u32.to_le_bytes());
        wr(&mut b, s + 48, &STORE_INFO_SIZE.to_le_bytes());

        // Store information @ store header + 128.
        let si = s + 128;
        wr(&mut b, si + 16, &SHADOW_ID);
        wr(&mut b, si + 32, &SET_ID);
        wr(&mut b, si + 48, &CTX.to_le_bytes());
        wr(&mut b, si + 56, &ATTR.to_le_bytes());
        let op = utf16le(OP_MACHINE);
        wr(&mut b, si + 64, &(op.len() as u16).to_le_bytes());
        wr(&mut b, si + 66, &op);
        let svc_off = si + 66 + op.len();
        let svc = utf16le(SVC_MACHINE);
        wr(&mut b, svc_off, &(svc.len() as u16).to_le_bytes());
        wr(&mut b, svc_off + 2, &svc);
    }
    b
}

// ---------------------------------------------------------------------------
// GUID formatting
// ---------------------------------------------------------------------------

#[test]
fn format_guid_matches_vss_identifier() {
    // The canonical VSS GUID from the libvshadow spec — an authoritative check
    // that the mixed-endian rendering is correct.
    assert_eq!(
        format_guid(&VSS_IDENTIFIER),
        "3808876b-c176-4e48-b7ae-04046e6cc752"
    );
}

#[test]
fn format_guid_store_id() {
    assert_eq!(format_guid(&STORE_ID), STORE_ID_STR);
}

// ---------------------------------------------------------------------------
// Volume header
// ---------------------------------------------------------------------------

#[test]
fn volume_header_parse_valid() {
    let b = build_image(true);
    let vh = VolumeHeader::parse(&b[0x1E00..0x1E00 + 128]);
    assert!(vh.has_vss_identifier);
    assert_eq!(vh.version, 1);
    assert_eq!(vh.record_type, 1);
    assert_eq!(vh.current_offset, 0x1E00);
    assert_eq!(vh.catalog_offset, CATALOG_OFF);
}

#[test]
fn volume_header_no_identifier() {
    let vh = VolumeHeader::parse(&[0u8; 128]);
    assert!(!vh.has_vss_identifier);
    assert_eq!(vh.catalog_offset, 0);
}

// ---------------------------------------------------------------------------
// Catalog entries
// ---------------------------------------------------------------------------

#[test]
fn parse_catalog_entry_snapshot() {
    let b = build_image(true);
    let e0 = CATALOG_OFF as usize + 128;
    match parse_catalog_entry(&b[e0..e0 + CATALOG_ENTRY_LEN]) {
        CatalogEntry::Snapshot(d) => {
            assert_eq!(d.store_id, STORE_ID);
            assert_eq!(d.volume_size, VOL_SIZE);
            assert_eq!(d.sequence, SEQ);
            assert_eq!(d.flags, FLAGS);
            assert_eq!(d.creation_time, CTIME);
            assert_eq!(d.store_header_offset, None);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[test]
fn parse_catalog_entry_store_pointer() {
    let b = build_image(true);
    let e1 = CATALOG_OFF as usize + 256;
    assert_eq!(
        parse_catalog_entry(&b[e1..e1 + CATALOG_ENTRY_LEN]),
        CatalogEntry::StorePointer {
            store_id: STORE_ID,
            store_block_list_offset: STORE_LIST_OFF,
            store_header_offset: STORE_HDR_OFF,
            store_bitmap_offset: STORE_BITMAP_OFF,
            store_previous_bitmap_offset: STORE_PREVIOUS_BITMAP_OFF,
            store_block_range_list_offset: 0,
            ntfs_file_reference: 0,
            allocated_size: 0,
        }
    );
}

#[test]
fn parse_catalog_entry_empty_and_other() {
    assert_eq!(parse_catalog_entry(&[0u8; 128]), CatalogEntry::Empty);
    let mut other = [0u8; 128];
    other[0] = 0x01; // type 0x01
    assert_eq!(parse_catalog_entry(&other), CatalogEntry::Other);
}

#[test]
fn parse_catalog_entry_short_buffer_no_panic() {
    // Bounds-checked reads must yield an empty/zero entry, never panic.
    assert_eq!(parse_catalog_entry(&[]), CatalogEntry::Empty);
}

#[test]
fn catalog_helpers() {
    let b = build_image(true);
    let c = CATALOG_OFF as usize;
    assert!(is_catalog_block(&b[c..c + BLOCK_SIZE]));
    assert_eq!(catalog_next_block_offset(&b[c..c + BLOCK_SIZE]), 0);
    assert!(!is_catalog_block(&[0u8; BLOCK_SIZE]));
}

// ---------------------------------------------------------------------------
// Volume open / enumeration
// ---------------------------------------------------------------------------

#[test]
fn open_enumerates_single_store() {
    let vol = VssVolume::open(Cursor::new(build_image(true))).unwrap();
    assert!(vol.has_vss_header());
    assert_eq!(vol.store_count(), 1);
    assert_eq!(vol.catalog_offset(), CATALOG_OFF);
    assert_eq!(vol.volume_size(), IMG_LEN as u64);
    assert_eq!(vol.stores()[0].store_id_string(), STORE_ID_STR);
    assert_eq!(
        vol.stores()[0],
        StoreDescriptor {
            store_id: STORE_ID,
            volume_size: VOL_SIZE,
            sequence: SEQ,
            flags: FLAGS,
            creation_time: CTIME,
            store_header_offset: Some(STORE_HDR_OFF),
            store_bitmap_offset: Some(STORE_BITMAP_OFF),
            store_block_list_offset: Some(STORE_LIST_OFF),
            store_previous_bitmap_offset: Some(STORE_PREVIOUS_BITMAP_OFF),
            store_block_range_list_offset: Some(0),
            ntfs_file_reference: Some(0),
            allocated_size: Some(0),
        }
    );
}

#[test]
fn open_no_vss_header() {
    let vol = VssVolume::open(Cursor::new(vec![0u8; IMG_LEN])).unwrap();
    assert!(!vol.has_vss_header());
    assert_eq!(vol.store_count(), 0);
    assert_eq!(vol.stores(), []);
}

#[test]
fn open_tiny_reader_no_header() {
    // A reader smaller than the header offset opens cleanly with no VSS.
    let vol = VssVolume::open(Cursor::new(vec![0u8; 100])).unwrap();
    assert!(!vol.has_vss_header());
    assert_eq!(vol.store_count(), 0);
}

#[test]
fn open_header_but_no_catalog() {
    let mut b = build_image(true);
    wr(&mut b, 0x1E00 + 48, &0u64.to_le_bytes()); // catalog offset = 0
    let vol = VssVolume::open(Cursor::new(b)).unwrap();
    assert!(vol.has_vss_header());
    assert_eq!(vol.store_count(), 0);
}

#[test]
fn open_corrupt_catalog_block_stops() {
    let mut b = build_image(true);
    let c = CATALOG_OFF as usize;
    wr(&mut b, c, &[0u8; 16]); // wipe the catalog block's VSS identifier
    assert!(matches!(
        VssVolume::open(Cursor::new(b)),
        Err(VssError::CorruptVolumeMetadata { .. })
    ));
}

#[test]
fn open_catalog_next_offset_out_of_range_stops() {
    let mut b = build_image(true);
    let c = CATALOG_OFF as usize;
    wr(&mut b, c + 40, &(IMG_LEN as u64).to_le_bytes()); // next block past end
    assert!(matches!(
        VssVolume::open(Cursor::new(b)),
        Err(VssError::CorruptVolumeMetadata { .. })
    ));
}

#[test]
fn open_catalog_self_loop_terminates() {
    let mut b = build_image(true);
    let c = CATALOG_OFF as usize;
    wr(&mut b, c + 40, &CATALOG_OFF.to_le_bytes()); // next block points at itself
    assert!(matches!(
        VssVolume::open(Cursor::new(b)),
        Err(VssError::CorruptVolumeMetadata { .. })
    ));
}

// ---------------------------------------------------------------------------
// Store information
// ---------------------------------------------------------------------------

#[test]
fn store_info_roundtrip() {
    let mut vol = VssVolume::open(Cursor::new(build_image(true))).unwrap();
    let info = vol.store_info(0).unwrap();
    assert_eq!(info.shadow_copy_id, SHADOW_ID);
    assert_eq!(info.shadow_copy_id_string(), SHADOW_ID_STR);
    assert_eq!(info.shadow_copy_set_id_string(), SET_ID_STR);
    assert_eq!(info.snapshot_context, CTX);
    assert_eq!(info.attributes.bits(), ATTR);
    assert!(info.attributes.is_persistent());
    assert!(info.attributes.is_client_accessible());
    assert!(info.attributes.is_differential());
    assert_eq!(info.operating_machine, OP_MACHINE);
    assert_eq!(info.service_machine, SVC_MACHINE);
}

#[test]
fn store_info_index_out_of_range() {
    let mut vol = VssVolume::open(Cursor::new(build_image(true))).unwrap();
    match vol.store_info(3) {
        Err(VssError::StoreIndexOutOfRange { index, count }) => {
            assert_eq!(index, 3);
            assert_eq!(count, 1);
        }
        other => panic!("expected StoreIndexOutOfRange, got {other:?}"),
    }
}

#[test]
fn store_info_unavailable_without_type3() {
    let mut vol = VssVolume::open(Cursor::new(build_image(false))).unwrap();
    assert_eq!(vol.store_count(), 1);
    assert!(vol.stores()[0].store_header_offset.is_none());
    match vol.store_info(0) {
        Err(VssError::StoreInfoUnavailable { index }) => assert_eq!(index, 0),
        other => panic!("expected StoreInfoUnavailable, got {other:?}"),
    }
}

#[test]
fn store_info_offset_out_of_bounds() {
    let mut b = build_image(true);
    let e1 = CATALOG_OFF as usize + 256;
    wr(
        &mut b,
        e1 + 32,
        &(IMG_LEN as u64 + 0x0010_0000).to_le_bytes(),
    );
    let mut vol = VssVolume::open(Cursor::new(b)).unwrap();
    match vol.store_info(0) {
        Err(VssError::StoreOffsetOutOfBounds { index, offset, .. }) => {
            assert_eq!(index, 0);
            assert_eq!(offset, IMG_LEN as u64 + 0x0010_0000);
        }
        other => panic!("expected StoreOffsetOutOfBounds, got {other:?}"),
    }
}

#[test]
fn store_info_offset_overflow() {
    let mut b = build_image(true);
    let e1 = CATALOG_OFF as usize + 256;
    wr(&mut b, e1 + 32, &u64::MAX.to_le_bytes());
    let mut vol = VssVolume::open(Cursor::new(b)).unwrap();
    assert!(matches!(
        vol.store_info(0),
        Err(VssError::StoreOffsetOutOfBounds { .. })
    ));
}

// ---------------------------------------------------------------------------
// Store block header + attribute flags
// ---------------------------------------------------------------------------

#[test]
fn store_block_header_parse() {
    let b = build_image(true);
    let s = STORE_HDR_OFF as usize;
    let sh = StoreBlockHeader::parse(&b[s..s + 128]);
    assert!(sh.has_vss_identifier);
    assert_eq!(sh.version, 1);
    assert_eq!(sh.record_type, 4);
    assert_eq!(sh.store_information_size, STORE_INFO_SIZE);
    assert_eq!(sh.next_offset, 0);
}

#[test]
fn attribute_flags_predicates() {
    let f = AttributeFlags(ATTR);
    assert_eq!(f.bits(), ATTR);
    assert!(f.is_persistent());
    assert!(f.is_client_accessible());
    assert!(f.is_differential());
    assert!(f.contains(AttributeFlags::PERSISTENT));
    let none = AttributeFlags(0);
    assert!(!none.is_persistent());
    assert!(!none.is_client_accessible());
    assert!(!none.is_differential());
    assert!(!none.contains(AttributeFlags::PERSISTENT));
}

#[test]
fn store_info_parse_short_buffer_no_panic() {
    let info = StoreInfo::parse(&[]);
    assert_eq!(info.operating_machine, "");
    assert_eq!(info.service_machine, "");
    assert_eq!(info.attributes.bits(), 0);
}

// ---------------------------------------------------------------------------
// Diff-area records (Phase-1 typed parsing only)
// ---------------------------------------------------------------------------

#[test]
fn block_descriptor_parse() {
    let mut buf = [0u8; 32];
    wr(&mut buf, 0, &0x1111_2222_3333_4444u64.to_le_bytes());
    wr(&mut buf, 8, &0x0000_0000_0000_5555u64.to_le_bytes());
    wr(&mut buf, 16, &0xaaaa_bbbb_cccc_ddddu64.to_le_bytes());
    wr(&mut buf, 24, &(BlockDescriptorFlags::OVERLAY).to_le_bytes());
    wr(&mut buf, 28, &0x89ab_cdefu32.to_le_bytes());
    let d = BlockDescriptor::parse(&buf);
    assert_eq!(d.original_offset, 0x1111_2222_3333_4444);
    assert_eq!(d.relative_store_offset, 0x5555);
    assert_eq!(d.store_offset, 0xaaaa_bbbb_cccc_dddd);
    assert!(d.flags.is_overlay());
    assert_eq!(d.allocation_bitmap, 0x89ab_cdef);
}

#[test]
fn block_descriptor_short_buffer_no_panic() {
    let d = BlockDescriptor::parse(&[]);
    assert_eq!(d.original_offset, 0);
    assert_eq!(d.allocation_bitmap, 0);
}

#[test]
fn block_descriptor_flags_predicates() {
    let f = BlockDescriptorFlags(BlockDescriptorFlags::FORWARDER | BlockDescriptorFlags::OVERLAY);
    assert_eq!(
        f.bits(),
        BlockDescriptorFlags::FORWARDER | BlockDescriptorFlags::OVERLAY
    );
    assert!(f.is_forwarder());
    assert!(f.is_overlay());
    assert!(!f.is_not_used());
    assert!(f.contains(BlockDescriptorFlags::FORWARDER));
    assert!(BlockDescriptorFlags(BlockDescriptorFlags::NOT_USED).is_not_used());
}

#[test]
fn store_block_range_parse() {
    let mut buf = [0u8; 24];
    wr(&mut buf, 0, &0x1000u64.to_le_bytes());
    wr(&mut buf, 8, &0x2000u64.to_le_bytes());
    wr(&mut buf, 16, &0x4000u64.to_le_bytes());
    let r = StoreBlockRange::parse(&buf);
    assert_eq!(r.store_offset, 0x1000);
    assert_eq!(r.relative_offset, 0x2000);
    assert_eq!(r.range_size, 0x4000);
    let empty = StoreBlockRange::parse(&[]);
    assert_eq!(empty.range_size, 0);
}

// ---------------------------------------------------------------------------
// UTF-16 decoding helper
// ---------------------------------------------------------------------------

#[test]
fn utf16le_string_handles_lone_surrogate_and_odd_byte() {
    // Lone high surrogate -> replacement char; trailing odd byte -> ignored.
    let bytes = [0x00, 0xd8, 0x41, 0x00, 0x42];
    assert_eq!(utf16le_string(&bytes), "\u{FFFD}A");
    assert_eq!(utf16le_string(&[]), "");
}

// ===========================================================================
// Copy-on-write reconstruction across the store chain (synthetic mechanics).
//
// Each fixture is an in-memory VSS volume: a catalog naming one or more stores,
// each with a block-descriptor list, a current bitmap and optionally a previous
// bitmap, plus filled data blocks. These prove mechanics only; the
// authoritative check is the env-gated pyvshadow oracle in
// `tests/oracle_dfvfs.rs`.
//
// Layout (16 KiB blocks): 0 boot area + volume header, 1 catalog, 2..=31 live
// volume blocks, 32..=39 saved store data, 40.. metadata (4 blocks per store:
// header, block list, current bitmap, previous bitmap).
// ===========================================================================

const RBS: usize = BLOCK_SIZE;
const R_IMG_LEN: usize = 64 * RBS;
const R_CATALOG: usize = 1;

const fn block(number: usize) -> u64 {
    (number * RBS) as u64
}

fn header_block(store: usize) -> usize {
    40 + 4 * store
}

fn list_block(store: usize) -> usize {
    header_block(store) + 1
}

fn bitmap_block(store: usize) -> usize {
    header_block(store) + 2
}

fn previous_bitmap_block(store: usize) -> usize {
    header_block(store) + 3
}

#[derive(Debug, Clone, Copy)]
struct Desc {
    original: u64,
    relative: u64,
    store: u64,
    flags: u32,
    allocation: u32,
}

fn plain(original: usize, store: usize) -> Desc {
    Desc {
        original: block(original),
        relative: 0,
        store: block(store),
        flags: 0,
        allocation: 0,
    }
}

fn overlay(original: usize, store: usize, allocation: u32) -> Desc {
    Desc {
        original: block(original),
        relative: 1,
        store: block(store),
        flags: BlockDescriptorFlags::OVERLAY,
        allocation,
    }
}

fn forwarder(original: usize, target: usize) -> Desc {
    Desc {
        original: block(original),
        relative: block(target),
        store: 0,
        flags: BlockDescriptorFlags::FORWARDER,
        allocation: 0,
    }
}

fn not_used(desc: Desc) -> Desc {
    Desc {
        flags: desc.flags | BlockDescriptorFlags::NOT_USED,
        ..desc
    }
}

#[derive(Debug, Default)]
struct FixtureStore {
    creation_time: u64,
    descriptors: Vec<Desc>,
    /// Blocks set in the current bitmap.
    unallocated: Vec<usize>,
    /// Blocks set in the previous bitmap, when the store has one.
    previous_unallocated: Option<Vec<usize>>,
}

fn store_block_hdr(b: &mut [u8], off: u64, record_type: u32, next: u64) {
    let o = off as usize;
    wr(b, o, &VSS_IDENTIFIER);
    wr(b, o + 16, &1u32.to_le_bytes());
    wr(b, o + 20, &record_type.to_le_bytes());
    wr(b, o + 32, &off.to_le_bytes());
    wr(b, o + 40, &next.to_le_bytes());
}

fn write_bitmap(b: &mut [u8], at: usize, blocks: &[usize]) {
    store_block_hdr(b, block(at), 6, 0);
    let payload = block(at) as usize + 128;
    for &n in blocks {
        b[payload + n / 8] |= 1 << (n % 8);
    }
}

fn fill_block(b: &mut [u8], number: usize, byte: u8) {
    let start = block(number) as usize;
    b[start..start + RBS].fill(byte);
}

/// Build a volume whose catalog lists `stores` in the given order, with
/// `fills` applied to whole blocks.
fn build_chain_image(stores: &[FixtureStore], fills: &[(usize, u8)]) -> Vec<u8> {
    let mut b = vec![0u8; R_IMG_LEN];
    wr(&mut b, 0x1E00, &VSS_IDENTIFIER);
    wr(&mut b, 0x1E00 + 16, &1u32.to_le_bytes());
    wr(&mut b, 0x1E00 + 20, &1u32.to_le_bytes());
    wr(&mut b, 0x1E00 + 24, &0x1E00u64.to_le_bytes());
    wr(&mut b, 0x1E00 + 48, &block(R_CATALOG).to_le_bytes());

    let catalog = block(R_CATALOG) as usize;
    wr(&mut b, catalog, &VSS_IDENTIFIER);
    wr(&mut b, catalog + 16, &1u32.to_le_bytes());
    wr(&mut b, catalog + 20, &2u32.to_le_bytes());

    let mut entry = catalog + 128;
    for (index, store) in stores.iter().enumerate() {
        let mut id = STORE_ID;
        id[15] = index as u8;

        wr(&mut b, entry, &2u64.to_le_bytes());
        wr(&mut b, entry + 8, &(R_IMG_LEN as u64).to_le_bytes());
        wr(&mut b, entry + 16, &id);
        wr(&mut b, entry + 32, &(index as u64 + 1).to_le_bytes());
        wr(&mut b, entry + 48, &store.creation_time.to_le_bytes());
        entry += 128;

        store_block_hdr(&mut b, block(header_block(index)), 4, 0);
        store_block_hdr(&mut b, block(list_block(index)), 3, 0);
        let mut record = block(list_block(index)) as usize + 128;
        for desc in &store.descriptors {
            wr(&mut b, record, &desc.original.to_le_bytes());
            wr(&mut b, record + 8, &desc.relative.to_le_bytes());
            wr(&mut b, record + 16, &desc.store.to_le_bytes());
            wr(&mut b, record + 24, &desc.flags.to_le_bytes());
            wr(&mut b, record + 28, &desc.allocation.to_le_bytes());
            record += 32;
        }
        write_bitmap(&mut b, bitmap_block(index), &store.unallocated);
        let previous = match &store.previous_unallocated {
            Some(blocks) => {
                write_bitmap(&mut b, previous_bitmap_block(index), blocks);
                block(previous_bitmap_block(index))
            }
            None => 0,
        };

        wr(&mut b, entry, &3u64.to_le_bytes());
        wr(&mut b, entry + 8, &block(list_block(index)).to_le_bytes());
        wr(&mut b, entry + 16, &id);
        wr(
            &mut b,
            entry + 32,
            &block(header_block(index)).to_le_bytes(),
        );
        wr(
            &mut b,
            entry + 48,
            &block(bitmap_block(index)).to_le_bytes(),
        );
        wr(&mut b, entry + 72, &previous.to_le_bytes());
        entry += 128;
    }
    for &(number, byte) in fills {
        fill_block(&mut b, number, byte);
    }
    b
}

fn single_store(descriptors: Vec<Desc>, unallocated: Vec<usize>) -> Vec<FixtureStore> {
    vec![FixtureStore {
        creation_time: CTIME,
        descriptors,
        unallocated,
        previous_unallocated: None,
    }]
}

fn read_store_block(image: &[u8], store: usize, number: usize) -> [u8; RBS] {
    let mut vol = VssVolume::open(Cursor::new(image.to_vec())).unwrap();
    let mut snap = vol.snapshot(store).unwrap();
    snap.read_block(block(number)).unwrap()
}

fn snapshot_error(image: Vec<u8>, store: usize) -> VssError {
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    match vol.snapshot(store) {
        Err(e) => e,
        Ok(_) => panic!("expected the snapshot to be rejected"),
    }
}

/// A block of `base` bytes with the listed 512-byte sub-blocks set to `patch`.
fn patched(base: u8, patch: u8, sub_blocks: &[usize]) -> [u8; RBS] {
    let mut out = [base; RBS];
    for &sub in sub_blocks {
        out[sub * 512..(sub + 1) * 512].fill(patch);
    }
    out
}

// ---------------------------------------------------------------------------
// One store (the newest, read directly)
// ---------------------------------------------------------------------------

#[test]
fn reconstruct_passthrough_block() {
    let image = build_chain_image(&single_store(vec![], vec![]), &[(2, 0x22)]);
    assert_eq!(read_store_block(&image, 0, 2), [0x22u8; RBS]);
}

#[test]
fn reconstruct_zero_fill_block() {
    // Live bytes are 0x33 but the bitmap marks block 3 unallocated -> zeros.
    let image = build_chain_image(&single_store(vec![], vec![3]), &[(3, 0x33)]);
    assert_eq!(read_store_block(&image, 0, 3), [0x00u8; RBS]);
}

#[test]
fn reconstruct_plain_cow_block_ignores_not_used() {
    let stores = single_store(vec![plain(10, 32), not_used(plain(10, 33))], vec![]);
    let image = build_chain_image(&stores, &[(10, 0xAA), (32, 0x77), (33, 0x88)]);
    assert_eq!(read_store_block(&image, 0, 10), [0x77u8; RBS]);
}

#[test]
fn reconstruct_later_plain_replaces_earlier() {
    let stores = single_store(vec![plain(10, 32), plain(10, 33)], vec![]);
    let image = build_chain_image(&stores, &[(32, 0x77), (33, 0x88)]);
    assert_eq!(read_store_block(&image, 0, 10), [0x88u8; RBS]);
}

#[test]
fn reconstruct_overlay_block() {
    let stores = single_store(vec![plain(11, 33), overlay(11, 34, 0b101)], vec![]);
    let image = build_chain_image(&stores, &[(33, 0x88), (34, 0x99)]);
    assert_eq!(
        read_store_block(&image, 0, 11),
        patched(0x88, 0x99, &[0, 2])
    );
}

#[test]
fn reconstruct_overlay_only_uses_live_base() {
    let stores = single_store(
        vec![overlay(12, 34, 0b001), not_used(overlay(12, 33, u32::MAX))],
        vec![],
    );
    let image = build_chain_image(&stores, &[(12, 0xCC), (33, 0x88), (34, 0x99)]);
    assert_eq!(read_store_block(&image, 0, 12), patched(0xCC, 0x99, &[0]));
}

#[test]
fn reconstruct_overlays_merge_their_bitmaps() {
    let stores = single_store(vec![overlay(12, 34, 0b001), overlay(12, 35, 0b100)], vec![]);
    let image = build_chain_image(&stores, &[(12, 0xCC), (34, 0x99), (35, 0x55)]);
    // The second overlay extends the first: both sub-blocks come from the
    // first overlay's saved block.
    assert_eq!(
        read_store_block(&image, 0, 12),
        patched(0xCC, 0x99, &[0, 2])
    );
}

#[test]
fn reconstruct_plain_after_overlay_keeps_the_overlay() {
    let stores = single_store(vec![overlay(13, 34, 0b010), plain(13, 32)], vec![]);
    let image = build_chain_image(&stores, &[(13, 0xCC), (32, 0x77), (34, 0x99)]);
    assert_eq!(read_store_block(&image, 0, 13), patched(0x77, 0x99, &[1]));
}

#[test]
fn reconstruct_unaligned_read_inside_overlay_run() {
    let stores = single_store(vec![plain(11, 33), overlay(11, 34, 0b0110)], vec![]);
    let image = build_chain_image(&stores, &[(33, 0x88), (34, 0x99)]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    // Bytes 700..1700 of the block: sub-block 1 (0x99) from 700, sub-block 2
    // (0x99) and sub-block 3 (0x88) up to 1700.
    let mut buf = [0u8; 1000];
    snap.read_at(block(11) + 700, &mut buf).unwrap();
    let mut expected = [0x99u8; 1000];
    expected[1536 - 700..].fill(0x88);
    assert_eq!(buf, expected);
}

#[test]
fn reconstruct_out_of_range_store_offset_is_an_error() {
    let mut bad = plain(13, 0);
    bad.store = 0xFFFF_0000;
    let image = build_chain_image(&single_store(vec![bad], vec![]), &[]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    assert!(matches!(snap.read_block(block(13)), Err(VssError::Io(_))));
}

#[test]
fn reconstruct_read_at_spans_block_boundary() {
    let image = build_chain_image(&single_store(vec![], vec![3]), &[(2, 0x22), (3, 0x33)]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    // Straddle block 2 (0x22 passthrough) and block 3 (bitmap zero-fill).
    let mut buf = [0xEEu8; 1024];
    snap.read_at(block(3) - 512, &mut buf).unwrap();
    let mut expected = [0u8; 1024];
    expected[0..512].fill(0x22);
    assert_eq!(buf, expected);
}

#[test]
fn reconstruct_read_past_volume_end_is_an_error() {
    let image = build_chain_image(&single_store(vec![], vec![]), &[]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    let mut buf = [0u8; RBS];
    match snap.read_at(R_IMG_LEN as u64 - 512, &mut buf) {
        Err(VssError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
        other => panic!("expected an EOF error, got {other:?}"),
    }
}

#[test]
fn reconstruct_read_block_unaligned_offset_aligns_down() {
    let image = build_chain_image(&single_store(vec![], vec![]), &[(2, 0x22)]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    assert_eq!(snap.read_block(block(2) + 777).unwrap(), [0x22u8; RBS]);
}

#[test]
fn reconstruct_read_at_empty_buffer_is_ok() {
    let image = build_chain_image(&single_store(vec![], vec![]), &[]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snap = vol.snapshot(0).unwrap();
    snap.read_at(0, &mut []).unwrap();
}

#[test]
fn forwarder_in_newest_store_reads_the_volume_at_its_target() {
    let stores = single_store(vec![forwarder(14, 15)], vec![15]);
    let image = build_chain_image(&stores, &[(14, 0x14), (15, 0x15)]);
    assert_eq!(read_store_block(&image, 0, 14), [0x15u8; RBS]);
    // A forwarder target is never zero-filled, even when its bitmap bit is set.
    assert_eq!(read_store_block(&image, 0, 15), [0x15u8; RBS]);
}

#[test]
fn forwarder_target_descriptor_takes_over_the_forwarded_block() {
    // The plain descriptor saved for block 15 is really block 14's old data:
    // the earlier forwarder 14 -> 15 swaps its original offset and is consumed.
    let stores = single_store(vec![forwarder(14, 15), plain(15, 32)], vec![15]);
    let image = build_chain_image(&stores, &[(14, 0x14), (15, 0x15), (32, 0x77)]);
    assert_eq!(read_store_block(&image, 0, 14), [0x77u8; RBS]);
    // Block 15 is no longer a forwarder target, so its bitmap bit zero-fills it.
    assert_eq!(read_store_block(&image, 0, 15), [0x00u8; RBS]);
}

#[test]
fn forwarder_to_itself_is_ignored() {
    let stores = single_store(vec![forwarder(14, 14)], vec![]);
    let image = build_chain_image(&stores, &[(14, 0x14)]);
    assert_eq!(read_store_block(&image, 0, 14), [0x14u8; RBS]);
}

#[test]
fn previous_bitmap_gates_zero_fill() {
    let stores = vec![FixtureStore {
        creation_time: CTIME,
        descriptors: vec![],
        unallocated: vec![20, 21],
        previous_unallocated: Some(vec![21]),
    }];
    let image = build_chain_image(&stores, &[(20, 0x20), (21, 0x21)]);
    assert_eq!(read_store_block(&image, 0, 20), [0x20u8; RBS]);
    assert_eq!(read_store_block(&image, 0, 21), [0x00u8; RBS]);
}

// ---------------------------------------------------------------------------
// Several stores: older snapshots read through newer stores
// ---------------------------------------------------------------------------

/// Store 0 (older) and store 1 (newer) over the same volume.
fn two_store_image(older: Vec<Desc>, newer: Vec<Desc>, fills: &[(usize, u8)]) -> Vec<u8> {
    let stores = vec![
        FixtureStore {
            creation_time: CTIME,
            descriptors: older,
            unallocated: vec![],
            previous_unallocated: None,
        },
        FixtureStore {
            creation_time: CTIME + 1,
            descriptors: newer,
            unallocated: vec![],
            previous_unallocated: None,
        },
    ];
    build_chain_image(&stores, fills)
}

#[test]
fn older_store_reads_blocks_saved_by_a_newer_store() {
    let image = two_store_image(
        vec![plain(4, 32)],
        vec![plain(4, 33), plain(5, 34)],
        &[
            (4, 0x04),
            (5, 0x05),
            (6, 0x06),
            (32, 0xA0),
            (33, 0xA1),
            (34, 0xB1),
        ],
    );
    // Block 4: each store's own saved copy.
    assert_eq!(read_store_block(&image, 0, 4), [0xA0u8; RBS]);
    assert_eq!(read_store_block(&image, 1, 4), [0xA1u8; RBS]);
    // Block 5 changed only after the newer snapshot: both read its saved copy.
    assert_eq!(read_store_block(&image, 0, 5), [0xB1u8; RBS]);
    assert_eq!(read_store_block(&image, 1, 5), [0xB1u8; RBS]);
    // Block 6 never changed.
    assert_eq!(read_store_block(&image, 0, 6), [0x06u8; RBS]);
}

#[test]
fn only_the_newest_store_read_directly_zero_fills() {
    let stores = vec![
        FixtureStore {
            creation_time: CTIME,
            descriptors: vec![],
            unallocated: vec![7],
            previous_unallocated: None,
        },
        FixtureStore {
            creation_time: CTIME + 1,
            descriptors: vec![],
            unallocated: vec![7],
            previous_unallocated: None,
        },
    ];
    let image = build_chain_image(&stores, &[(7, 0x07)]);
    assert_eq!(read_store_block(&image, 0, 7), [0x07u8; RBS]);
    assert_eq!(read_store_block(&image, 1, 7), [0x00u8; RBS]);
}

#[test]
fn newer_overlay_only_entry_is_invisible_to_an_older_store() {
    let image = two_store_image(
        vec![],
        vec![overlay(8, 34, 0b001)],
        &[(8, 0xDD), (34, 0x99)],
    );
    assert_eq!(read_store_block(&image, 0, 8), [0xDDu8; RBS]);
    assert_eq!(read_store_block(&image, 1, 8), patched(0xDD, 0x99, &[0]));
}

#[test]
fn older_store_uses_a_newer_base_without_its_overlay() {
    let image = two_store_image(
        vec![],
        vec![plain(9, 33), overlay(9, 34, 0b001)],
        &[(9, 0xDD), (33, 0xE1), (34, 0x99)],
    );
    assert_eq!(read_store_block(&image, 0, 9), [0xE1u8; RBS]);
    assert_eq!(read_store_block(&image, 1, 9), patched(0xE1, 0x99, &[0]));
}

#[test]
fn older_forwarder_reads_the_next_store_at_its_target() {
    let image = two_store_image(
        vec![forwarder(16, 17)],
        vec![plain(17, 33)],
        &[(16, 0x16), (17, 0x17), (33, 0x61)],
    );
    assert_eq!(read_store_block(&image, 0, 16), [0x61u8; RBS]);
    assert_eq!(read_store_block(&image, 0, 17), [0x61u8; RBS]);
    assert_eq!(read_store_block(&image, 1, 16), [0x16u8; RBS]);
}

#[test]
fn stores_are_ordered_by_creation_time() {
    // The catalog lists the newer store first.
    let stores = vec![
        FixtureStore {
            creation_time: CTIME + 1,
            descriptors: vec![plain(5, 34)],
            unallocated: vec![],
            previous_unallocated: None,
        },
        FixtureStore {
            creation_time: CTIME,
            descriptors: vec![],
            unallocated: vec![],
            previous_unallocated: None,
        },
    ];
    let image = build_chain_image(&stores, &[(5, 0x05), (34, 0xB1)]);
    let vol = VssVolume::open(Cursor::new(image.clone())).unwrap();
    let times: Vec<u64> = vol.stores().iter().map(|s| s.creation_time).collect();
    assert_eq!(times, vec![CTIME, CTIME + 1]);
    // The older store (now index 0) reads block 5 through the newer one.
    assert_eq!(read_store_block(&image, 0, 5), [0xB1u8; RBS]);
}

#[test]
fn store_map_counts_defined_blocks() {
    let image = two_store_image(vec![plain(4, 32), overlay(4, 33, 1)], vec![], &[]);
    let mut vol = VssVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(vol.store_map(0).unwrap().block_count(), 1);
    assert_eq!(vol.store_map(1).unwrap().block_count(), 0);
}

// ---------------------------------------------------------------------------
// Errors and corrupt metadata
// ---------------------------------------------------------------------------

#[test]
fn snapshot_index_out_of_range() {
    let image = build_chain_image(&single_store(vec![], vec![]), &[]);
    match snapshot_error(image, 5) {
        VssError::StoreIndexOutOfRange { index, count } => {
            assert_eq!((index, count), (5, 1));
        }
        e => panic!("expected StoreIndexOutOfRange, got {e:?}"),
    }
}

#[test]
fn snapshot_unavailable_without_type3() {
    // build_image(false) has a snapshot entry but no type-0x03 store pointer.
    match snapshot_error(build_image(false), 0) {
        VssError::StoreInfoUnavailable { index } => assert_eq!(index, 0),
        e => panic!("expected StoreInfoUnavailable, got {e:?}"),
    }
}

/// Snapshot store `read` and expect store `corrupt`'s block at `offset` to be
/// reported as corrupt.
fn assert_corrupt(image: Vec<u8>, read: usize, corrupt: usize, expected_offset: u64) {
    match snapshot_error(image, read) {
        VssError::CorruptStoreMetadata { index, offset, .. } => {
            assert_eq!((index, offset), (corrupt, expected_offset));
        }
        e => panic!("expected CorruptStoreMetadata, got {e:?}"),
    }
}

#[test]
fn block_list_with_wrong_record_type_is_corrupt() {
    let mut image = build_chain_image(&single_store(vec![plain(10, 32)], vec![]), &[]);
    let list = block(list_block(0));
    wr(&mut image, list as usize + 20, &5u32.to_le_bytes());
    assert_corrupt(image, 0, 0, list);
}

#[test]
fn block_list_with_wrong_version_is_corrupt() {
    let mut image = build_chain_image(&single_store(vec![plain(10, 32)], vec![]), &[]);
    let list = block(list_block(0));
    wr(&mut image, list as usize + 16, &2u32.to_le_bytes());
    assert_corrupt(image, 0, 0, list);
}

#[test]
fn block_list_cycle_is_corrupt() {
    let mut image = build_chain_image(&single_store(vec![plain(10, 32)], vec![]), &[]);
    let list = block(list_block(0));
    wr(&mut image, list as usize + 40, &list.to_le_bytes());
    assert_corrupt(image, 0, 0, list);
}

#[test]
fn bitmap_next_out_of_range_is_corrupt() {
    let mut image = build_chain_image(&single_store(vec![], vec![3]), &[]);
    let bitmap = block(bitmap_block(0));
    let past_end = R_IMG_LEN as u64;
    wr(&mut image, bitmap as usize + 40, &past_end.to_le_bytes());
    assert_corrupt(image, 0, 0, past_end);
}

#[test]
fn metadata_block_shared_by_two_chains_is_corrupt() {
    let mut image = build_chain_image(&single_store(vec![], vec![3]), &[]);
    // Point the block list at the bitmap block.
    let entry = block(R_CATALOG) as usize + 256;
    let bitmap = block(bitmap_block(0));
    wr(&mut image, entry + 8, &bitmap.to_le_bytes());
    assert_corrupt(image, 0, 0, bitmap);
}

#[test]
fn forwarder_with_store_offset_is_corrupt() {
    let mut bad = forwarder(14, 15);
    bad.store = block(32);
    let image = build_chain_image(&single_store(vec![bad], vec![]), &[]);
    assert_corrupt(image, 0, 0, block(list_block(0)));
}

#[test]
fn newer_store_corruption_blocks_an_older_snapshot() {
    let mut image = two_store_image(vec![], vec![plain(4, 33)], &[]);
    let list = block(list_block(1));
    wr(&mut image, list as usize + 20, &5u32.to_le_bytes());
    assert_corrupt(image, 0, 1, list);
}
