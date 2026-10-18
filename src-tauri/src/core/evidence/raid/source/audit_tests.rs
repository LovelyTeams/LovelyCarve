//! Read-fault audit of assembled devices: member faults reach the assembled
//! image at array offsets through the `DiskImage` paths the hash and RAW export
//! workflows use (the image registry and the operation-scoped collection).

use std::ops::Range;
use std::sync::Arc;

use super::test_support::{encode_striped, pattern, FaultyMember};
use super::*;
use crate::core::evidence::image::ReadFaultReport;

fn member_image(label: &str, bytes: Vec<u8>, bad: Vec<Range<u64>>) -> Arc<DiskImage> {
    let source = Arc::new(FaultyMember { bytes, bad });
    let image = DiskImage::from_assembled(label.into(), source, Vec::new(), 512);
    Arc::new(image.expect("member image"))
}

fn whole(image: &Arc<DiskImage>) -> StripedMember {
    StripedMember {
        reader: Arc::new(WholeDiskMember {
            image: Arc::clone(image),
            byte_offset: 0,
            byte_length: image.len(),
        }),
        data_offset_bytes: 0,
        data_size_bytes: image.len(),
    }
}

fn assemble(label: &str, source: Arc<dyn SectorSource + Send + Sync>) -> DiskImage {
    DiskImage::from_assembled(label.into(), source, Vec::new(), 512).expect("assembled image")
}

/// Read the whole image inside one collection scope, the way RAW export
/// reads each chunk.
fn export(image: &DiskImage) -> (Vec<u8>, ReadFaultReport) {
    let mut buffer = vec![0u8; image.len() as usize];
    let (read, report) = image.collect_read_faults(|| image.read_into(0, &mut buffer));
    assert_eq!(read.expect("read"), buffer.len());
    (buffer, report)
}

fn region(offset: u64, len: u64) -> BadRegion {
    BadRegion { offset, len }
}

#[test]
fn raid0_reports_member_faults_at_array_offsets() {
    let chunk = 512;
    let logical = pattern(chunk * 2 * 8, 1);
    let members = encode_striped(RaidLevel::Raid0, &logical, 2, chunk);
    // Member 1 holds the odd chunks: member offset 1536 + 100 is row 3, which
    // is array chunk 7.
    let images = [
        member_image("m0", members[0].clone(), Vec::new()),
        member_image("m1", members[1].clone(), vec![1636..1700]),
    ];
    let slots = images.iter().map(|image| Some(whole(image))).collect();
    let array = RaidVolume::from_slots(RaidLevel::Raid0, 512, chunk as u64, None, slots);
    let assembled = assemble("raid0", Arc::new(array));
    let expected = vec![region(7 * 512 + 100, 64)];

    let (bytes, report) = export(&assembled);
    assert_eq!(report, ReadFaultReport::from_regions(expected.clone()));
    assert_eq!(report.synthetic_bytes, 64);
    let mut expected_bytes = logical;
    expected_bytes[7 * 512 + 100..7 * 512 + 164].fill(0);
    assert_eq!(bytes, expected_bytes);

    // The source hash reads the image registry and checks strict reads
    // against it; the member offset must not show up as an array offset.
    assert_eq!(assembled.bad_regions(), expected);
    assert!(assembled.has_bad_region_in_range(7 * 512, 512));
    assert!(!assembled.has_bad_region_in_range(1536, 512));
    // The member image keeps its own record.
    assert_eq!(images[1].bad_regions(), vec![region(1636, 64)]);
}

#[test]
fn raid1_serves_member_faults_from_the_other_mirror() {
    let data = pattern(4096, 9);
    let images = [
        member_image("a", data.clone(), vec![1000..1100]),
        member_image("b", data.clone(), Vec::new()),
    ];
    let slots = images.iter().map(|image| Some(whole(image))).collect();
    let mirror = MirrorVolume::new(slots, 4096).expect("mirror");
    let assembled = assemble("raid1", Arc::new(mirror));

    let (bytes, report) = export(&assembled);
    assert_eq!(bytes, data);
    assert_eq!(report, ReadFaultReport::default());
    assert!(assembled.bad_regions().is_empty());
    assert_eq!(images[0].bad_regions(), vec![region(1000, 100)]);
}

/// Left-symmetric RAID5 over three members: row 0 is "D0 D1 P".
#[test]
fn raid5_rebuilds_member_faults_and_reports_what_parity_cannot_cover() {
    let chunk = 512;
    let level = RaidLevel::Raid5(Raid5Layout::LeftSymmetric);
    let logical = pattern(chunk * 2 * 4, 4);
    let members = encode_striped(level, &logical, 3, chunk);
    let images: Vec<_> = members
        .iter()
        .enumerate()
        .map(|(slot, bytes)| {
            let bad = if slot == 0 {
                vec![100..164]
            } else {
                Vec::new()
            };
            member_image(&format!("m{slot}"), bytes.clone(), bad)
        })
        .collect();

    let slots = images.iter().map(|image| Some(whole(image))).collect();
    let complete = RaidVolume::from_slots(level, 512, chunk as u64, None, slots);
    let (bytes, report) = export(&assemble("raid5", Arc::new(complete)));
    assert_eq!(bytes, logical);
    assert_eq!(report, ReadFaultReport::default());

    // Without member 2, which holds P of row 0, the span stays zero and is
    // reported at its array offset.
    let slots = vec![Some(whole(&images[0])), Some(whole(&images[1])), None];
    let degraded = RaidVolume::from_slots(level, 512, chunk as u64, None, slots);
    let assembled = assemble("raid5-degraded", Arc::new(degraded));
    let (bytes, report) = export(&assembled);
    assert_eq!(report.bad_regions, vec![region(100, 64)]);
    let mut expected_bytes = logical;
    expected_bytes[100..164].fill(0);
    assert_eq!(bytes, expected_bytes);
    assert_eq!(assembled.bad_regions(), vec![region(100, 64)]);
}

/// A flat VMDK over the first `extent_len` bytes of `data`, declared at the
/// full length: the tolerant reader zero-fills and records what lies past the
/// end of the extent file, as it does for unreadable sectors.
fn flat_vmdk(
    directory: &std::path::Path,
    name: &str,
    data: &[u8],
    extent_len: usize,
) -> Arc<DiskImage> {
    let extent = format!("{name}.bin");
    std::fs::write(directory.join(&extent), &data[..extent_len]).expect("write extent");
    let descriptor = directory.join(format!("{name}.vmdk"));
    let sectors = data.len() / 512;
    let text = format!(
        "# Disk DescriptorFile\ncreateType=\"monolithicFlat\"\nRW {sectors} FLAT \"{extent}\" 0\n"
    );
    std::fs::write(&descriptor, text).expect("write descriptor");
    Arc::new(DiskImage::open(&descriptor).expect("open VMDK"))
}

/// Real tolerant members: the VMDK backend zero-fills through its block cache
/// and registry, and the array maps what the member recorded.
#[test]
fn tolerant_vmdk_members_feed_the_array_audit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let size = 128 * 1024;
    let data = pattern(size, 7);
    let full = flat_vmdk(directory.path(), "full", &data, size);
    let short = flat_vmdk(directory.path(), "short", &data, 96 * 1024);

    let linear = ConcatVolume::new(vec![Some(whole(&full)), Some(whole(&short))], 0);
    let assembled = assemble("linear-vmdk", Arc::new(linear.expect("linear")));
    let (bytes, report) = export(&assembled);
    let lost = short.bad_regions();
    assert!(!lost.is_empty());
    let shifted: Vec<BadRegion> = lost
        .iter()
        .map(|lost| region(size as u64 + lost.offset, lost.len))
        .collect();
    assert_eq!(report.bad_regions, shifted);
    let logical = [data.as_slice(), data.as_slice()].concat();
    super::test_support::assert_audited(&bytes, &report.bad_regions, &logical, "linear");
    // Read again: the member serves its cached zero-filled block and its
    // registry still marks it.
    let (again, report) = assembled.collect_read_faults(|| assembled.read_range(size as u64, size));
    assert_eq!(again.expect("read").len(), size);
    assert_eq!(report.bad_regions, shifted);

    // RAID1 with the short mirror first: the full mirror supplies what it lost.
    let mirror = MirrorVolume::new(vec![Some(whole(&short)), Some(whole(&full))], size as u64);
    let (bytes, report) = export(&assemble("raid1-vmdk", Arc::new(mirror.expect("mirror"))));
    assert_eq!(bytes, data);
    assert_eq!(report, ReadFaultReport::default());
}

/// `PvSources` hands LV segments the member images themselves.
#[test]
fn lv_over_member_images_reports_faults_at_lv_offsets() {
    let first = pattern(2048, 2);
    let second = pattern(2048, 3);
    let images = [
        member_image("pv0", first.clone(), vec![1500..1520]),
        member_image("pv1", second.clone(), vec![10..12]),
    ];
    let lv = LinearLvSource {
        vg_uuid: "vg".into(),
        vg_name: "vg".into(),
        lv_name: "data".into(),
        size_bytes: 2048,
        segments: vec![
            LvByteSegment {
                source: images[0].clone(),
                byte_offset: 1024,
                byte_length: 1024,
            },
            LvByteSegment {
                source: images[1].clone(),
                byte_offset: 0,
                byte_length: 1024,
            },
        ],
    };
    let (bytes, report) = export(&assemble("lv", Arc::new(lv)));
    assert_eq!(report.bad_regions, vec![region(476, 20), region(1034, 2)]);
    let mut expected_bytes = [&first[1024..], &second[..1024]].concat();
    expected_bytes[476..496].fill(0);
    expected_bytes[1034..1036].fill(0);
    assert_eq!(bytes, expected_bytes);
}
