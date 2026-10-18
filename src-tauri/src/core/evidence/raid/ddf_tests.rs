use super::test_support::*;
use super::*;
use crate::core::evidence::partitions::test_support::BufSource;

const IMAGE_BYTES: usize = 128 * 1024;
const VD_GUID: [u8; 24] = [0xA1; 24];

fn container() -> ContainerFields {
    ContainerFields::new(
        vec![PhysFields::online(0x1001), PhysFields::online(0x1002)],
        vec![VirtualFields::new(VD_GUID, "Mirror")],
    )
}

fn mirror_config() -> ConfigFields {
    ConfigFields {
        array_blocks: 64,
        ..ConfigFields::new(VD_GUID, 0x01, 0x00, vec![0x1001, 0x1002], 64)
    }
}

fn member(fields: &ContainerFields, refnum: u32) -> Vec<u8> {
    let mut image = vec![0u8; IMAGE_BYTES];
    fields.place(&mut image, refnum, &[mirror_config()]);
    image
}

fn decoded(image: Vec<u8>) -> DdfMetadata {
    match probe(&BufSource(image)).expect("probe") {
        DdfProbe::Valid(metadata) => *metadata,
        other => panic!("expected valid metadata, got {other:?}"),
    }
}

fn invalid_reason(image: Vec<u8>) -> String {
    match probe(&BufSource(image)).expect("probe") {
        DdfProbe::Invalid(reason) => reason,
        other => panic!("expected invalid metadata, got {other:?}"),
    }
}

/// Sector where the primary copy of a member starts.
fn primary_sector() -> usize {
    IMAGE_BYTES / SECTOR - 1 - COPY_SECTORS
}

#[test]
fn decodes_headers_records_and_configuration() {
    let metadata = decoded(member(&container(), 0x1002));
    assert_eq!(
        metadata.header,
        DdfHeader {
            guid: [0x11; 24],
            revision: "01.02.00".into(),
            seq: 7,
            open: false,
        }
    );
    assert_eq!(metadata.disk_refnum, 0x1002);
    assert_eq!(metadata.disk_guid, disk_guid(0x1002));
    assert_eq!(metadata.phys_disks.len(), 2);
    assert!(metadata.phys_disks.iter().all(DdfPhysDisk::in_sync));
    assert_eq!(metadata.virtual_disks.len(), 1);
    assert_eq!(metadata.virtual_disks[0].name, "Mirror");
    let config = &metadata.configs[0];
    assert_eq!(
        (config.prl, config.rlq, config.prim_elmnt_count),
        (0x01, 0x00, 2)
    );
    assert_eq!((config.blocks, config.array_blocks), (64, 64));
    assert_eq!(&config.phys_refnum[..3], &[0x1001, 0x1002, NO_REFNUM]);
    assert_eq!(&config.lba_offset[..3], &[0, 0, u64::MAX]);
}

#[test]
fn no_anchor_is_absent() {
    assert_eq!(
        probe(&BufSource(vec![0u8; IMAGE_BYTES])).expect("probe"),
        DdfProbe::Absent
    );
}

#[test]
fn corrupt_crcs_are_rejected() {
    let fields = container();
    // Anchor.
    let mut image = member(&fields, 0x1001);
    let anchor = IMAGE_BYTES - SECTOR;
    image[anchor + 128] ^= 0x01;
    assert!(invalid_reason(image).contains("anchor"));
    // Physical disk records.
    let mut image = member(&fields, 0x1001);
    image[(primary_sector() + 2) * SECTOR + 64 + 24] ^= 0x01;
    assert!(invalid_reason(image).contains("physical disk section fails its CRC"));
    // A configuration record.
    let mut image = member(&fields, 0x1001);
    image[(primary_sector() + 6) * SECTOR + 72] ^= 0x01;
    assert!(invalid_reason(image).contains("configuration record 0 fails its CRC"));
    // The disk's own data.
    let mut image = member(&fields, 0x1001);
    image[(primary_sector() + 14) * SECTOR + 32] ^= 0x01;
    assert!(invalid_reason(image).contains("physical disk data section fails its CRC"));
}

#[test]
fn newer_secondary_copy_wins_and_open_copies_count_as_older() {
    let primary = container();
    let mut secondary = container();
    secondary.seq = 9;
    secondary.virtuals[0].name = "Newer";
    let mut image = vec![0u8; IMAGE_BYTES];
    primary.place_with_secondary(&secondary, &mut image, 0x1001, &[mirror_config()]);
    let metadata = decoded(image);
    assert_eq!(metadata.header.seq, 9);
    assert_eq!(metadata.virtual_disks[0].name, "Newer");

    // Equal sequence numbers: an open primary loses to a closed secondary.
    let mut open = container();
    open.open = true;
    let mut closed = container();
    closed.virtuals[0].name = "Closed";
    let mut image = vec![0u8; IMAGE_BYTES];
    open.place_with_secondary(&closed, &mut image, 0x1001, &[mirror_config()]);
    assert_eq!(decoded(image).virtual_disks[0].name, "Closed");
    assert_eq!(open.seq, 7);
    let header = DdfHeader {
        guid: [0; 24],
        revision: String::new(),
        seq: 7,
        open: true,
    };
    assert_eq!(header.effective_seq(), 6);
}

#[test]
fn damaged_primary_falls_back_to_the_secondary_copy() {
    let fields = container();
    let mut secondary = container();
    secondary.seq = 6;
    let mut image = vec![0u8; IMAGE_BYTES];
    fields.place_with_secondary(&secondary, &mut image, 0x1001, &[mirror_config()]);
    image[(primary_sector() + 4) * SECTOR + 64] ^= 0x01;
    let metadata = decoded(image);
    assert_eq!(metadata.header.seq, 6);
}

#[test]
fn copies_inconsistent_with_the_anchor_are_ignored() {
    let mut image = member(&container(), 0x1001);
    // Rewrite the primary header with another container GUID.
    let at = primary_sector() * SECTOR;
    image[at + 8] ^= 0xFF;
    seal(&mut image[at..at + SECTOR]);
    assert!(invalid_reason(image).contains("neither the primary nor the secondary"));
}

#[test]
fn unsupported_revisions_are_rejected() {
    let mut fields = container();
    fields.revision = "09.00.00";
    assert!(invalid_reason(member(&fields, 0x1001)).contains("revision"));
}
