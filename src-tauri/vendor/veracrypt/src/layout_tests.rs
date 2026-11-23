use super::*;
use crate::header::MASTER_KEY_LEN;
use zeroize::Zeroizing;

fn header(flavor: Flavor, required: u16, start: u64, length: u64, volume: u64) -> Header {
    Header {
        flavor,
        version: 5,
        required_program_version: required,
        volume_created: 0,
        header_created: 0,
        hidden_volume_size: 0,
        volume_size: volume,
        encrypted_area_start: start,
        encrypted_area_length: length,
        flags: 0,
        sector_size: 512,
        master_keys: Zeroizing::new([0u8; MASTER_KEY_LEN]),
    }
}

fn view(geometry: &Geometry) -> (u64, u64, u64, u64, bool) {
    (
        geometry.view_start,
        geometry.view_len,
        geometry.encrypted_start,
        geometry.encrypted_end,
        geometry.complete,
    )
}

#[test]
fn header_offsets_follow_the_volume_layout() {
    let size = 1 << 20;
    let offsets: Vec<_> = [
        HeaderKind::Normal,
        HeaderKind::Hidden,
        HeaderKind::BackupNormal,
        HeaderKind::BackupHidden,
        HeaderKind::LegacyHidden,
        HeaderKind::System,
    ]
    .into_iter()
    .map(|kind| kind.offset(size))
    .collect();
    assert_eq!(
        offsets,
        [
            Some(0),
            Some(65536),
            Some(size - 131_072),
            Some(size - 65536),
            Some(size - 1536),
            Some(31_744)
        ]
    );
    assert_eq!(HeaderKind::BackupNormal.offset(1000), None);
}

#[test]
fn normal_and_hidden_volumes_span_their_declared_area() {
    let veracrypt = header(Flavor::VeraCrypt, 0x010b, 131_072, 65536, 65536);
    let geometry = super::geometry(HeaderKind::Normal, &veracrypt, 262_144, None).unwrap();
    assert_eq!(view(&geometry), (131_072, 65536, 131_072, 196_608, true));
    assert!(geometry.boot_sector_candidate.is_none());
    // A truncated source fails rather than reading past its end.
    assert!(matches!(
        super::geometry(HeaderKind::Hidden, &veracrypt, 196_607, None),
        Err(VeraError::Corrupt(_))
    ));
}

#[test]
fn unfinished_or_inconsistent_areas_are_refused() {
    let mut partial = header(Flavor::VeraCrypt, 0x010b, 131_072, 4096, 65536);
    assert!(matches!(
        geometry(HeaderKind::Normal, &partial, 1 << 20, None),
        Err(VeraError::Corrupt(_))
    ));
    partial.flags = FLAG_IN_PLACE;
    assert!(matches!(
        geometry(HeaderKind::Normal, &partial, 1 << 20, None),
        Err(VeraError::Unsupported(_))
    ));
    // A finished in-place encryption reads like any other volume.
    let finished = Header {
        flags: FLAG_IN_PLACE,
        ..header(Flavor::VeraCrypt, 0x010b, 131_072, 65536, 65536)
    };
    assert!(geometry(HeaderKind::Normal, &finished, 1 << 20, None).is_ok());
    let overlapping = header(Flavor::VeraCrypt, 0x010b, 0, 65536, 65536);
    assert!(geometry(HeaderKind::Normal, &overlapping, 1 << 20, None).is_err());
}

#[test]
fn pre_6_truecrypt_layouts_start_after_the_header() {
    let old = header(Flavor::TrueCrypt, 0x0500, 0, 0, 0);
    let geometry = super::geometry(HeaderKind::Normal, &old, 65536, None).unwrap();
    assert_eq!(view(&geometry), (512, 65024, 512, 65536, true));
    assert!(matches!(
        super::geometry(HeaderKind::BackupNormal, &old, 1 << 20, None),
        Err(VeraError::Unsupported(_))
    ));
    let hidden = Header {
        hidden_volume_size: 4096,
        ..header(Flavor::TrueCrypt, 0x0500, 0, 0, 4096)
    };
    let geometry = super::geometry(HeaderKind::LegacyHidden, &hidden, 65536, None).unwrap();
    assert_eq!(geometry.view_start, 65536 - 1536 - 4096);
    assert_eq!(geometry.view_len, 4096);
}

#[test]
fn system_partitions_must_start_inside_the_encrypted_area() {
    let system = header(Flavor::VeraCrypt, 0x010b, 1 << 20, 4 << 20, 4 << 20);
    let size = 8 << 20;
    let inside = SystemRange::Partition {
        offset: 1 << 20,
        length: 4 << 20,
    };
    let geometry = super::geometry(HeaderKind::System, &system, size, Some(inside)).unwrap();
    assert_eq!(view(&geometry), (1 << 20, 4 << 20, 1 << 20, 5 << 20, true));
    assert_eq!(geometry.boot_sector_candidate, Some(1 << 20));
    let outside = SystemRange::Partition {
        offset: 6 << 20,
        length: 1 << 20,
    };
    assert!(super::geometry(HeaderKind::System, &system, size, Some(outside)).is_err());
    let too_long = SystemRange::Partition {
        offset: 1 << 20,
        length: 5 << 20,
    };
    assert!(super::geometry(HeaderKind::System, &system, size, Some(too_long)).is_err());
    let drive =
        super::geometry(HeaderKind::System, &system, size, Some(SystemRange::Drive)).unwrap();
    assert_eq!(view(&drive), (0, size, 1 << 20, 5 << 20, true));
}

#[test]
fn a_cut_down_drive_reads_while_the_partition_is_inside_it() {
    // The header describes a 64 GiB area; the image stops after 8 MiB.
    let system = header(Flavor::VeraCrypt, 0x010b, 1 << 20, 64 << 30, 64 << 30);
    let range = SystemRange::Partition {
        offset: 1 << 20,
        length: 4 << 20,
    };
    let geometry = super::geometry(HeaderKind::System, &system, 8 << 20, Some(range)).unwrap();
    assert_eq!(
        view(&geometry),
        (1 << 20, 4 << 20, 1 << 20, (1 << 20) + (64 << 30), true)
    );
    let past = SystemRange::Partition {
        offset: 1 << 20,
        length: 8 << 20,
    };
    assert!(super::geometry(HeaderKind::System, &system, 8 << 20, Some(past)).is_err());
}

#[test]
fn unfinished_system_encryption_reads_its_tail_as_stored() {
    let partial = header(Flavor::VeraCrypt, 0x010b, 1 << 20, 1 << 20, 4 << 20);
    let range = SystemRange::Partition {
        offset: 1 << 20,
        length: 4 << 20,
    };
    let geometry = super::geometry(HeaderKind::System, &partial, 8 << 20, Some(range)).unwrap();
    assert_eq!(view(&geometry), (1 << 20, 4 << 20, 1 << 20, 2 << 20, false));
}
