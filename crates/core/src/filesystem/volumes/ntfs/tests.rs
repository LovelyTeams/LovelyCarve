use super::*;

#[test]
fn parse_ntfs_id_round_trip() {
    let (source, record) = parse_ntfs_id("ntfs:part:3:42").expect("parse part");
    assert_eq!(source, NtfsSource::Partition(3));
    assert_eq!(record, 42);

    let (source, record) = parse_ntfs_id("ntfs:lvm:2:7:42").expect("parse lvm");
    assert_eq!(
        source,
        NtfsSource::Lvm {
            part_id: 2,
            lv_index: 7
        }
    );
    assert_eq!(record, 42);

    assert!(parse_ntfs_id("ntfs:part:3").is_none());
    assert!(parse_ntfs_id("ntfs:part:3:42:extra").is_none());
    assert!(parse_ntfs_id("ntfs:lvm:2:7").is_none());
    assert!(parse_ntfs_id("ntfs:lvm:2:7:42:extra").is_none());
    assert!(parse_ntfs_id("ext4:part:3:42").is_none());
}

#[test]
fn parse_ntfs_ads_id_round_trip() {
    let id = format!("ntfs-ads:part:3:42:{}", hex::encode("secret:weird"));
    let (source, record, stream) = parse_ntfs_ads_id(&id).expect("parse part ads");
    assert_eq!(source, NtfsSource::Partition(3));
    assert_eq!(record, 42);
    assert_eq!(stream, "secret:weird");

    let id = format!("ntfs-ads:lvm:2:7:42:{}", hex::encode("ads"));
    let (source, record, stream) = parse_ntfs_ads_id(&id).expect("parse lvm ads");
    assert_eq!(
        source,
        NtfsSource::Lvm {
            part_id: 2,
            lv_index: 7
        }
    );
    assert_eq!(record, 42);
    assert_eq!(stream, "ads");

    // Empty stream, missing hex, or a non-ADS prefix all reject.
    assert!(parse_ntfs_ads_id(&format!("ntfs-ads:part:3:42:{}", hex::encode(""))).is_none());
    assert!(parse_ntfs_ads_id("ntfs-ads:part:3:42").is_none());
    assert!(parse_ntfs_ads_id("ntfs:part:3:42").is_none());
    assert!(parse_ntfs_ads_id(&format!("ntfs-ads:part:3:{}", hex::encode("x"))).is_none());

    // is_surfaced_ads_name skips the body + Zone.Identifier, keeps the rest.
    assert!(!is_surfaced_ads_name(""));
    assert!(!is_surfaced_ads_name("Zone.Identifier"));
    assert!(is_surfaced_ads_name("secret"));
}
