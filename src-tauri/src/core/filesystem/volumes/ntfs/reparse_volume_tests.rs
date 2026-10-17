//! Reparse lookup must not resolve unrelated or obsolete attribute references.
use super::*;
use crate::core::filesystem::volumes::ntfs::reparse::read_reparse;

#[test]
fn deleted_unrelated_stale_list_does_not_break_reparse_lookup() {
    let mut bytes = image();
    record(
        &mut bytes,
        16,
        &[
            resident(0x20, &list_entry(16, 0, 1, "")),
            resident(0x80, b"old data"),
        ],
    );
    // Deletion leaves an older self-reference in the attribute list.
    bytes[MFT + 16 * 1024 + 16..MFT + 16 * 1024 + 18].copy_from_slice(&2u16.to_le_bytes());
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].fill(0);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut fs = volume.reader();
    let file = volume.ntfs.file(&mut fs, 16).unwrap();
    assert!(read_reparse(&file, &mut fs).unwrap().is_none());
    assert!(volume.read_file(16, 99).is_err());
}

#[test]
fn deleted_non_reparse_files_do_not_read_freed_attribute_lists() {
    for flags in [0u32, 0x400] {
        let mut bytes = image();
        let mut info = vec![0; 72];
        info[32..36].copy_from_slice(&flags.to_le_bytes());
        let mut list = non_resident("", &[0x11, 1, 32, 0], 200, 200, 4096);
        list[..4].copy_from_slice(&0x20u32.to_le_bytes());
        list[12..14].fill(0);
        list[24..32].fill(0);
        list[34] = 0;
        record(&mut bytes, 16, &[resident(0x10, &info), list]);
        bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].fill(0);
        let (_file, image) = open_image(&bytes);
        let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
        let mut fs = volume.reader();
        let file = volume.ntfs.file(&mut fs, 16).unwrap();
        let result = read_reparse(&file, &mut fs);
        if flags == 0 {
            assert!(result.unwrap().is_none());
        } else {
            // A reparse flag is evidence to inspect, even if its list is bad.
            assert!(result.is_err());
        }
        assert!(file.attributes().attach(&mut fs).any(|item| item.is_err()));
    }
}

#[test]
fn reparse_in_extension_checks_only_relevant_references() {
    for sequence in [1u64, 2] {
        let mut bytes = image();
        // Unrelated data points to a stale generation of the base record.
        let mut entries = list_entry(16, 0, 0, "");
        entries[16..24].copy_from_slice(&(16 | (2u64 << 48)).to_le_bytes());
        let mut point_entry = list_entry(17, 0, 3, "");
        point_entry[..4].copy_from_slice(&0xc0u32.to_le_bytes());
        point_entry[16..24].copy_from_slice(&(17 | (sequence << 48)).to_le_bytes());
        entries.extend(point_entry);
        record(&mut bytes, 16, &[resident(0x20, &entries)]);
        let mut point = vec![0; 24];
        point[..4].copy_from_slice(&0x8000_0023u32.to_le_bytes());
        let mut attribute = resident(0xc0, &point);
        attribute[14..16].copy_from_slice(&3u16.to_le_bytes());
        record(&mut bytes, 17, &[attribute]);
        let (_file, image) = open_image(&bytes);
        let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
        let mut fs = volume.reader();
        let file = volume.ntfs.file(&mut fs, 16).unwrap();
        let result = read_reparse(&file, &mut fs);
        if sequence == 1 {
            assert_eq!(result.unwrap().unwrap().tag, 0x8000_0023);
        } else {
            assert!(matches!(
                result.unwrap_err().downcast_ref::<ntfs::NtfsError>(),
                Some(ntfs::NtfsError::StaleFileReference { .. })
            ));
        }
    }
}

#[test]
fn local_reparse_is_not_blocked_by_an_unrelated_stale_list() {
    let mut bytes = image();
    let mut entry = list_entry(17, 0, 0, "");
    entry[16..24].copy_from_slice(&(17 | (2u64 << 48)).to_le_bytes());
    let mut point = vec![0; 24];
    point[..4].copy_from_slice(&0x8000_0023u32.to_le_bytes());
    record(
        &mut bytes,
        16,
        &[
            resident(0x10, &[0; 72]),
            resident(0x20, &entry),
            resident(0xc0, &point),
            filename("socket", 1),
        ],
    );
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].fill(0);
    record(&mut bytes, 17, &[resident(0x80, b"unrelated")]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut fs = volume.reader();
    let file = volume.ntfs.file(&mut fs, 16).unwrap();
    assert_eq!(
        read_reparse(&file, &mut fs).unwrap().unwrap().tag,
        0x8000_0023
    );
    let mut edges = Vec::new();
    volume.collect_record_edges(&mut fs, 16, &mut edges, &mut Vec::new());
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].node.name, "socket");
    assert_eq!(edges[0].node.kind, NodeKind::File);
}

#[test]
fn split_nonresident_reparse_keeps_connected_extents() {
    let mut bytes = image();
    let mut unrelated = list_entry(16, 0, 0, "");
    unrelated[16..24].copy_from_slice(&(16 | (2u64 << 48)).to_le_bytes());
    let mut entries = unrelated;
    for (number, vcn, instance) in [(16, 0, 3), (17, 1, 7)] {
        let mut entry = list_entry(number, vcn, instance, "");
        entry[..4].copy_from_slice(&0xc0u32.to_le_bytes());
        entries.extend(entry);
    }
    let mut first = non_resident("", &[0x11, 1, 32, 0], 6000, 6000, 8192);
    first[..4].copy_from_slice(&0xc0u32.to_le_bytes());
    first[12..14].fill(0);
    first[14..16].copy_from_slice(&3u16.to_le_bytes());
    first[24..32].fill(0);
    first[34] = 0;
    let mut continuation = non_resident("", &[0x11, 1, 40, 0], 0, 0, 0);
    continuation[..4].copy_from_slice(&0xc0u32.to_le_bytes());
    continuation[12..14].fill(0);
    continuation[14..16].copy_from_slice(&7u16.to_le_bytes());
    continuation[16..24].copy_from_slice(&1u64.to_le_bytes());
    continuation[24..32].copy_from_slice(&1u64.to_le_bytes());
    continuation[34] = 0;
    record(&mut bytes, 16, &[resident(0x20, &entries), first]);
    record(&mut bytes, 17, &[continuation]);
    bytes[32 * CLUSTER..33 * CLUSTER].fill(0x42);
    bytes[40 * CLUSTER..41 * CLUSTER].fill(0x43);
    bytes[32 * CLUSTER..32 * CLUSTER + 8].copy_from_slice(&[0x23, 0, 0, 0x80, 0x68, 0x17, 0, 0]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut fs = volume.reader();
    let file = volume.ntfs.file(&mut fs, 16).unwrap();
    let point = read_reparse(&file, &mut fs).unwrap().unwrap();
    assert_eq!(point.data, [vec![0x42; 4088], vec![0x43; 1904]].concat());
}

#[test]
fn padded_wof_reparse_keeps_all_logical_reads_consistent() {
    let expected = b"LovelyCarve WOF XPRESS oracle 0123456789\n".repeat(1000)[..4096].to_vec();
    let mut bytes = wof_image(
        0,
        &expected,
        &[include_bytes!("testdata/wof_xpress_4096.bin")],
    );
    // The WOF fixture's reparse attribute is last in record 16.
    let mut offset = MFT + 16 * 1024 + 56;
    while u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) != 0xc0 {
        offset += u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
    }
    bytes[offset + 4..offset + 8].copy_from_slice(&64u32.to_le_bytes());
    bytes[offset + 16..offset + 20].copy_from_slice(&40u32.to_le_bytes());
    bytes[offset + 48..offset + 64].fill(0xa5);
    bytes[offset + 64..offset + 68].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[MFT + 16 * 1024 + 24..MFT + 16 * 1024 + 28]
        .copy_from_slice(&((offset + 68 - MFT - 16 * 1024) as u32).to_le_bytes());
    assert_all_reads(&bytes, &expected);
}
