use super::super::optical_disc::{tests::fixture, DATA_BYTES, RAW_SECTOR_BYTES};
use super::*;
use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::test_support::BufSource;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, FsSession};
use std::sync::Arc;

fn xa_fixture(width: usize) -> (Vec<u8>, Vec<u8>) {
    let (raw, iso, content) = fixture();
    let mut xa = vec![0x5a; 40 * width];
    for (number, sector) in xa.chunks_exact_mut(width).enumerate() {
        let subheader = if width == 2352 {
            sector[..16].copy_from_slice(&raw[number * 2352..number * 2352 + 16]);
            sector[15] = 2;
            16
        } else {
            0
        };
        sector[subheader..subheader + 8].copy_from_slice(&[1, 0, 8, 0, 1, 0, 8, 0]);
        sector[subheader + 8..subheader + 8 + DATA_BYTES]
            .copy_from_slice(&iso[number * DATA_BYTES..(number + 1) * DATA_BYTES]);
    }
    (xa, content)
}

fn verify_file(image: DiskImage, partition_id: u8, expected: &[u8]) {
    let image = Arc::new(EvidenceSession::from(image));
    let mut session = FsSession::new(&image);
    let files = session.children(&format!("part:{partition_id}")).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "测试数据.bin");
    let id = &files[0].id;
    assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), expected);
    for (offset, length) in [(0, 0), (2043, 30), (4091, 100), (6998, 30), (7001, 1)] {
        let start = offset as usize;
        let end = (start + length).min(expected.len());
        assert_eq!(
            session.read_file_range(id, offset, length).unwrap(),
            expected[start..end]
        );
    }
    let mut streamed = Vec::new();
    fs_tree::stream_file_chunks(&image, id, 2047, |bytes| {
        streamed.extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("output.bin");
    fs_tree::export_file(&image, id, &destination).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), expected);
}

#[test]
fn cue_mixed_tracks_keep_raw_bytes_indexes_sessions_and_file_identity() {
    let (raw, iso, content) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let audio = vec![0x96; 3 * RAW_SECTOR_BYTES];
    // INDEX 00 is stored in the file; PREGAP / POSTGAP are declarations only.
    let mut first = audio.clone();
    first.extend_from_slice(&raw);
    first.extend_from_slice(&[0x71; RAW_SECTOR_BYTES]);
    std::fs::write(directory.path().join("第一 track.bin"), &first).unwrap();
    std::fs::write(directory.path().join("second.iso"), &iso).unwrap();
    let cue = directory.path().join("disc.cue");
    std::fs::write(&cue, concat!(
        "\u{feff}REM SESSION 01\r\nFILE \"第一 track.bin\" BINARY\r\n",
        " TRACK 01 AUDIO\r\n INDEX 01 00:00:00\r\n",
        " TRACK 02 MODE1/2352\r\n PREGAP 00:02:00\r\n INDEX 00 00:00:02\r\n INDEX 01 00:00:03\r\n INDEX 02 00:00:04\r\n POSTGAP 00:00:01\r\n",
        " REM RUN-OUT 00:00:43\r\n REM LEAD-OUT 00:00:44\r\n REM SESSION 02\r\n",
        "FILE second.iso BINARY\r\n TRACK 03 MODE1/2048\r\n INDEX 01 00:00:00\r\n"
    )).unwrap();
    let image = DiskImage::open(&cue).unwrap();
    assert_eq!(image.sector_size, 1);
    let expected = [first.clone(), iso.clone()].concat();
    assert_eq!(image.read_range(0, expected.len()).unwrap(), expected);
    let mut buffer = [0; 31];
    assert_eq!(
        image
            .read_into(first.len() as u64 - 7, &mut buffer)
            .unwrap(),
        31
    );
    assert_eq!(buffer, expected[first.len() - 7..first.len() + 24]);
    assert_eq!(image.len(), expected.len() as u64);
    assert_eq!(
        image.on_disk_bytes.unwrap(),
        expected.len() as u64 + std::fs::metadata(&cue).unwrap().len()
    );
    assert_eq!(
        image
            .partitions
            .iter()
            .map(|partition| partition.kind)
            .collect::<Vec<_>>(),
        [
            PartitionKind::Unknown,
            PartitionKind::Cdfs,
            PartitionKind::Cdfs
        ]
    );
    let info = image.info().optical_disc.unwrap();
    assert_eq!(
        info.tracks
            .iter()
            .map(|track| (
                track.number,
                track.session,
                track.source_offset,
                track.source_length
            ))
            .collect::<Vec<_>>(),
        [
            (1, 1, 0, (2 * RAW_SECTOR_BYTES) as u64),
            (2, 1, audio.len() as u64, raw.len() as u64),
            (3, 2, first.len() as u64, iso.len() as u64)
        ]
    );
    assert_eq!(info.tracks[1].pregap_sectors, 150);
    assert_eq!(info.tracks[1].postgap_sectors, 1);
    assert_eq!(info.tracks[1].indexes.len(), 3);
    assert_eq!(info.tracks[1].run_out_sector, Some(43));
    assert_eq!(info.lead_outs[0].file_sector, 44);
    let wire = serde_json::to_value(image.info()).unwrap();
    assert_eq!(wire["optical_disc"]["tracks"][2]["session"], 2);
    image.validate_source_file_stable().unwrap();
    verify_file(image, 1, &content);
    verify_file(DiskImage::open(&cue).unwrap(), 2, &content);
    // A second member must participate even if its metadata is never read by browsing.
    let image = DiskImage::open(&cue).unwrap();
    std::fs::write(directory.path().join("second.iso"), b"truncated").unwrap();
    assert!(image.validate_source_file_stable().is_err());
    assert!(image.read_range(first.len() as u64, 100).is_err());
}

#[test]
fn raw_cd_xa_form1_reads_cue_2336_and_standalone_2352() {
    let directory = tempfile::tempdir().unwrap();
    for width in [2336, 2352] {
        let (bytes, content) = xa_fixture(width);
        let member = format!("xa{width}.bin");
        let member_path = directory.path().join(&member);
        std::fs::write(&member_path, &bytes).unwrap();
        let cue = directory.path().join(format!("xa{width}.cue"));
        std::fs::write(
            &cue,
            format!("FILE {member} BINARY\nTRACK 01 MODE2/{width}\nINDEX 01 00:00:00\n"),
        )
        .unwrap();
        let image = DiskImage::open(&cue).unwrap();
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
        verify_file(image, 0, &content);
        if width == 2352 {
            let image = DiskImage::open(&member_path).unwrap();
            assert_eq!(image.sector_size, 2352);
            verify_file(image, 0, &content);
        }
    }
}

#[test]
fn raw_cd_xa_rejects_form2_bad_copies_channels_and_short_reads() {
    for width in [2336, 2352] {
        let (bytes, _) = xa_fixture(width);
        let source = BufSource(bytes.clone());
        let layout = if width == 2336 {
            DataTrackLayout::probe_xa2336(&source)
        } else {
            DataTrackLayout::probe(&source)
        }
        .unwrap()
        .unwrap();
        let subheader = 25 * width + if width == 2352 { 16 } else { 0 };
        for (field, value, duplicate) in [(0, 2, false), (2, 0x28, true), (1, 32, true)] {
            let mut broken = bytes.clone();
            broken[subheader + field] = value;
            if duplicate {
                broken[subheader + field + 4] = value;
            }
            assert!(layout
                .read(&BufSource(broken), 25 * DATA_BYTES as u64, 1)
                .is_err());
        }
        let mut short = bytes.clone();
        short.pop();
        assert!(layout
            .read(&BufSource(short), 39 * DATA_BYTES as u64, 1)
            .is_err());
    }
}

#[test]
fn cue_rejects_invalid_geometry_and_unsupported_layouts_without_raw_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let (raw, _, _) = fixture();
    std::fs::write(directory.path().join("data.bin"), &raw).unwrap();
    let path = directory.path().join("invalid.cue");
    for text in [
        "FILE missing.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        "FILE data.bin WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:40\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:60:00\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:75\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nINDEX 01 00:00:01\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:02\nTRACK 02 AUDIO\nINDEX 00 00:00:01\nINDEX 01 00:00:04\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 MODE1/2048\nINDEX 01 00:00:20\n",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nREM LEAD-OUT 00:00:39\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nREM RUN-OUT 00:00:41\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nREM RUN-OUT 00:00:00\n",
        "FILE data.bin BINARY\nREM LEAD-OUT 00:00:40\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE data.bin BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        "FILE \"data.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        "FILE data.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nUNKNOWN 12\n",
        "TRACK 01 AUDIO\nINDEX 01 00:00:00\n",
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(DiskImage::open(&path).is_err(), "accepted invalid CUE: {text}");
    }
    std::fs::write(
        &path,
        "FILE data.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(directory.path().join("data.bin"), &raw[..raw.len() - 1]).unwrap();
    assert!(DiskImage::open(&path).is_err());
    let token = CancellationToken::new();
    token.cancel();
    assert!(CueImage::open(&path, &token).is_err());
}

#[test]
fn cue_source_stability_and_fingerprint_include_descriptor_and_all_files() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("folder.cue");
    std::fs::create_dir(&folder).unwrap();
    assert!(DiskImage::open(&folder).unwrap().folder.is_some());
    let path = directory.path().join("audio.cue");
    std::fs::write(directory.path().join("audio.bin"), vec![0x42; 2352 * 4]).unwrap();
    let text = "FILE audio.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n";
    std::fs::write(&path, text).unwrap();
    let image = DiskImage::open(&path).unwrap();
    let fingerprint = image.evidence_fingerprint();
    std::fs::write(&path, format!("{text}TITLE \"different\"\n")).unwrap();
    assert!(image.validate_source_file_stable().is_err());
    assert_ne!(
        fingerprint,
        DiskImage::open(&path).unwrap().evidence_fingerprint()
    );
}
