use std::sync::Arc;

use crate::bytes::slice;
use crate::data::{DataPlan, Extent};
use crate::metadata::{physical_block, read_node, BlockRange};
use crate::table::{Row, TablePage};
use crate::*;

struct Memory(Vec<u8>);
impl ReadAt for Memory {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let offset = usize::try_from(offset).map_err(|_| corrupt("offset overflow"))?;
        buffer.copy_from_slice(slice(&self.0, offset, buffer.len())?);
        Ok(())
    }
}

fn shared_holder(id: u64, block: Option<u64>) -> Row {
    let mut key = vec![0; 40];
    key[8..10].copy_from_slice(&2u16.to_le_bytes());
    key[12..14].copy_from_slice(&0x80u16.to_le_bytes());
    key[16..24].copy_from_slice(&id.to_le_bytes());
    let mut value = vec![0; 208];
    value[4..8].copy_from_slice(&0x10028u32.to_le_bytes());
    value[56..64].copy_from_slice(&12u64.to_le_bytes());
    value[0x88..0x8C].copy_from_slice(&40u32.to_le_bytes());
    value[0x98..0x9C].copy_from_slice(&64u32.to_le_bytes());
    if let Some(block) = block {
        value[0x9C..0xA0].copy_from_slice(&1u32.to_le_bytes());
        value[0xB0..0xB8].copy_from_slice(&block.to_le_bytes());
        value[0xB8..0xBC].copy_from_slice(&0x50u32.to_le_bytes());
        value[0xC4..0xC8].copy_from_slice(&1u32.to_le_bytes());
        value[0xC8..0xCA].copy_from_slice(&40u16.to_le_bytes());
    }
    Row {
        key,
        value,
        inode: None,
    }
}

#[test]
fn live_cow_uses_newest_blocks_but_saved_versions_never_use_newer_blocks() -> Result<()> {
    let boot = boot()?;
    let mut bytes = vec![0; boot.volume_size as usize];
    bytes[20 * 4096..21 * 4096].fill(b'A');
    bytes[30 * 4096..31 * 4096].fill(b'B');
    let source = Memory(bytes);
    let rows = vec![
        shared_holder(0x1000, None),
        shared_holder(0x1001, Some(20)),
        shared_holder(0x1002, None),
        shared_holder(0x1003, Some(30)),
    ];
    for (id, expected) in [(0x1000, b'B'), (0x1002, b'A')] {
        let plan = DataPlan::parse_data_attributes(&rows, id, 12, &boot, &[], &source)?;
        let mut buffer = [0; 12];
        assert_eq!(
            plan.read_range(&source, &file_entry(false)?, 0, &mut buffer)?,
            12
        );
        assert_eq!(buffer, [expected; 12]);
    }
    let rows = vec![shared_holder(0x1001, None), shared_holder(0x1002, Some(30))];
    let plan = DataPlan::parse_data_attributes(&rows, 0x1001, 12, &boot, &[], &source)?;
    assert!(plan
        .read_range(&source, &file_entry(false)?, 0, &mut [0; 12])
        .is_err());
    Ok(())
}

fn boot_bytes() -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[3..7].copy_from_slice(b"ReFS");
    bytes[16..20].copy_from_slice(b"FSRS");
    bytes[24..32].copy_from_slice(&1024u64.to_le_bytes());
    bytes[32..36].copy_from_slice(&512u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&8u32.to_le_bytes());
    bytes[40..42].copy_from_slice(&[3, 14]);
    bytes
}

fn boot() -> Result<BootSector> {
    BootSector::parse(&boot_bytes(), 1024 * 512)
}

#[test]
fn long_records_use_macb_order_and_hardlinks_keep_filename_times() -> Result<()> {
    let mut value = vec![0; 120];
    for (offset, ticks) in [(40, 11u64), (48, 22), (56, 33), (64, 44)] {
        value[offset..offset + 8].copy_from_slice(&ticks.to_le_bytes());
    }
    let mut record = DirectoryRecord {
        name: "file".into(),
        entry_type: 1,
        value,
        inode: Some(9),
        filename_timestamps: None,
    };
    let entry = FileEntry::parse(record.clone(), &boot()?)?;
    assert_eq!(
        [entry.created, entry.modified, entry.changed, entry.accessed],
        [11, 22, 33, 44]
    );
    record.filename_timestamps = Some([111, 444, 222, 333]);
    let entry = FileEntry::parse(record, &boot()?)?;
    assert_eq!(
        [entry.created, entry.modified, entry.changed, entry.accessed],
        [111, 222, 333, 444]
    );
    Ok(())
}

fn allocation_table() -> Vec<u8> {
    let mut bytes = vec![0; 144];
    bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&128u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&132u32.to_le_bytes());
    bytes[40..44].copy_from_slice(&32u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&[16, 0, 8, 0]);
    bytes[50..54].copy_from_slice(&[24, 0, 8, 0]);
    bytes[56..64].copy_from_slice(b"keybytes");
    bytes[64..72].copy_from_slice(b"contents");
    bytes[128..130].copy_from_slice(&40u16.to_le_bytes());
    bytes
}

#[test]
fn boot_rejects_short_source_invalid_geometry_and_overflow() -> Result<()> {
    let bytes = boot_bytes();
    let expected = BootSector {
        major: 3,
        minor: 14,
        sector_size: 512,
        cluster_size: 4096,
        volume_size: 524288,
        serial_number: 0,
    };
    assert_eq!(BootSector::parse(&bytes, 524288)?, expected);
    for length in 0..512 {
        assert!(BootSector::parse(&bytes[..length], 524288).is_err());
    }
    assert!(BootSector::parse(&bytes, 524287).is_err());
    let mut bytes = bytes;
    bytes[36..40].copy_from_slice(&7u32.to_le_bytes());
    assert!(BootSector::parse(&bytes, 524288).is_err());
    bytes[36..40].copy_from_slice(&8u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(BootSector::parse(&bytes, u64::MAX).is_err());
    Ok(())
}

#[test]
fn table_rows_are_bounded_by_the_offsets_array() -> Result<()> {
    let boot = boot()?;
    let bytes = allocation_table();
    assert_eq!(
        TablePage::parse_allocation(&bytes, 0, 2, &boot)?.rows,
        vec![Row {
            key: b"keybytes".to_vec(),
            value: b"contents".to_vec(),
            inode: Some(40)
        }]
    );
    for length in 0..132 {
        assert!(TablePage::parse_allocation(&bytes[..length], 0, 2, &boot).is_err());
    }
    let mut bytes = bytes;
    bytes[40..44].copy_from_slice(&100u32.to_le_bytes());
    assert!(TablePage::parse_allocation(&bytes, 0, 2, &boot).is_err());
    bytes[40..44].copy_from_slice(&32u32.to_le_bytes());
    bytes[128..130].copy_from_slice(&4u16.to_le_bytes());
    assert!(TablePage::parse_allocation(&bytes, 0, 2, &boot).is_err());
    Ok(())
}

#[test]
fn mapping_rows_can_share_their_key_and_value_bytes() -> Result<()> {
    let mut bytes = allocation_table();
    bytes[50..52].copy_from_slice(&16u16.to_le_bytes());
    let table = TablePage::parse_allocation(&bytes, 0, 0xB, &boot()?)?;
    assert_eq!(
        table.rows,
        vec![Row {
            key: Vec::new(),
            value: b"keybytes".to_vec(),
            inode: Some(40)
        }]
    );
    assert_eq!(
        TablePage::parse_allocation(&bytes, 0, 0, &boot()?)?.rows,
        vec![Row {
            key: b"keybytes".to_vec(),
            value: b"keybytes".to_vec(),
            inode: Some(40)
        }]
    );
    Ok(())
}

fn extent_node(block: u64, index: bool, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 16384];
    bytes[..4].copy_from_slice(b"MSB+");
    for inset in 0..4 {
        bytes[32 + inset * 8..40 + inset * 8]
            .copy_from_slice(&(block + inset as u64).to_le_bytes());
    }
    bytes[72..80].copy_from_slice(&0x600u64.to_le_bytes());
    bytes[80..84].copy_from_slice(&8u32.to_le_bytes());
    bytes[88..92].copy_from_slice(&40u32.to_le_bytes());
    if index {
        bytes[100..104].copy_from_slice(&0x100u32.to_le_bytes());
    }
    bytes[104..108].copy_from_slice(&128u32.to_le_bytes());
    bytes[108..112].copy_from_slice(&1u32.to_le_bytes());
    bytes[120..124].copy_from_slice(&132u32.to_le_bytes());
    bytes[128..128 + payload.len()].copy_from_slice(payload);
    bytes[216..218].copy_from_slice(&40u16.to_le_bytes());
    bytes
}

fn node_index_row(block: u64) -> Vec<u8> {
    let mut bytes = vec![0; 48];
    bytes[..4].copy_from_slice(&48u32.to_le_bytes());
    bytes[4..6].copy_from_slice(&16u16.to_le_bytes());
    bytes[10..12].copy_from_slice(&16u16.to_le_bytes());
    bytes[12..14].copy_from_slice(&32u16.to_le_bytes());
    for inset in 0..4 {
        bytes[16 + inset * 8..24 + inset * 8]
            .copy_from_slice(&(block + inset as u64).to_le_bytes());
    }
    bytes
}

#[test]
fn extent_subtrees_follow_indexes_and_reject_cycles_and_short_leaves() -> Result<()> {
    let boot = boot()?;
    let mut extent = vec![0; 24];
    extent[..8].copy_from_slice(&100u64.to_le_bytes());
    extent[12..20].copy_from_slice(&3u64.to_le_bytes());
    extent[20..24].copy_from_slice(&2u32.to_le_bytes());
    let mut source = vec![0; boot.volume_size as usize];
    source[40 * 4096..44 * 4096].copy_from_slice(&extent_node(40, true, &node_index_row(50)));
    source[50 * 4096..54 * 4096].copy_from_slice(&extent_node(50, false, &extent));
    let mut list = vec![0; 92];
    list[12..16].copy_from_slice(&1u32.to_le_bytes());
    list[16..20].copy_from_slice(&88u32.to_le_bytes());
    list[20..24].copy_from_slice(&1u32.to_le_bytes());
    list[40..88].copy_from_slice(&node_index_row(40));
    list[88..90].copy_from_slice(&40u16.to_le_bytes());
    assert_eq!(
        crate::extents::parse_list(&list, &boot, &[], &Memory(source.clone()))?,
        vec![Extent {
            logical: 3 * 4096,
            physical: Some(100 * 4096),
            length: 2 * 4096
        }]
    );
    source[50 * 4096..54 * 4096].copy_from_slice(&extent_node(50, true, &node_index_row(40)));
    assert!(crate::extents::parse_list(&list, &boot, &[], &Memory(source.clone())).is_err());
    let mut leaf = extent_node(50, false, &extent);
    leaf[216..218].copy_from_slice(&112u16.to_le_bytes());
    source[50 * 4096..54 * 4096].copy_from_slice(&leaf);
    assert!(crate::extents::parse_list(&list, &boot, &[], &Memory(source)).is_err());
    Ok(())
}

#[test]
fn extents_crossing_virtual_chunks_map_each_piece_separately() -> Result<()> {
    let mut boot = boot()?;
    boot.volume_size = 131072 * 4096;
    let mappings = [
        BlockRange {
            start: 32768,
            length: 16384,
        },
        BlockRange {
            start: 65536,
            length: 16384,
        },
    ];
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(&98303u64.to_le_bytes());
    bytes[20..24].copy_from_slice(&2u32.to_le_bytes());
    let mut extents = Vec::new();
    crate::extents::append_extent(&bytes, &boot, &mappings, &mut extents)?;
    assert_eq!(
        extents,
        vec![
            Extent {
                logical: 0,
                physical: Some(49151 * 4096),
                length: 4096
            },
            Extent {
                logical: 4096,
                physical: Some(65536 * 4096),
                length: 4096
            }
        ]
    );
    Ok(())
}

#[test]
fn explicitly_unwritten_and_hole_extents_never_expose_allocated_bytes() -> Result<()> {
    let boot = boot()?;
    let source = Memory(vec![0xAB; boot.volume_size as usize]);
    for (flags, zero_filled) in [(0x40u32, true), (0x30, true), (0x50, false), (0, false)] {
        let mut bytes = [0; 24];
        bytes[..8].copy_from_slice(&100u64.to_le_bytes());
        bytes[8..12].copy_from_slice(&flags.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        let mut extents = Vec::new();
        crate::extents::append_extent(&bytes, &boot, &[], &mut extents)?;
        let plan = DataPlan::Extents(extents);
        let mut buffer = [0xCC; 16];
        assert_eq!(
            plan.read_range(&source, &file_entry(false)?, 0, &mut buffer)?,
            12
        );
        let mut expected = [0xCC; 16];
        expected[..12].fill(if zero_filled { 0 } else { 0xAB });
        assert_eq!(buffer, expected);
    }
    Ok(())
}

#[test]
fn metadata_reads_follow_fragmented_cluster_references() -> Result<()> {
    let boot = boot()?;
    let blocks = [40u64, 70, 45, 89];
    let mut source = vec![0; boot.volume_size as usize];
    let mut expected = vec![0; 16384];
    for (index, block) in blocks.iter().enumerate() {
        let payload = vec![index as u8 + 1; 4096];
        let offset = *block as usize * 4096;
        source[offset..offset + 4096].copy_from_slice(&payload);
        expected[index * 4096..(index + 1) * 4096].copy_from_slice(&payload);
    }
    for (index, block) in blocks.iter().enumerate() {
        source[40 * 4096 + 32 + index * 8..40 * 4096 + 40 + index * 8]
            .copy_from_slice(&block.to_le_bytes());
        expected[32 + index * 8..40 + index * 8].copy_from_slice(&block.to_le_bytes());
    }
    let source = Memory(source);
    assert_eq!(
        read_node(&source, &boot, &[], NodeReference { blocks })?,
        expected
    );
    assert_eq!(
        read_node(
            &source,
            &boot,
            &[],
            NodeReference {
                blocks: [40, 0, 0, 0]
            }
        )?,
        expected
    );
    assert!(read_node(
        &source,
        &boot,
        &[],
        NodeReference {
            blocks: [127, 128, 129, 130]
        }
    )
    .is_err());
    Ok(())
}

#[test]
fn virtual_mapping_preserves_chunk_insets_and_rejects_missing_regions() -> Result<()> {
    let boot = boot()?;
    let mappings = [
        BlockRange {
            start: 32768,
            length: 16384,
        },
        BlockRange {
            start: 49152,
            length: 16384,
        },
    ];
    assert_eq!(
        (
            physical_block(&boot, &mappings, 65535)?,
            physical_block(&boot, &mappings, 65536)?,
            physical_block(&boot, &mappings, 98311)?
        ),
        (65535, 32768, 49159)
    );
    assert!(physical_block(&boot, &[], 65536).is_err());
    Ok(())
}

fn file_entry(sparse: bool) -> Result<FileEntry> {
    let mut value = vec![0; 72];
    value[56..64].copy_from_slice(&12u64.to_le_bytes());
    if sparse {
        value[64..68].copy_from_slice(&0x200u32.to_le_bytes());
    }
    FileEntry::parse(
        DirectoryRecord {
            name: "file".into(),
            entry_type: 2,
            inode: None,
            filename_timestamps: None,
            value,
        },
        &boot()?,
    )
}

#[test]
fn range_reads_cross_extents_fill_sparse_holes_and_clamp_eof() -> Result<()> {
    let source = Memory(b"abcdefgh".to_vec());
    let plan = DataPlan::Extents(vec![
        Extent {
            logical: 0,
            physical: Some(0),
            length: 4,
        },
        Extent {
            logical: 8,
            physical: Some(4),
            length: 4,
        },
    ]);
    let entry = file_entry(true)?;
    let mut bytes = [0xAA; 12];
    let count = plan.read_range(&source, &entry, 2, &mut bytes)?;
    assert_eq!(
        (count, bytes),
        (
            10,
            [b'c', b'd', 0, 0, 0, 0, b'e', b'f', b'g', b'h', 0xAA, 0xAA]
        )
    );
    assert_eq!(plan.read_range(&source, &entry, 12, &mut bytes)?, 0);
    assert_eq!(plan.read_range(&source, &entry, u64::MAX, &mut bytes)?, 0);
    assert!(plan
        .read_range(&source, &file_entry(false)?, 2, &mut bytes)
        .is_err());
    let truncated = Memory(b"abcdef".to_vec());
    assert!(plan.read_range(&truncated, &entry, 8, &mut bytes).is_err());
    Ok(())
}

#[test]
fn resident_ranges_do_not_modify_bytes_past_eof() -> Result<()> {
    let source: Arc<dyn ReadAt> = Arc::new(Memory(Vec::new()));
    let plan = DataPlan::Resident(b"abcdefghijkl".to_vec());
    let mut bytes = [0xAA; 4];
    assert_eq!(
        plan.read_range(source.as_ref(), &file_entry(false)?, 10, &mut bytes)?,
        2
    );
    assert_eq!(bytes, [b'k', b'l', 0xAA, 0xAA]);
    Ok(())
}

#[test]
fn reparse_target_uses_print_name_and_enforces_payload_bounds() -> Result<()> {
    let mut value = vec![0; 40];
    value[12..16].copy_from_slice(&0xA000000Cu32.to_le_bytes());
    value[16..18].copy_from_slice(&20u16.to_le_bytes());
    value[26..28].copy_from_slice(&8u16.to_le_bytes());
    for (index, word) in "目标文件".encode_utf16().enumerate() {
        value[32 + index * 2..34 + index * 2].copy_from_slice(&word.to_le_bytes());
    }
    assert_eq!(
        crate::reparse::target(&value)?,
        Some("目标文件".to_string())
    );
    value[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(crate::reparse::target(&value).is_err());
    value[24..26].copy_from_slice(&0u16.to_le_bytes());
    for length in 0..40 {
        assert!(crate::reparse::target(&value[..length]).is_err());
    }
    Ok(())
}

#[test]
fn checkpoint_offsets_follow_the_stored_position_for_both_checksum_layouts() -> Result<()> {
    for start in [0xA8usize, 0xB0] {
        let mut bytes = vec![0; 512];
        bytes[0..4].copy_from_slice(b"CHKP");
        bytes[32..40].copy_from_slice(&10u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&123u64.to_le_bytes());
        bytes[144..148].copy_from_slice(&1u32.to_le_bytes());
        bytes[148..152].copy_from_slice(&(start as u32).to_le_bytes());
        bytes[start..start + 4].copy_from_slice(&256u32.to_le_bytes());
        bytes[256..264].copy_from_slice(&40u64.to_le_bytes());
        let expected = Checkpoint {
            block: 10,
            sequence: 123,
            roots: vec![NodeReference {
                blocks: [40, 0, 0, 0],
            }],
        };
        assert_eq!(Checkpoint::parse(&bytes, 10, &boot()?)?, expected);
    }
    Ok(())
}
