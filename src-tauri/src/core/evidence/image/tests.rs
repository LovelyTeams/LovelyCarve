//! Image adapter, cache and integrity regressions.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;
use std::time::Duration;
use tempfile::NamedTempFile;

#[test]
fn bitlocker_source_rejects_audited_synthetic_ciphertext() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&[0; 4096]).unwrap();
    file.flush().unwrap();
    let image = DiskImage::open(file.path()).unwrap();
    image.record_faults(vec![BadRegion {
        offset: 512,
        len: 512,
    }]);
    let mut output = [0; 512];
    assert_eq!(
        VolumeByteReader::read_at(&image, 0, &mut output).unwrap(),
        512
    );
    let error = VolumeByteReader::read_at(&image, 512, &mut output).unwrap_err();
    assert!(error.to_string().contains("synthesized source bytes"));
}

#[test]
fn opens_raw_gpt_with_crc_validated_geometry_and_backup_recovery() {
    for sector_size in [512, 1024, 2048, 4096] {
        for populated in [0, 1] {
            for wipe_primary in [false, true] {
                let mut bytes = partitions::gpt::fixture(sector_size, 4, 128, populated);
                if wipe_primary {
                    bytes[sector_size..2 * sector_size].fill(0);
                }
                let mut file = NamedTempFile::new().expect("GPT fixture");
                file.write_all(&bytes).expect("write fixture");
                file.flush().expect("flush fixture");
                let image = DiskImage::open(file.path()).expect("open GPT image");
                assert_eq!(image.sector_size, sector_size);
                assert_eq!(image.table_scheme, Some(TableScheme::Gpt));
                assert_eq!(
                    image.disk_id.as_deref(),
                    Some("12345678-1122-3344-5566-778899AABBCC")
                );
                assert_eq!(image.partitions.len(), populated);
                if populated != 0 {
                    assert_eq!(image.partitions[0].start_lba, 200);
                    assert_eq!(image.partitions[0].length_lba, 1);
                }
            }
        }
    }
}

#[test]
fn mbr_geometry_inference_does_not_override_a_detected_gpt() {
    let mut bytes = vec![0; 200 * 512];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    bytes[450] = 0x0f;
    bytes[454..458].copy_from_slice(&16u32.to_le_bytes());
    bytes[458..462].copy_from_slice(&8u32.to_le_bytes());
    // An EBR valid only at 4 KiB geometry must not supersede the GPT at 512.
    let ebr = 16 * 4096;
    bytes[ebr + 510..ebr + 512].copy_from_slice(&[0x55, 0xaa]);
    bytes[ebr + 450] = 0x83;
    bytes[ebr + 454..ebr + 458].copy_from_slice(&1u32.to_le_bytes());
    bytes[ebr + 458..ebr + 462].copy_from_slice(&4u32.to_le_bytes());
    bytes[512..520].copy_from_slice(b"EFI PART");
    bytes[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
    bytes[512 + 80..512 + 84].copy_from_slice(&1u32.to_le_bytes());
    bytes[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
    bytes[1024..1040].fill(1);
    bytes[1024 + 32..1024 + 40].copy_from_slice(&100u64.to_le_bytes());
    bytes[1024 + 40..1024 + 48].copy_from_slice(&199u64.to_le_bytes());
    let mut file = NamedTempFile::new().expect("fixture");
    file.write_all(&bytes).expect("write fixture");
    file.flush().expect("flush fixture");
    let image = DiskImage::open(file.path()).expect("open GPT image");
    assert_eq!(
        (
            image.sector_size,
            image.table_scheme,
            image.partitions[0].start_lba
        ),
        (512, Some(TableScheme::Gpt), 100)
    );
}

#[test]
fn opens_raw_mbr_with_primary_and_large_sector_logical_partitions() {
    for sector_size in [512, 1024, 2048, 4096] {
        let mut bytes = vec![0u8; sector_size * 64];
        bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
        bytes[440..444].copy_from_slice(&0x1234abcd_u32.to_le_bytes());
        bytes[450] = 0x17;
        bytes[454..458].copy_from_slice(&2u32.to_le_bytes());
        bytes[458..462].copy_from_slice(&4u32.to_le_bytes());
        bytes[466] = 0x0f;
        bytes[470..474].copy_from_slice(&16u32.to_le_bytes());
        bytes[474..478].copy_from_slice(&48u32.to_le_bytes());
        let ebr = 16 * sector_size;
        bytes[ebr + 510..ebr + 512].copy_from_slice(&[0x55, 0xaa]);
        bytes[ebr + 450] = 0x11;
        bytes[ebr + 454..ebr + 458].copy_from_slice(&1u32.to_le_bytes());
        bytes[ebr + 458..ebr + 462].copy_from_slice(&4u32.to_le_bytes());
        let mut file = NamedTempFile::new().expect("fixture");
        file.write_all(&bytes).expect("write fixture");
        file.flush().expect("flush fixture");
        let image = DiskImage::open(file.path()).expect("open MBR image");
        let info = image.info();
        assert_eq!(
            (
                info.sector_size,
                info.table_scheme,
                info.disk_id,
                info.partitions
            ),
            (
                sector_size,
                Some(TableScheme::Mbr),
                Some("0x1234ABCD".into()),
                vec![
                    Partition {
                        id: 0,
                        // Zero-filled payloads have no filesystem signature;
                        // refinement reports Unknown while retaining geometry.
                        kind: PartitionKind::Unknown,
                        start_lba: 2,
                        length_lba: 4,
                        label: None,
                        unique_guid: None
                    },
                    Partition {
                        id: 1,
                        kind: PartitionKind::Unknown,
                        start_lba: 17,
                        length_lba: 4,
                        label: None,
                        unique_guid: None
                    },
                ]
            )
        );
    }
}

#[test]
fn flat_vmdk_finds_adjacent_descriptor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let flat_path = temp.path().join("A06-cl1-flat.vmdk");
    let descriptor_path = temp.path().join("A06-cl1.vmdk");
    std::fs::write(&flat_path, [0u8; 512]).expect("write flat extent");
    std::fs::write(
        &descriptor_path,
        r#"
# Disk DescriptorFile
version=1
createType="monolithicFlat"

# Extent description
RW 83886080 FLAT "A06-cl1-flat.vmdk" 0
"#,
    )
    .expect("write descriptor");

    assert_eq!(
        companion_descriptor_for_flat_extent(&flat_path),
        Some(descriptor_path)
    );
}

#[test]
fn flat_vmdk_scans_nonstandard_descriptor_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let flat_path = temp.path().join("case-flat.vmdk");
    let descriptor_path = temp.path().join("descriptor.vmdk");
    std::fs::write(&flat_path, [0u8; 512]).expect("write flat extent");
    std::fs::write(
        &descriptor_path,
        r#"
# Disk DescriptorFile
version=1
createType="monolithicFlat"

# Extent description
RW 2048 FLAT "CASE-FLAT.VMDK" 0
"#,
    )
    .expect("write descriptor");

    assert_eq!(
        companion_descriptor_for_flat_extent(&flat_path),
        Some(descriptor_path)
    );
}

#[test]
fn flat_vmdk_without_descriptor_opens_as_raw_vmdk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let flat_path = temp.path().join("case-flat.vmdk");
    std::fs::write(&flat_path, [7u8; 512]).expect("write flat extent");

    let source = ImageSource::open(&flat_path).expect("open flat extent");

    assert_eq!(source.format(), ImageFormat::Vmdk);
    assert_eq!(source.len(), 512);
    assert_eq!(source.read_range(0, 4).expect("read bytes"), vec![7; 4]);
}

#[test]
fn xiaomi_firmware_opens_with_byte_addressed_volume_offsets() {
    const SEGMENT_HEADER_OFFSET: usize = 56;
    const SEGMENT_PAYLOAD_OFFSET: usize = SEGMENT_HEADER_OFFSET + 48;
    const PAYLOAD_LEN: usize = 4096;
    const SIGNATURE_OFFSET: usize = SEGMENT_PAYLOAD_OFFSET + PAYLOAD_LEN;

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("speaker.bin");
    let mut bytes = vec![0u8; SIGNATURE_OFFSET + 32];
    bytes[..4].copy_from_slice(b"HDR1");
    bytes[4..8].copy_from_slice(&(SIGNATURE_OFFSET as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&(SEGMENT_HEADER_OFFSET as u32).to_le_bytes());
    bytes[SEGMENT_HEADER_OFFSET..SEGMENT_HEADER_OFFSET + 4].copy_from_slice(&[0xBE, 0xBA, 0, 0]);
    bytes[SEGMENT_HEADER_OFFSET + 8..SEGMENT_HEADER_OFFSET + 12]
        .copy_from_slice(&(PAYLOAD_LEN as u32).to_le_bytes());
    bytes[SEGMENT_HEADER_OFFSET + 16..SEGMENT_HEADER_OFFSET + 26].copy_from_slice(b"rootfs.img");
    bytes[SEGMENT_PAYLOAD_OFFSET..SEGMENT_PAYLOAD_OFFSET + 4].copy_from_slice(b"hsqs");
    std::fs::write(&path, bytes).expect("write Xiaomi firmware fixture");

    let image = DiskImage::open(&path).expect("open Xiaomi firmware");
    let info = image.info();

    assert_eq!(info.source_format, ImageFormat::XiaomiFirmware);
    assert_eq!(info.sector_size, 1);
    assert_eq!(info.table_scheme, Some(TableScheme::Firmware));
    assert_eq!(
        info.partitions,
        vec![Partition {
            id: 0,
            kind: PartitionKind::Squashfs,
            start_lba: SEGMENT_PAYLOAD_OFFSET as u64,
            length_lba: PAYLOAD_LEN as u64,
            label: Some("rootfs.img".into()),
            unique_guid: None,
        }]
    );
}

#[test]
fn monolithic_sparse_vmdk_uses_native_reader() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("case.vmdk");
    std::fs::write(&path, synth_monolithic_sparse_vmdk()).expect("write sparse vmdk");

    let image = DiskImage::open(&path).expect("open sparse vmdk");

    assert!(matches!(image.source, ImageSource::Vmdk { .. }));
    assert_eq!(image.info().source_format, ImageFormat::Vmdk);
    assert_eq!(image.len(), 1024);
    assert_eq!(
        image.read_range(3, 8).expect("read NTFS marker"),
        b"NTFS    "
    );
    assert_eq!(image.partitions.len(), 1);
    assert_eq!(image.partitions[0].kind, PartitionKind::Ntfs);
}

#[test]
fn qcow_supports_all_versions_compression_and_legacy_aes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/qcow2/tests/data");
    let plaintext = std::fs::read(fixture_root.join("plain.bin")).expect("plaintext oracle");
    for version in 1..=3 {
        let path = temp.path().join(format!("v{version}.bin"));
        std::fs::copy(fixture_root.join(format!("v{version}-deflate.qcow")), &path)
            .expect("copy fixture");
        let source = ImageSource::open(&path).expect("signature-detected compressed QCOW");
        assert_eq!(source.format(), ImageFormat::Qcow2);
        assert_eq!(
            source.read_range(3970, 200).expect("cross cluster read"),
            [plaintext[3970..].to_vec(), vec![0; 74]].concat()
        );
        std::fs::copy(fixture_root.join(format!("v{version}-aes.qcow")), &path)
            .expect("copy fixture");
        assert!(ImageSource::open(&path).is_err());
        let image = DiskImage::open_with_qcow_key(
            &path,
            qcow2::EncryptionKey::from_password("qcow-test").expect("ASCII key"),
        )
        .expect("open encrypted QCOW");
        assert_eq!(
            image.read_range(4601, 555).expect("unaligned plaintext"),
            plaintext[505..1060]
        );
    }
}

#[test]
fn qcow_backing_is_used_by_host_and_missing_parent_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("child.qcow2");
    let mut child = vec![0; 512 * 3];
    child[..4].copy_from_slice(b"QFI\xfb");
    child[4..8].copy_from_slice(&2u32.to_be_bytes());
    child[8..16].copy_from_slice(&256u64.to_be_bytes());
    child[16..20].copy_from_slice(&10u32.to_be_bytes());
    child[20..24].copy_from_slice(&9u32.to_be_bytes());
    child[24..32].copy_from_slice(&1024u64.to_be_bytes());
    child[36..40].copy_from_slice(&1u32.to_be_bytes());
    child[40..48].copy_from_slice(&512u64.to_be_bytes());
    child[256..266].copy_from_slice(b"parent.raw");
    std::fs::write(&path, &child).expect("child fixture");
    assert!(ImageSource::open(&path).is_err());
    std::fs::write(temp.path().join("parent.raw"), vec![0x42; 1024]).expect("RAW parent");
    let source = ImageSource::open(&path).expect("backing-aware source");
    assert_eq!(
        source.read_range(509, 31).expect("backing data"),
        vec![0x42; 31]
    );
}

#[test]
fn qcow2_uses_logical_guest_stream() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("case.qcow2");
    std::fs::write(
        &path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/qcow2/tests/test.qcow2"
        )),
    )
    .expect("write qcow2 fixture");

    let source = ImageSource::open(&path).expect("open qcow2");
    assert_eq!(source.format(), ImageFormat::Qcow2);
    assert_eq!(source.len(), 1000 * 1024 * 1024);
    assert_eq!(
        source
            .read_range(200 * 1024 * 1024, 11)
            .expect("read guest bytes"),
        b"Lorem ipsum"
    );
}

#[test]
fn vdi_uses_logical_guest_stream_and_magic_detection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("case.bin");
    std::fs::write(&path, synth_vdi()).expect("write VDI fixture");

    let source = ImageSource::open(&path).expect("open VDI");
    assert_eq!(source.format(), ImageFormat::Vdi);
    assert_eq!(source.len(), 1024);
    assert_eq!(source.sector_size(), 512);
    assert_eq!(
        source.read_range(510, 8).expect("read guest bytes"),
        vec![8, 9, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn open_sector_source_reads_across_cached_block_boundaries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("cache.raw");
    let bytes = (0..CACHE_BLOCK as usize * 2)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(&path, &bytes).expect("write cache fixture");
    let source = open_raw_mmap(&path, ImageFormat::Raw).expect("open raw fixture");
    let cache = SharedBlockCache::new(2);
    let bad_regions = Mutex::new(Vec::new());
    let probe_source = OpenSectorSource {
        source: &source,
        cache: Some(&cache),
        bad_regions: &bad_regions,
    };

    let offset = CACHE_BLOCK - 3;
    assert_eq!(
        probe_source.read(offset, 8).expect("cross-block read"),
        bytes[offset as usize..offset as usize + 8]
    );
    assert_eq!(
        cache
            .state
            .lock()
            .slots
            .iter()
            .filter(|slot| slot.is_some())
            .count(),
        2
    );
    assert!(bad_regions.lock().expect("bad-region lock").is_empty());
}

fn synth_vdi() -> Vec<u8> {
    const MAP_OFFSET: usize = 512;
    const DATA_OFFSET: usize = 1024;
    const BLOCK_SIZE: usize = 512;

    let mut bytes = vec![0_u8; DATA_OFFSET + BLOCK_SIZE];
    bytes[..0x40].fill(b' ');
    bytes[64..68].copy_from_slice(&vdi::header::VdiHeader::SIGNATURE.to_le_bytes());
    bytes[68..72].copy_from_slice(&vdi::header::VdiHeader::VERSION.to_le_bytes());
    bytes[72..76].copy_from_slice(&vdi::header::VdiHeader::MINIMUM_BODY_SIZE.to_le_bytes());
    bytes[76..80].copy_from_slice(&1_u32.to_le_bytes());
    bytes[340..344].copy_from_slice(&(MAP_OFFSET as u32).to_le_bytes());
    bytes[344..348].copy_from_slice(&(DATA_OFFSET as u32).to_le_bytes());
    bytes[360..364].copy_from_slice(&512_u32.to_le_bytes());
    bytes[368..376].copy_from_slice(&(2 * BLOCK_SIZE as u64).to_le_bytes());
    bytes[376..380].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
    bytes[384..388].copy_from_slice(&2_u32.to_le_bytes());
    bytes[388..392].copy_from_slice(&1_u32.to_le_bytes());
    bytes[MAP_OFFSET..MAP_OFFSET + 4].copy_from_slice(&0_u32.to_le_bytes());
    bytes[MAP_OFFSET + 4..MAP_OFFSET + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    for (index, byte) in bytes[DATA_OFFSET..].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    bytes
}

fn synth_monolithic_sparse_vmdk() -> Vec<u8> {
    const SECTOR: usize = 512;
    let grain_sectors = 8u64;
    let num_gtes = 4u64;
    let capacity = 2u64;
    let gd_sector = 1u64;
    let gt_sector = 2u64;
    let grain0_sector = 3u64;
    let mut data = vec![0u8; 11 * SECTOR];

    data[0..4].copy_from_slice(b"KDMV");
    data[4..8].copy_from_slice(&1u32.to_le_bytes());
    data[8..12].copy_from_slice(&3u32.to_le_bytes());
    data[12..20].copy_from_slice(&capacity.to_le_bytes());
    data[20..28].copy_from_slice(&grain_sectors.to_le_bytes());
    data[44..48].copy_from_slice(&(num_gtes as u32).to_le_bytes());
    data[56..64].copy_from_slice(&gd_sector.to_le_bytes());

    let gd = gd_sector as usize * SECTOR;
    data[gd..gd + 4].copy_from_slice(&(gt_sector as u32).to_le_bytes());
    let gt = gt_sector as usize * SECTOR;
    data[gt..gt + 4].copy_from_slice(&(grain0_sector as u32).to_le_bytes());
    let grain = grain0_sector as usize * SECTOR;
    data[grain + 3..grain + 11].copy_from_slice(b"NTFS    ");
    data
}

#[test]
fn tolerant_fill_zeroes_bad_sector_in_reused_buffer() {
    const START: u64 = 4096;
    let bad_sector = START + FAULT_GRANULARITY as u64;
    // A dirty buffer, as a hash / export loop reuses between chunks.
    let mut buf = vec![0xAA; FAULT_GRANULARITY * 3];
    let outcome = tolerant_fill(START, &mut buf, |offset, dst| {
        if offset == bad_sector {
            anyhow::bail!("unreadable sector");
        }
        let read = dst.len().min(FAULT_GRANULARITY);
        dst[..read].fill(0x11);
        Ok(read)
    });

    let mut expected = vec![0x11; FAULT_GRANULARITY * 3];
    expected[FAULT_GRANULARITY..FAULT_GRANULARITY * 2].fill(0);
    assert_eq!(
        outcome,
        (
            FAULT_GRANULARITY * 3,
            vec![BadRegion {
                offset: bad_sector,
                len: FAULT_GRANULARITY as u64,
            }],
        )
    );
    assert_eq!(buf, expected);
}

#[test]
fn block_cache_hits_share_the_cached_allocation() {
    let mut cache = BlockCache::new(32);
    let data: Arc<[u8]> = vec![1, 2, 3, 4].into();
    cache.insert(0, Arc::clone(&data));
    let hit = cache.get(0).expect("cache hit");
    assert!(Arc::ptr_eq(&data, &hit));
}

#[test]
fn block_cache_capacity_config_maps_mib_to_slots() {
    assert_eq!(
        normalize_block_cache_capacity_mib(None),
        DEFAULT_CACHE_CAPACITY_MIB
    );
    assert_eq!(normalize_block_cache_capacity_mib(Some("4")), 4);
    assert_eq!(
        normalize_block_cache_capacity_mib(Some("0")),
        MIN_CACHE_CAPACITY_MIB
    );
    assert_eq!(
        normalize_block_cache_capacity_mib(Some("999")),
        MAX_CACHE_CAPACITY_MIB
    );
    assert_eq!(
        normalize_block_cache_capacity_mib(Some("not-a-number")),
        DEFAULT_CACHE_CAPACITY_MIB
    );
    assert_eq!(block_cache_slot_count_from_mib(1), 16);
    assert_eq!(
        block_cache_slot_count_from_mib(DEFAULT_CACHE_CAPACITY_MIB),
        32
    );
}

#[test]
fn shared_block_cache_single_flights_concurrent_miss() {
    const THREADS: usize = 8;
    let cache = Arc::new(SharedBlockCache::new(32));
    let fetches = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(THREADS));

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let cache = Arc::clone(&cache);
            let fetches = Arc::clone(&fetches);
            let start = Arc::clone(&start);
            handles.push(scope.spawn(move || {
                start.wait();
                let (data, faults) = cache
                    .get_or_insert_with(17, || {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        Ok((
                            vec![7u8, 7, 7, 7].into(),
                            vec![BadRegion { offset: 17, len: 4 }],
                        ))
                    })
                    .expect("cache fetch should succeed");
                assert_eq!(&data[..], &[7, 7, 7, 7]);
                faults
            }));
        }

        let fault_batches = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker should not panic"))
            .collect::<Vec<_>>();

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(
            fault_batches
                .iter()
                .filter(|faults| !faults.is_empty())
                .count(),
            1
        );
    });
}

#[test]
fn bad_region_report_coalesces_overlaps_and_adjacency() {
    let report = ReadFaultReport::from_regions(vec![
        BadRegion {
            offset: 1024,
            len: 512,
        },
        BadRegion {
            offset: 0,
            len: 512,
        },
        BadRegion {
            offset: 512,
            len: 512,
        },
        BadRegion {
            offset: 1280,
            len: 512,
        },
        BadRegion {
            offset: 4096,
            len: 0,
        },
    ]);

    assert_eq!(
        report.bad_regions,
        vec![BadRegion {
            offset: 0,
            len: 1792,
        }]
    );
    assert_eq!(report.synthetic_bytes, 1792);
    assert!(report.read_faults_detected);
}

#[test]
fn source_file_stability_detects_size_and_timestamp_changes() {
    let opened = SourceFileSnapshot {
        path: PathBuf::from(r"C:\evidence\case.E01"),
        len: 8192,
        modified_unix_ms: Some(1000),
        created_unix_ms: Some(500),
    };
    validate_source_file_snapshots(&opened, &opened).expect("same snapshot should validate");

    let mut truncated = opened.clone();
    truncated.len = 4096;
    let error = validate_source_file_snapshots(&opened, &truncated)
        .expect_err("size change should fail")
        .to_string();
    assert!(error.contains("源镜像文件已变化"));
    assert!(error.contains("case.E01"));
    assert!(error.contains("大小"));

    let mut modified = opened.clone();
    modified.modified_unix_ms = Some(2000);
    let error = validate_source_file_snapshots(&opened, &modified)
        .expect_err("mtime change should fail")
        .to_string();
    assert!(error.contains("修改时间"));
}

#[test]
fn read_fault_intersections_are_clipped_to_requested_range() {
    let regions = vec![
        BadRegion {
            offset: 100,
            len: 100,
        },
        BadRegion {
            offset: 300,
            len: 50,
        },
    ];

    assert_eq!(
        collect_read_fault_intersections(&regions, 150, 175),
        vec![
            BadRegion {
                offset: 150,
                len: 50,
            },
            BadRegion {
                offset: 300,
                len: 25,
            },
        ],
    );
}

#[test]
fn opens_real_e01_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_E01_TEST") else {
        return;
    };

    let image = DiskImage::open(path).expect("E01 image should open");
    let info = image.info();
    assert_eq!(info.source_format, ImageFormat::Ewf);
    assert!(info.size_bytes > 0);
    assert_eq!(
        image
            .read_range(0, 512)
            .expect("first sector should read")
            .len(),
        512
    );
}

/// Regression guard for the docker-vs-plain dispatch reorder: a real
/// phone / filesystem `.tar` (set `LOVELYCARVE_TAR_TEST`) must still open as
/// `ImageFormat::Tar` — not get misrouted to Docker or fall through to RAW —
/// after `DiskImage::open` switched to building `TarImage` first and probing
/// docker markers from its index (which also removed the redundant second
/// full header scan).
#[test]
fn opens_real_tar_as_tar_format_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_TAR_TEST") else {
        return;
    };
    let image = DiskImage::open(&path).expect("tar image should open");
    let info = image.info();
    assert_eq!(info.source_format, ImageFormat::Tar);
    assert!(info.size_bytes > 0);
}

#[test]
fn detects_only_first_ewf_segment_extensions() {
    assert!(is_ewf_extension("E01"));
    assert!(is_ewf_extension("ex01"));
    assert!(!is_ewf_extension("E02"));
    assert!(!is_ewf_extension("exe"));
}

#[test]
fn detects_pmfx_by_signature() {
    let mut file = NamedTempFile::new().expect("temp file");
    file.write_all(b"PMFX").expect("write PMFX signature");

    assert!(should_open_as_pmfx(file.path()).expect("PMFX signature probe"));
}

#[test]
fn opens_real_vmdk_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_VMDK_TEST") else {
        return;
    };

    let image = DiskImage::open(path).expect("VMDK image should open");
    let info = image.info();
    assert_eq!(info.source_format, ImageFormat::Vmdk);
    assert!(info.size_bytes > 0);
    assert_eq!(
        image
            .read_range(0, 512)
            .expect("first sector should read")
            .len(),
        512
    );
}

#[test]
fn opens_real_vhdi_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_VHDI_TEST") else {
        return;
    };

    let image = DiskImage::open(&path).expect("VHD/VHDX image should open");
    let info = image.info();
    let extension = Path::new(&path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    assert_eq!(
        info.source_format,
        if extension.as_deref() == Some("vhdx") {
            ImageFormat::Vhdx
        } else {
            ImageFormat::Vhd
        }
    );
    assert!(info.size_bytes > 0);
    assert!(matches!(info.sector_size, 512 | 4096));
    assert_eq!(
        image
            .read_range(0, 512)
            .expect("first sector should read")
            .len(),
        512
    );
}

#[test]
fn opens_real_pmfx_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_PMFX_TEST") else {
        return;
    };

    let image = DiskImage::open(path).expect("PMFX image should open");
    let info = image.info();
    assert_eq!(info.source_format, ImageFormat::Pmfx);
    assert!(info.size_bytes > 0);
    assert!(matches!(info.sector_size, 512 | 4096));
    assert_eq!(
        image
            .read_range(0, 512)
            .expect("first sector should read")
            .len(),
        512
    );
}

#[test]
fn detects_vhdx_signature_bytes() {
    let mut file = NamedTempFile::new().expect("temp file");
    file.write_all(b"vhdxfile").expect("write sig");

    assert!(has_vhdx_signature(file.path()).expect("signature probe"));
}

/// Wrap a `Vec<u8>` as a `SectorSource` and confirm `from_assembled`
/// produces a `DiskImage` whose `read_range` returns the original
/// bytes. This is the contract that `raid_auto_detect` relies on when
/// it hands a `Raid0Source` over to the IPC layer.
#[test]
fn from_assembled_round_trips_reads() {
    struct Mem(Vec<u8>);
    impl partitions::SectorSource for Mem {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = (start + len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }
    let bytes: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
    let source: Arc<dyn partitions::SectorSource + Send + Sync> = Arc::new(Mem(bytes.clone()));
    let image = DiskImage::from_assembled(
        "test:zero".into(),
        source,
        vec![Partition {
            id: 0,
            kind: PartitionKind::Btrfs,
            start_lba: 0,
            length_lba: 16,
            label: Some("test".into()),
            unique_guid: None,
        }],
        512,
    )
    .expect("from_assembled must succeed for a non-empty source");

    assert_eq!(image.len(), 8192);
    assert_eq!(image.info().source_format, ImageFormat::Assembled);
    let head = image.read_range(0, 1024).expect("read prefix");
    assert_eq!(head, bytes[..1024]);
    let mid = image.read_range(4096, 1024).expect("read middle");
    assert_eq!(mid, bytes[4096..5120]);
    let mut mid_into = vec![0u8; 1024];
    let mid_into_len = image
        .read_into(4096, &mut mid_into)
        .expect("read middle into caller buffer");
    assert_eq!(mid_into_len, 1024);
    assert_eq!(mid_into, bytes[4096..5120]);
    let tail = image.read_range(8000, 256).expect("read past end");
    assert_eq!(tail, bytes[8000..8192]); // clamped to image size
    let mut tail_into = vec![0u8; 256];
    let tail_into_len = image
        .read_into(8000, &mut tail_into)
        .expect("read tail into caller buffer");
    assert_eq!(tail_into_len, 192);
    assert_eq!(&tail_into[..tail_into_len], &bytes[8000..8192]);
}
