use super::*;

#[path = "reparse_volume_tests.rs"]
mod reparse;

fn named_resident(ty: u32, name: &str, data: &[u8]) -> Vec<u8> {
    let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let offset = (24 + name.len()).next_multiple_of(8);
    let size = (offset + data.len()).next_multiple_of(8);
    let mut attribute = vec![0; size];
    attribute[..4].copy_from_slice(&ty.to_le_bytes());
    attribute[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    attribute[9] = (name.len() / 2) as u8;
    attribute[10..12].copy_from_slice(&24u16.to_le_bytes());
    attribute[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes());
    attribute[20..22].copy_from_slice(&(offset as u16).to_le_bytes());
    attribute[24..24 + name.len()].copy_from_slice(&name);
    attribute[offset..offset + data.len()].copy_from_slice(data);
    attribute
}

fn wof_image(algorithm: u32, expected: &[u8], chunks: &[&[u8]]) -> Vec<u8> {
    let mut image = image();
    let mut stored = Vec::new();
    let mut offset = 0u32;
    for chunk in chunks.iter().take(chunks.len() - 1) {
        offset += chunk.len() as u32;
        stored.extend(offset.to_le_bytes());
    }
    for chunk in chunks {
        stored.extend_from_slice(chunk);
    }
    let count = expected.len().div_ceil(CLUSTER);
    let mut default = non_resident(
        "",
        &[0x01, count as u8, 0],
        expected.len() as u64,
        expected.len() as u64,
        (count * CLUSTER) as u64,
    );
    default[12..14].copy_from_slice(&0x8000u16.to_le_bytes());
    default[34] = 0;
    let count = stored.len().div_ceil(CLUSTER);
    let mut encoded = non_resident(
        "WofCompressedData",
        &[0x11, count as u8, 32, 0],
        stored.len() as u64,
        stored.len() as u64,
        (count * CLUSTER) as u64,
    );
    encoded[12..14].fill(0);
    encoded[34] = 0;
    encoded[14..16].copy_from_slice(&1u16.to_le_bytes());
    let mut reparse = vec![0x17, 0, 0, 0x80, 16, 0, 0, 0];
    for word in [1u32, 2, 1, algorithm] {
        reparse.extend(word.to_le_bytes());
    }
    record(
        &mut image,
        16,
        &[default, encoded, resident(0xc0, &reparse)],
    );
    image[32 * CLUSTER..32 * CLUSTER + stored.len()].copy_from_slice(&stored);
    image
}

fn assert_all_reads(bytes: &[u8], expected: &[u8]) {
    let (_file, image) = open_image(bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), expected);
    assert_eq!(volume.read_file(16, 37).unwrap(), expected[..37]);
    let mut streamed = Vec::new();
    assert_eq!(
        volume
            .stream_file(16, 777, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap(),
        expected.len() as u64
    );
    assert_eq!(streamed, expected);
    for (start, length) in [
        (0, 16),
        (4091, 20),
        (16380, 100),
        (expected.len() - 7, 30),
        (expected.len(), 1),
        (usize::MAX, 4),
    ] {
        assert_eq!(
            volume.read_range(16, start as u64, length).unwrap(),
            expected
                .get(start..expected.len().min(start.saturating_add(length)))
                .unwrap_or_default()
        );
    }
    let sample = volume.file_stat(16, "test").unwrap();
    assert_eq!(sample.size, expected.len() as u64);
    assert_eq!(
        sample.magic_hex.unwrap(),
        expected[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

#[test]
fn windows_xpress_4k_8k_16k_wof_chunks_support_every_read_path() {
    for (algorithm, size, encoded) in [
        (
            0,
            4096,
            include_bytes!("testdata/wof_xpress_4096.bin").as_slice(),
        ),
        (
            2,
            8192,
            include_bytes!("testdata/wof_xpress_8192.bin").as_slice(),
        ),
        (
            3,
            16384,
            include_bytes!("testdata/wof_xpress_16384.bin").as_slice(),
        ),
    ] {
        let expected = b"LovelyCarve WOF XPRESS oracle 0123456789\n".repeat(1000)[..size].to_vec();
        let mut whole = expected.clone();
        whole.extend(vec![b'R'; size]); // A stored, uncompressed chunk.
        whole.extend(&expected);
        let raw = vec![b'R'; size];
        let bytes = wof_image(algorithm, &whole, &[encoded, &raw, encoded]);
        assert_all_reads(&bytes, &whole);
    }
}

#[test]
fn independent_wimlib_lzx_full_and_partial_wof_chunks() {
    let pattern = b"native WIM LZX compressed block\n";
    let mut whole = pattern.repeat(1200)[..32768].to_vec();
    whole.extend(&pattern.repeat(200)[..4096]);
    let bytes = wof_image(
        1,
        &whole,
        &[
            include_bytes!("testdata/wof_lzx_32768.bin"),
            include_bytes!("testdata/wof_lzx_4096.bin"),
        ],
    );
    assert_all_reads(&bytes, &whole);
}

#[test]
fn corrupt_wof_offsets_missing_stream_and_unknown_algorithm_fail() {
    let expected = vec![b'Z'; 8192];
    for kind in 0..3 {
        let mut bytes = wof_image(
            if kind == 2 { 7 } else { 0 },
            &expected,
            &[&expected[..4096], &expected[4096..]],
        );
        if kind == 0 {
            bytes[32 * CLUSTER..32 * CLUSTER + 4].fill(0xff);
        }
        if kind == 1 {
            let mut reparse = vec![0x17, 0, 0, 0x80, 16, 0, 0, 0];
            for word in [1u32, 2, 1, 0] {
                reparse.extend(word.to_le_bytes());
            }
            record(
                &mut bytes,
                16,
                &[resident(0x80, &expected[..20]), resident(0xc0, &reparse)],
            );
        }
        let (_file, image) = open_image(&bytes);
        let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
        assert!(volume.read_file(16, usize::MAX).is_err());
        assert!(volume.read_range(16, 0, 10).is_err());
        assert!(volume.stream_file(16, 1, |_| Ok(())).is_err());
    }
}

fn list_entry(number: u64, vcn: u64, instance: u16, name: &str) -> Vec<u8> {
    let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let size = (32 + name.len()).next_multiple_of(8);
    let mut entry = vec![0; size];
    entry[..4].copy_from_slice(&0x80u32.to_le_bytes());
    entry[4..6].copy_from_slice(&(size as u16).to_le_bytes());
    entry[6] = (name.len() / 2) as u8;
    entry[7] = 32; // Exercise padded name offsets rather than assuming 26.
    entry[8..16].copy_from_slice(&vcn.to_le_bytes());
    entry[16..24].copy_from_slice(&(number | (1 << 48)).to_le_bytes());
    entry[24..26].copy_from_slice(&instance.to_le_bytes());
    entry[32..32 + name.len()].copy_from_slice(&name);
    entry
}

#[test]
fn attribute_list_joins_different_instances_and_preserves_other_ads() {
    let mut bytes = super::image();
    let mut entries = list_entry(16, 0, 3, "secret");
    entries.extend(list_entry(17, 1, 7, "secret"));
    entries.extend(list_entry(17, 0, 8, "z"));
    let mut first = non_resident("secret", &[0x11, 1, 32, 0], 6000, 6000, 8192);
    first[12..14].fill(0);
    first[34] = 0;
    first[14..16].copy_from_slice(&3u16.to_le_bytes());
    first[24..32].fill(0);
    let mut continuation = non_resident("secret", &[0x11, 1, 40, 0], 0, 0, 0);
    continuation[12..14].fill(0);
    continuation[34] = 0;
    continuation[14..16].copy_from_slice(&7u16.to_le_bytes());
    continuation[16..24].copy_from_slice(&1u64.to_le_bytes());
    let mut other = named_resident(0x80, "z", b"different stream");
    other[14..16].copy_from_slice(&8u16.to_le_bytes());
    record(&mut bytes, 16, &[resident(0x20, &entries), first]);
    record(&mut bytes, 17, &[continuation, other]);
    bytes[32 * CLUSTER..33 * CLUSTER].fill(b'A');
    bytes[40 * CLUSTER..41 * CLUSTER].fill(b'B');
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut expected = vec![b'A'; 4096];
    expected.extend(vec![b'B'; 1904]);
    assert_eq!(
        volume.read_data_stream(16, "secret", usize::MAX).unwrap(),
        expected
    );
    assert_eq!(
        volume
            .read_data_stream_range(16, "secret", 4090, 20)
            .unwrap(),
        expected[4090..4110]
    );
    assert_eq!(
        volume.read_data_stream(16, "z", usize::MAX).unwrap(),
        b"different stream"
    );
    assert_eq!(
        volume
            .list_data_streams(16)
            .unwrap()
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["secret", "z"]
    );
}

#[test]
fn mft_bootstrap_resolves_extension_runs_without_recursive_lookup() {
    let mut bytes = super::image();
    record(
        &mut bytes,
        16,
        &[resident(0x80, b"mapped MFT continuation")],
    );
    let tail = bytes[MFT + 16 * 1024..MFT + 32 * 1024].to_vec();
    bytes[32 * CLUSTER..36 * CLUSTER].copy_from_slice(&tail);
    let mut first = non_resident("", &[0x11, 4, 4, 0], 32768, 32768, 32768);
    first[12..14].fill(0);
    first[34] = 0;
    first[24..32].copy_from_slice(&3u64.to_le_bytes());
    let mut continuation = non_resident("", &[0x11, 4, 32, 0], 0, 0, 0);
    continuation[12..14].fill(0);
    continuation[34] = 0;
    continuation[14..16].copy_from_slice(&7u16.to_le_bytes());
    continuation[16..24].copy_from_slice(&4u64.to_le_bytes());
    continuation[24..32].copy_from_slice(&7u64.to_le_bytes());
    let mut entries = list_entry(0, 0, 0, "");
    entries.extend(list_entry(8, 4, 7, ""));
    let mut list = resident(0x20, &entries);
    list[14..16].copy_from_slice(&1u16.to_le_bytes());
    record(&mut bytes, 0, &[list, first]);
    record(&mut bytes, 8, &[continuation]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(
        volume.read_file(16, usize::MAX).unwrap(),
        b"mapped MFT continuation"
    );
    assert_eq!(volume.mft_total_records().unwrap(), 32);
}

fn filename(name: &str, namespace: u8) -> Vec<u8> {
    let names: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut bytes = vec![0; 66];
    bytes[..8].copy_from_slice(&(5u64 | (1 << 48)).to_le_bytes());
    bytes[64] = (names.len() / 2) as u8;
    bytes[65] = namespace;
    bytes.extend(names);
    resident(0x30, &bytes)
}

#[test]
fn same_parent_hardlinks_and_case_distinct_names_are_preserved() {
    let mut bytes = super::image();
    record(
        &mut bytes,
        16,
        &[
            filename("A.txt", 1),
            filename("a.txt", 0),
            filename("A~1.TXT", 2),
            resident(0x80, b"shared"),
        ],
    );
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut edges = Vec::new();
    volume.collect_record_edges(&mut volume.reader(), 16, &mut edges, &mut Vec::new());
    let mut names: Vec<_> = edges.iter().map(|edge| edge.node.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["A.txt", "a.txt"]);
    assert!(edges
        .iter()
        .all(|edge| edge.parent_record_number == 5 && edge.node.inode == Some(16)));
}

#[test]
fn damaged_upcase_table_keeps_exact_ads_reads_available() {
    let mut bytes = super::image();
    record(&mut bytes, 10, &[]);
    record(
        &mut bytes,
        16,
        &[named_resident(0x80, "Secret", b"evidence")],
    );
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(
        volume.read_data_stream(16, "Secret", 99).unwrap(),
        b"evidence"
    );
    assert!(volume.read_data_stream(16, "secret", 99).is_err());
}

#[test]
fn zero_lcn_delta_is_allocated_data_not_a_sparse_run() {
    let mut bytes = super::image();
    let mut attribute = non_resident("", &[0x11, 1, 32, 0x11, 1, 0, 0], 8192, 8192, 8192);
    attribute[12..14].fill(0);
    attribute[34] = 0;
    record(&mut bytes, 16, &[attribute]);
    bytes[32 * CLUSTER..33 * CLUSTER].fill(b'R');
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), vec![b'R'; 8192]);
}

#[test]
fn truncated_physical_data_and_attribute_headers_fail_without_panicking() {
    let mut bytes = super::image();
    let mut attribute = non_resident("", &[0x11, 1, 127, 0], 4096, 4096, 4096);
    attribute[12..14].fill(0);
    attribute[34] = 0;
    record(&mut bytes, 16, &[attribute]);
    bytes.truncate(127 * CLUSTER + 20);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert!(volume.read_file(16, usize::MAX).is_err());
    let mut bytes = super::image();
    let mut malformed = vec![0; 16];
    malformed[..4].copy_from_slice(&0x80u32.to_le_bytes());
    malformed[4..8].copy_from_slice(&16u32.to_le_bytes());
    malformed[8] = 1;
    record(&mut bytes, 16, &[malformed]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert!(volume.read_file(16, 10).is_err());
}

#[test]
fn metadata_preserves_si_fn_times_names_extents_and_raw_values() {
    let mut bytes = super::image();
    let mut info = vec![0; 72];
    let time = 132_000_000_000_000_123u64;
    info[..8].copy_from_slice(&time.to_le_bytes());
    info[48..52].copy_from_slice(&42u32.to_le_bytes());
    let mut standard = resident(0x10, &info);
    standard[14..16].copy_from_slice(&3u16.to_le_bytes());
    let mut data = non_resident("", &[0x11, 1, 32, 0x01, 1, 0], 6000, 5000, 8192);
    data[12..14].copy_from_slice(&0x8000u16.to_le_bytes());
    data[34] = 0;
    data[14..16].copy_from_slice(&7u16.to_le_bytes());
    record(
        &mut bytes,
        16,
        &[standard, filename("evidence.txt", 1), data],
    );
    let mut volume = vec![0; 12];
    volume[8] = 3;
    volume[9] = 1;
    record(&mut bytes, 3, &[resident(0x70, &volume)]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let metadata = volume.record_metadata(16).unwrap();
    assert_eq!(metadata.standard_filetimes, Some([time, 0, 0, 0]));
    assert_eq!(metadata.owner_id, Some(42));
    assert_eq!(metadata.filenames[0].filetimes, [0; 4]);
    let extents = &metadata.attributes.last().unwrap().extents;
    assert_eq!(
        extents
            .iter()
            .map(|extent| (
                extent.logical_offset,
                extent.storage_length,
                extent.volume_offset
            ))
            .collect::<Vec<_>>(),
        [(0, 4096, Some(32 * CLUSTER as u64)), (4096, 4096, None)]
    );
    assert_eq!(volume.read_attribute_value(16, 3, 72).unwrap(), info);
    assert_eq!(volume.volume_metadata().unwrap().version, (3, 1));
    let mut exported = tempfile::NamedTempFile::new().unwrap();
    exported.write_all(&bytes[MFT..MFT + 32768]).unwrap();
    let mut mft = NtfsMftMetadataFile::open(exported.path()).unwrap();
    assert_eq!(mft.record_count(), 32);
    assert_eq!(mft.volume_metadata().unwrap().version, Some((3, 1)));
    let standalone = mft.record_metadata(16).unwrap();
    assert_eq!(standalone.standard_filetimes, metadata.standard_filetimes);
    assert_eq!(standalone.filenames[0].name, "evidence.txt");
    assert_eq!(mft.read_resident_attribute(16, 3, 72).unwrap(), info);
    assert!(mft.read_resident_attribute(16, 7, 10000).is_err());
    assert!(mft.record_metadata(32).is_err());
}

#[test]
fn modern_security_descriptors_resolve_sii_and_sds_with_bounds() {
    let mut bytes = super::image();
    let mut descriptor = vec![0; 32];
    descriptor[0] = 1;
    descriptor[3] = 0x80;
    descriptor[4..8].copy_from_slice(&20u32.to_le_bytes());
    descriptor[20..32].copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]);
    let id = 256u32;
    let mut location = 0x1234u32.to_le_bytes().to_vec();
    location.extend(id.to_le_bytes());
    location.extend(0u64.to_le_bytes());
    location.extend(52u32.to_le_bytes());
    let mut sds = location.clone();
    sds.extend(&descriptor);
    let mut entry = vec![0; 48];
    entry[..2].copy_from_slice(&20u16.to_le_bytes());
    entry[2..4].copy_from_slice(&20u16.to_le_bytes());
    entry[8..10].copy_from_slice(&48u16.to_le_bytes());
    entry[10..12].copy_from_slice(&4u16.to_le_bytes());
    entry[16..20].copy_from_slice(&id.to_le_bytes());
    entry[20..40].copy_from_slice(&location);
    let mut root = vec![0; 32];
    root[4..8].copy_from_slice(&0x10u32.to_le_bytes());
    root[8..12].copy_from_slice(&4096u32.to_le_bytes());
    root[12] = 1;
    root[16..20].copy_from_slice(&16u32.to_le_bytes());
    root[20..24].copy_from_slice(&80u32.to_le_bytes());
    root[24..28].copy_from_slice(&80u32.to_le_bytes());
    root.extend(entry);
    let mut end = vec![0; 16];
    end[8..10].copy_from_slice(&16u16.to_le_bytes());
    end[12] = 2;
    root.extend(end);
    record(
        &mut bytes,
        9,
        &[
            named_resident(0x90, "$SII", &root),
            named_resident(0x80, "$SDS", &sds),
        ],
    );
    let mut info = vec![0; 72];
    info[52..56].copy_from_slice(&id.to_le_bytes());
    record(&mut bytes, 16, &[resident(0x10, &info)]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(
        volume.security_descriptor(16, 1024).unwrap(),
        Some(descriptor.clone())
    );
    assert!(volume.security_descriptor(16, 16).is_err());
    record(&mut bytes, 16, &[resident(0x50, &descriptor)]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(
        volume.security_descriptor(16, 1024).unwrap(),
        Some(descriptor)
    );
}

#[test]
fn cyclic_index_fails_both_enumeration_and_lookup() {
    let mut bytes = super::image();
    let mut end = vec![0; 24];
    end[8..10].copy_from_slice(&24u16.to_le_bytes());
    end[12] = 3;
    let mut root = vec![0; 32];
    root[..4].copy_from_slice(&0x30u32.to_le_bytes());
    root[4..8].copy_from_slice(&1u32.to_le_bytes());
    root[8..12].copy_from_slice(&4096u32.to_le_bytes());
    root[12] = 1;
    root[16..20].copy_from_slice(&16u32.to_le_bytes());
    root[20..24].copy_from_slice(&40u32.to_le_bytes());
    root[24..28].copy_from_slice(&40u32.to_le_bytes());
    root[28] = 1;
    root.extend(&end);
    let mut allocation = non_resident("$I30", &[0x11, 1, 40, 0], 4096, 4096, 4096);
    allocation[..4].copy_from_slice(&0xa0u32.to_le_bytes());
    allocation[12..14].fill(0);
    allocation[34] = 0;
    record(
        &mut bytes,
        5,
        &[named_resident(0x90, "$I30", &root), allocation],
    );
    bytes[MFT + 5 * 1024 + 22..MFT + 5 * 1024 + 24].copy_from_slice(&3u16.to_le_bytes());
    let block = &mut bytes[40 * CLUSTER..41 * CLUSTER];
    block[..4].copy_from_slice(b"INDX");
    block[4..6].copy_from_slice(&40u16.to_le_bytes());
    block[6..8].copy_from_slice(&9u16.to_le_bytes());
    block[24..28].copy_from_slice(&40u32.to_le_bytes());
    block[28..32].copy_from_slice(&64u32.to_le_bytes());
    block[32..36].copy_from_slice(&4072u32.to_le_bytes());
    block[36] = 1;
    block[40..42].copy_from_slice(&[0xaa, 0x55]);
    block[64..88].copy_from_slice(&end);
    for index in 1..=8 {
        block[index * 512 - 2..index * 512].copy_from_slice(&[0xaa, 0x55]);
    }
    let mut reader = std::io::Cursor::new(bytes);
    let ntfs = Ntfs::new(&mut reader).unwrap();
    let file = ntfs.file(&mut reader, 5).unwrap();
    let index = file.directory_index(&mut reader).unwrap();
    assert!(index.entries().next(&mut reader).unwrap().is_err());
    assert!(index
        .finder()
        .find(&mut reader, |_| std::cmp::Ordering::Less)
        .unwrap()
        .is_err());
}

#[test]
fn malformed_runs_return_one_error_and_terminate() {
    let mut bytes = super::image();
    let mut data = non_resident("", &[0x11, 1, 32, 0xf1, 1, 0], 8192, 8192, 8192);
    data[12..14].fill(0);
    data[34] = 0;
    record(&mut bytes, 16, &[data]);
    let mut reader = std::io::Cursor::new(bytes);
    let ntfs = Ntfs::new(&mut reader).unwrap();
    let file = ntfs.file(&mut reader, 16).unwrap();
    let item = file.data(&mut reader, "").unwrap().unwrap();
    let attribute = item.to_attribute().unwrap();
    let ntfs::attribute_value::NtfsAttributeValue::NonResident(value) =
        attribute.value(&mut reader).unwrap()
    else {
        panic!("expected nonresident data")
    };
    let mut runs = value.data_runs();
    assert!(runs.next().unwrap().is_ok());
    assert!(runs.next().unwrap().is_err());
    assert!(runs.next().is_none());
}

#[test]
fn deleted_snapshot_retains_same_parent_links_and_never_collapses_sparse_holes() {
    let mut bytes = super::image();
    let mut data = non_resident("", &[0x11, 1, 32, 0x01, 1, 0], 6000, 6000, 8192);
    data[12..14].copy_from_slice(&0x8000u16.to_le_bytes());
    data[34] = 0;
    record(
        &mut bytes,
        16,
        &[filename("first.txt", 1), filename("second.txt", 1), data],
    );
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].fill(0);
    bytes[32 * CLUSTER..33 * CLUSTER].fill(b'A');
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let snapshot = volume.build_deleted_snapshot().unwrap();
    let mut names: Vec<_> = snapshot
        .for_parent(5)
        .map(|entry| entry.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["first.txt", "second.txt"]);
    assert!(snapshot
        .for_parent(5)
        .all(|entry| entry.data_runs.is_empty()));
    assert_eq!(
        volume.read_range(16, 4090, 20).unwrap(),
        [vec![b'A'; 6], vec![0; 14]].concat()
    );
}
