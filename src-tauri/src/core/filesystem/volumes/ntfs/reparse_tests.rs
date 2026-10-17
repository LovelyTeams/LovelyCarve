use super::*;

#[test]
fn external_reparse_image_regression() -> Result<()> {
    let Ok(path) = std::env::var("NTFS_REPARSE_IMAGE") else {
        return Ok(());
    };
    let image = DiskImage::open(&path)?;
    let volume = NtfsVolume::open(&image, &image.partitions[0])?;
    let mut fs = volume.reader();
    for number in [12114, 12115, 12117, 12125, 12128, 12131, 12133, 12134] {
        let file = volume.ntfs.file(&mut fs, number)?;
        let point = read_reparse(&file, &mut fs)?.context("missing AF_UNIX point")?;
        assert_eq!(point.tag, 0x8000_0023);
        assert!(point.data.is_empty());
        assert!(point.link_target()?.is_none());
    }
    for number in [15422, 15485, 15511, 15609, 15616, 15649] {
        let file = volume.ntfs.file(&mut fs, number)?;
        assert!(!file.flags().contains(NtfsFileFlags::IN_USE));
        assert!(read_reparse(&file, &mut fs)?.is_none());
        // Reparse lookup must not weaken the ordinary attribute-chain check.
        assert!(file
            .attributes()
            .attach(&mut fs)
            .any(|item| matches!(item, Err(ntfs::NtfsError::StaleFileReference { .. }))));
    }
    let total = volume.mft_total_records()?;
    let mut readable = 0;
    let mut points = 0;
    let mut failures = Vec::new();
    for number in 0..total {
        let Ok(file) = volume.ntfs.file(&mut fs, number) else {
            continue;
        };
        readable += 1;
        match read_reparse(&file, &mut fs) {
            Ok(Some(_)) => points += 1,
            Ok(None) => {}
            Err(err) => {
                if failures.len() < 12 {
                    println!(
                        "failure record={number} flags={:?} info={:?} error={err:#}",
                        file.flags(),
                        file.info().map(|info| info.file_attributes())
                    );
                    for attribute in file.attributes_raw() {
                        let attribute = attribute?;
                        println!(
                            "  type={:x} length={} resident={}",
                            attribute.raw_type(),
                            attribute.value_length(),
                            attribute.is_resident()
                        );
                    }
                }
                failures.push(number);
            }
        }
    }
    let (edges, _, truncated) =
        volume.scan_tree_edges(2_000_000, &CancellationToken::new(), &mut |_| {})?;
    assert!(!truncated);
    assert!(!edges.is_empty());
    println!(
        "MFT slots={total}, readable={readable}, reparse points={points}, failures={}, tree edges={}",
        failures.len(),
        edges.len()
    );
    assert!(failures.is_empty(), "failed records: {failures:?}");
    Ok(())
}

#[test]
fn symlinks_and_junctions_preserve_evidence_targets() {
    for (tag, header) in [(0xa000_000cu32, 12), (0xa000_0003, 8)] {
        let target = r"\??\C:\Users\测试";
        let names: Vec<_> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut data = vec![0; header];
        data[2..4].copy_from_slice(&(names.len() as u16).to_le_bytes());
        data.extend(names);
        let mut raw = tag.to_le_bytes().to_vec();
        raw.extend((data.len() as u16).to_le_bytes());
        raw.extend([0, 0]);
        raw.extend(data);
        assert_eq!(
            ReparsePoint::parse(&raw).unwrap().link_target().unwrap(),
            Some(target.into())
        );
        raw[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(ReparsePoint::parse(&raw).unwrap().link_target().is_err());
    }
    assert!(ReparsePoint::parse(&[0; 7]).is_err());
    assert!(ReparsePoint::parse(&[0, 0, 0, 0, 2, 0, 0, 0, 0]).is_err());
}

#[test]
fn trailing_attribute_bytes_are_excluded_from_reparse_payload() {
    let mut unix = vec![0; 24];
    unix[..4].copy_from_slice(&0x8000_0023u32.to_le_bytes());
    let point = ReparsePoint::parse(&unix).unwrap();
    assert_eq!(point.tag, 0x8000_0023);
    assert!(point.data.is_empty());
    assert!(point.link_target().unwrap().is_none());

    // A name outside the declared payload remains invalid, even when those
    // bytes happen to exist in the attribute's trailing area.
    let mut link = vec![0; 24];
    link[..4].copy_from_slice(&0xa000_0003u32.to_le_bytes());
    link[4..6].copy_from_slice(&8u16.to_le_bytes());
    link[10..12].copy_from_slice(&2u16.to_le_bytes());
    link[16..18].copy_from_slice(&u16::from(b'X').to_le_bytes());
    assert!(ReparsePoint::parse(&link).unwrap().link_target().is_err());

    let mut oversized = vec![0; 16 * 1024 + 1];
    oversized[..8].copy_from_slice(&unix[..8]);
    assert!(ReparsePoint::parse(&oversized).is_err());
    unix[4..6].copy_from_slice(&17u16.to_le_bytes());
    assert!(ReparsePoint::parse(&unix).is_err());
}
