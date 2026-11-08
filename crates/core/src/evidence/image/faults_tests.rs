//! Sector fault isolation and container audit regressions.

use super::*;

#[test]
fn tolerant_fill_isolates_errors_anywhere_in_the_request() {
    const SECTORS: usize = 4096;
    const SIZE: usize = SECTORS * FAULT_GRANULARITY;
    let source: Vec<_> = (0..SIZE).map(|index| (index % 251 + 1) as u8).collect();
    for bad_sectors in [vec![0], vec![2048], vec![2048, 2049], vec![SECTORS - 1]] {
        // Unaligned endpoints exercise clipping to absolute sector boundaries.
        for (start, length) in [(0, SIZE), (123, SIZE - 246)] {
            let mut output = vec![0xaa; length];
            let mut calls = 0;
            let mut requested_bytes = 0;
            let outcome = tolerant_fill(start as u64, &mut output, |offset, buffer| {
                calls += 1;
                requested_bytes += buffer.len();
                let offset = offset as usize;
                // Mimic read_exact backends that copy bytes before failing,
                // without returning how much of the buffer was overwritten.
                buffer.fill(0xbb);
                if bad_sectors.iter().any(|sector| {
                    offset < (sector + 1) * FAULT_GRANULARITY
                        && offset + buffer.len() > sector * FAULT_GRANULARITY
                }) {
                    anyhow::bail!("request overlaps unreadable sector");
                }
                buffer.copy_from_slice(&source[offset..offset + buffer.len()]);
                Ok(buffer.len())
            });
            let mut expected = source[start..start + length].to_vec();
            let mut faults = Vec::new();
            for sector in &bad_sectors {
                let from = (sector * FAULT_GRANULARITY).max(start);
                let to = ((sector + 1) * FAULT_GRANULARITY).min(start + length);
                expected[from - start..to - start].fill(0);
                faults.push(BadRegion {
                    offset: from as u64,
                    len: (to - from) as u64,
                });
            }
            assert_eq!(outcome, (length, normalize_bad_regions(faults)));
            assert_eq!(output, expected);
            // Sparse faults should take logarithmic retries, not one large
            // suffix read for every preceding sector (and quadratic I/O).
            assert!(calls <= 1 + 2 * bad_sectors.len() * 12, "{calls} calls");
            assert!(
                requested_bytes <= SIZE * 4,
                "{requested_bytes} requested bytes"
            );
        }
    }
}

#[test]
fn tolerant_fill_keeps_short_reads_and_eof_after_bisection() {
    let mut output = vec![0xaa; 4096];
    let outcome = tolerant_fill(0, &mut output, |offset, buffer| {
        if offset < 512 && offset + buffer.len() as u64 > 512 {
            anyhow::bail!("request crosses bad sector");
        }
        if (512..1024).contains(&offset) {
            anyhow::bail!("bad sector");
        }
        let count = buffer
            .len()
            .min(73)
            .min(1500usize.saturating_sub(offset as usize));
        buffer[..count].fill(0x11);
        Ok(count)
    });
    assert_eq!(
        outcome,
        (
            1500,
            vec![BadRegion {
                offset: 512,
                len: 512
            }]
        )
    );
    let mut expected = vec![0x11; 1500];
    expected[512..1024].fill(0);
    expected.resize(4096, 0xaa);
    assert_eq!(output, expected);
}

#[test]
fn truncated_flat_vmdk_audits_only_missing_sectors_for_bulk_and_cached_reads() {
    const SIZE: usize = 2 * 1024 * 1024;
    // Test both the reported 1.5 MiB truncation and a truncation inside a
    // 64 KiB cache block, where readable bytes must survive a block miss.
    for extent_len in [1536 * 1024, 1536 * 1024 + 512] {
        let directory = tempfile::tempdir().expect("temporary directory");
        let bytes: Vec<_> = (0..extent_len)
            .map(|index| (index % 251 + 1) as u8)
            .collect();
        std::fs::write(directory.path().join("probe.bin"), &bytes).expect("write flat extent");
        let path = directory.path().join("probe.vmdk");
        std::fs::write(
            &path,
            "# Disk DescriptorFile\ncreateType=\"monolithicFlat\"\nRW 4096 FLAT \"probe.bin\" 0\n",
        )
        .expect("write descriptor");

        let image = DiskImage::open(&path).expect("open VMDK");
        assert!(image.block_cache.is_some());
        // Hash/RAW export reuse a 16 MiB buffer and bypass the block cache.
        let mut output = vec![0xaa; 16 * 1024 * 1024];
        let (filled, report) =
            image.collect_read_faults(|| image.read_into(0, &mut output).expect("bulk read"));
        assert_eq!(filled, SIZE);
        let mut expected = bytes;
        expected.resize(SIZE, 0);
        assert_eq!(&output[..SIZE], expected);
        assert!(output[SIZE..].iter().all(|byte| *byte == 0xaa));
        let regions = vec![BadRegion {
            offset: extent_len as u64,
            len: (SIZE - extent_len) as u64,
        }];
        assert_eq!(report, ReadFaultReport::from_regions(regions.clone()));
        assert_eq!(image.bad_regions(), regions);

        // A fresh image exercises discovery on a cache miss, then reporting
        // on a hit; the operation report clips faults to the requested bytes.
        let cached = DiskImage::open(&path).expect("reopen VMDK");
        let start = extent_len - 256;
        for _ in 0..2 {
            let (output, report) = cached.collect_read_faults(|| {
                cached.read_range(start as u64, 1024).expect("cached read")
            });
            assert_eq!(output, expected[start..start + 1024]);
            assert_eq!(
                report,
                ReadFaultReport::from_regions(vec![BadRegion {
                    offset: extent_len as u64,
                    len: 768
                }])
            );
        }
    }
}

#[test]
fn tolerant_fill_bounds_retries_for_an_entirely_unreadable_span() {
    const SECTORS: usize = 1024;
    let mut output = vec![0xaa; SECTORS * FAULT_GRANULARITY];
    let mut calls = 0;
    let mut requested_bytes = 0;
    let outcome = tolerant_fill(0, &mut output, |_, buffer| {
        calls += 1;
        requested_bytes += buffer.len();
        anyhow::bail!("unreadable source");
    });
    assert_eq!(
        outcome,
        (
            output.len(),
            vec![BadRegion {
                offset: 0,
                len: output.len() as u64
            }]
        )
    );
    assert!(output.iter().all(|byte| *byte == 0));
    assert_eq!(calls, 2 * SECTORS - 1);
    assert_eq!(requested_bytes, output.len() * 11);
}

#[test]
fn corrupt_ewf_chunk_preserves_its_readable_neighbors() {
    use flate2::{write::ZlibEncoder, Compression};
    const CHUNK: usize = 32768;
    let source: Vec<_> = (0..CHUNK * 3)
        .map(|index| (index % 251 + 1) as u8)
        .collect();
    let mut stored = Vec::new();
    for bytes in source.chunks(CHUNK) {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("compress fixture chunk");
        stored.push(encoder.finish().expect("finish fixture chunk"));
    }
    // Destroy the middle chunk's zlib header, leaving its table and neighbors
    // intact. The reader must fail even for a one-sector request in this chunk.
    stored[1][0] = 0;
    let mut file = ewf::EVF_SIGNATURE.to_vec();
    file.extend_from_slice(&[1, 1, 0, 0, 0]);
    let mut section = |kind: &str, body: &[u8]| {
        let size = 76 + body.len();
        let next = file.len() + if kind == "done" { 0 } else { size };
        let mut descriptor = [0; 76];
        descriptor[..kind.len()].copy_from_slice(kind.as_bytes());
        descriptor[16..24].copy_from_slice(&(next as u64).to_le_bytes());
        descriptor[24..32].copy_from_slice(&(size as u64).to_le_bytes());
        let checksum = ewf::sections::adler32(&descriptor[..72]);
        descriptor[72..].copy_from_slice(&checksum.to_le_bytes());
        file.extend_from_slice(&descriptor);
        file.extend_from_slice(body);
    };
    let mut volume = [0; 1052];
    volume[0] = 1;
    volume[4..8].copy_from_slice(&3u32.to_le_bytes());
    volume[8..12].copy_from_slice(&64u32.to_le_bytes());
    volume[12..16].copy_from_slice(&512u32.to_le_bytes());
    volume[16..24].copy_from_slice(&192u64.to_le_bytes());
    volume[52] = 1;
    let checksum = ewf::sections::adler32(&volume[..1048]);
    volume[1048..].copy_from_slice(&checksum.to_le_bytes());
    section("volume", &volume);
    let data_offset = 13 + 76 + volume.len() + 76;
    section("sectors", &stored.concat());
    let mut table = vec![0; 24];
    table[..4].copy_from_slice(&3u32.to_le_bytes());
    table[8..16].copy_from_slice(&(data_offset as u64).to_le_bytes());
    let checksum = ewf::sections::adler32(&table[..20]);
    table[20..24].copy_from_slice(&checksum.to_le_bytes());
    let mut offset = 0u32;
    for chunk in &stored {
        table.extend_from_slice(&(0x8000_0000 | offset).to_le_bytes());
        offset += chunk.len() as u32;
    }
    let checksum = ewf::sections::adler32(&table[24..]);
    table.extend_from_slice(&checksum.to_le_bytes());
    section("table", &table);
    section("table2", &table);
    section("done", &[]);
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("corrupt.E01");
    std::fs::write(&path, file).expect("write EWF fixture");
    let image = DiskImage::open(&path).expect("open EWF fixture");
    let mut expected = source;
    expected[CHUNK..CHUNK * 2].fill(0);
    let mut output = vec![0xaa; 16 * 1024 * 1024];
    let (filled, report) =
        image.collect_read_faults(|| image.read_into(0, &mut output).expect("bulk EWF read"));
    assert_eq!(
        (filled, &output[..filled]),
        (expected.len(), expected.as_slice())
    );
    assert_eq!(
        report,
        ReadFaultReport::from_regions(vec![BadRegion {
            offset: CHUNK as u64,
            len: CHUNK as u64
        }])
    );
    assert_eq!(
        image
            .read_range(0, expected.len())
            .expect("cached EWF read"),
        expected
    );
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
