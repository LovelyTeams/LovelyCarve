use super::*;
use crate::core::evidence::partitions::lvm2::{header::checksum, test_support::*};
use std::io::Write;

fn member(text: &str, identifier: &str) -> (tempfile::NamedTempFile, Arc<DiskImage>) {
    let mut bytes = image(text, 1, false);
    bytes[544..576].copy_from_slice(identifier.as_bytes());
    let crc = checksum(&bytes[532..1024]);
    bytes[528..532].copy_from_slice(&crc.to_le_bytes());
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    (file, image)
}

#[test]
fn lvm_cross_image_same_size_pvs_preserve_complete_logical_bytes() {
    let text = text().replace("\"pv1\", 0", "\"pv0\", 0");
    let (_first_file, first) = member(&text, PV_ID);
    let (_second_file, second) = member(&text, OTHER_PV_ID);
    let images = [first, second];
    let metadata: Vec<_> = images
        .iter()
        .map(|image| {
            lvm_meta::read_pv_metadata(image, &image.partitions[0])
                .unwrap()
                .unwrap()
        })
        .collect();
    assert_eq!(metadata[0].pv_local_id, "pv1");
    assert_eq!(metadata[1].pv_local_id, "pv0");
    let mut diagnostics = Vec::new();
    let groups = group_pvs_into_vgs(&metadata, &mut diagnostics);
    assert!(diagnostics.is_empty());
    let sources = PvSources::from_images(&images);
    let volumes = materialise_logical_volumes(&sources, &groups);
    assert_eq!(volumes.len(), 1);
    let expected = [vec![0x11; 8192], vec![0x22; 8192]].concat();
    assert_eq!(volumes[0].read_at(0, usize::MAX).unwrap(), expected);
    assert_eq!(
        volumes[0].read_at(8190, 4).unwrap(),
        [0x11, 0x11, 0x22, 0x22]
    );
    let missing = group_pvs_into_vgs(&metadata[..1], &mut diagnostics);
    assert!(!diagnostics.is_empty());
    assert!(materialise_logical_volumes(&sources, &missing).is_empty());
    let first_only = PvSources::from_images(&images[..1]);
    assert!(materialise_logical_volumes(&first_only, &groups).is_empty());
}

#[test]
fn lvm_assembly_uses_latest_uuid_bindings_and_rejects_conflicts() {
    let text = text().replace("\"pv1\", 0", "\"pv0\", 0");
    let newer = text
        .replace("seqno = 7", "seqno = 8")
        .replace("pv1", "renamed");
    let (_first_file, first) = member(&text, PV_ID);
    let (_second_file, second) = member(&newer, OTHER_PV_ID);
    let images = [first, second];
    let mut metadata: Vec<_> = images
        .iter()
        .map(|image| {
            lvm_meta::read_pv_metadata(image, &image.partitions[0])
                .unwrap()
                .unwrap()
        })
        .collect();
    let mut diagnostics = Vec::new();
    let sources = PvSources::from_images(&images);
    let groups = group_pvs_into_vgs(&metadata, &mut diagnostics);
    assert_eq!(groups[0].members[0].pv_local_id, "renamed");
    assert_eq!(materialise_logical_volumes(&sources, &groups).len(), 1);
    metadata[0].volume_group.sequence_number = 8;
    let groups = group_pvs_into_vgs(&metadata, &mut diagnostics);
    assert!(materialise_logical_volumes(&sources, &groups).is_empty());
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("conflicting")));
    metadata[0].volume_group = metadata[1].volume_group.clone();
    metadata.push(metadata[0].clone());
    let groups = group_pvs_into_vgs(&metadata, &mut diagnostics);
    assert!(materialise_logical_volumes(&sources, &groups).is_empty());
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("ambiguous")));
}

#[test]
fn lvm_linear_source_never_returns_partial_allocated_content() {
    let (_file, image) = member(&text(), PV_ID);
    let source = LinearLvSource {
        vg_uuid: "group".into(),
        vg_name: "vg".into(),
        lv_name: "root".into(),
        size_bytes: 8,
        segments: vec![LvByteSegment {
            source: image.clone(),
            byte_offset: image.len() - 1,
            byte_length: 8,
        }],
    };
    assert!(source.read_at(0, 8).is_err());
    let source = LinearLvSource {
        size_bytes: 9,
        segments: vec![LvByteSegment {
            source: image,
            byte_offset: 0,
            byte_length: 8,
        }],
        ..source
    };
    assert!(source.read_at(0, 9).is_err());
}
