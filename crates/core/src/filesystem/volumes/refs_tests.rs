use super::*;
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::FsSession;
use sha2::{Digest, Sha256};

#[test]
fn refs_ids_preserve_unicode_paths_and_stream_colons() {
    let path = "/目录/文件.txt";
    let stream = "name:$SNAPSHOT";
    let id = format!(
        "refs-snap:part:1:{}:{}",
        hex::encode(path),
        hex::encode(stream)
    );
    assert!(
        matches!(parse_refs_snap_id(&id), Some((RefsSource::Partition(1), decoded, name)) if decoded == path && name == stream)
    );
    assert!(parse_refs_snap_id("refs-snap:part:1::61").is_none());
    assert!(parse_refs_id("refs:part:1:ff").is_none());
    assert!(
        matches!(parse_refs_id("refs:part:1:"), Some((RefsSource::Partition(1), root)) if root.is_empty())
    );
}

#[test]
fn filetimes_keep_subseconds_and_pre_unix_dates() {
    assert_eq!(filetime(0), None);
    assert_eq!(
        filetime(116_444_736_000_000_123),
        Some("1970-01-01T00:00:00.000012300+00:00".into())
    );
    assert_eq!(
        filetime(116_444_735_999_999_999),
        Some("1969-12-31T23:59:59.999999900+00:00".into())
    );
}

#[test]
#[ignore = "requires REFS_RUST_IMAGE pointing to either documented 4 KiB or 64 KiB fixture"]
fn production_reader_traverses_streams_and_seeks_without_the_dll() -> Result<()> {
    let path = std::env::var_os("REFS_RUST_IMAGE").context("set REFS_RUST_IMAGE")?;
    let image = Arc::new(EvidenceSession::open(path)?);
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == crate::evidence::image::PartitionKind::Refs)
        .context("ReFS partition")?;
    let volume = open_refs_source(&image, RefsSource::Partition(partition.id))?;
    let root = volume.root_children()?;
    let is_four_k = root.iter().any(|node| node.name == "test");
    let (path, expected, count) = if is_four_k {
        (
            "/test/elvis_dir_victor_783323/xbpt_large_149mb_alpha_610845.dat",
            "c7d96f97bfab0380aa99d8c0c8ffdec69715d637febcc74e5e83cc8292ecb664",
            1744,
        )
    } else {
        (
            "/testintegrity/elvis_dir_victor_783323/xbpt_large_149mb_alpha_610845.dat",
            "c7d96f97bfab0380aa99d8c0c8ffdec69715d637febcc74e5e83cc8292ecb664",
            894,
        )
    };
    let node_id = volume.node_id_for_path(path);
    let stat = volume.file_stat(path, &node_id)?;
    assert_eq!(stat.size, 156237824);
    assert!(!stat.data_runs.is_empty());
    let mut digest = Sha256::new();
    let mut total = 0;
    let streamed = volume.stream_file(path, 1024 * 1024, |bytes| {
        digest.update(bytes);
        total += bytes.len() as u64;
        Ok(())
    })?;
    assert_eq!(
        (streamed, total, hex::encode(digest.finalize())),
        (stat.size, stat.size, expected.to_string())
    );
    let mut session = FsSession::new(&image);
    for (offset, length) in [
        (0, 17),
        (stat.size / 2, 1234),
        (stat.size - 3, 32),
        (stat.size, 16),
    ] {
        assert_eq!(
            session.read_file_range(&node_id, offset, length)?,
            volume.read_range(path, offset, length)?
        );
    }
    let token = CancellationToken::new();
    let walk = volume.walk_tree(&token)?;
    assert_eq!(walk.entries.len(), count);
    assert!(walk.streams.is_empty());
    token.cancel();
    assert!(volume.walk_tree(&token).is_err());
    Ok(())
}

#[test]
#[ignore = "requires REFS_RUST_SNAPSHOT_IMAGE pointing to win11refs2tsnapshots.raw"]
fn production_snapshot_streams_keep_versions_ranges_and_partial_output_contracts() -> Result<()> {
    use crate::catalog::snapshot::volume_snapshot::{build_volume_snapshot, load_snapshot};
    let path =
        std::env::var_os("REFS_RUST_SNAPSHOT_IMAGE").context("set REFS_RUST_SNAPSHOT_IMAGE")?;
    let image = Arc::new(EvidenceSession::open(path)?);
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == crate::evidence::image::PartitionKind::Refs)
        .context("ReFS partition")?;
    let volume = open_refs_source(&image, RefsSource::Partition(partition.id))?;
    let token = CancellationToken::new();
    let walk = volume.walk_tree(&token)?;
    assert_eq!((walk.entries.len(), walk.streams.len()), (304, 10));
    let expected = [
        (
            "arg.txt",
            "test2",
            11,
            "e88d9660beb6a3325a37a49a8ba71fbbbcc72773b7f9981845443c6d6e8e3131",
        ),
        (
            "lasttest.txt",
            "first",
            186,
            "e982a6d97f54d8a1baecda9159fbba2df7a63803464152b3a0c4a8fa81c64f59",
        ),
        (
            "lasttest.txt",
            "sameaslastversion",
            201,
            "f12e572bf28afe6bfc5e8603ef4a761f844ab81eed89e3426de5344421033f79",
        ),
        (
            "test.txt",
            "firstversion",
            42,
            "633f1a2050b65f4fad72d21f36f144738e32c14b358e25078f2cc367e027bc68",
        ),
        (
            "test.txt",
            "last",
            5,
            "552bab6864c7a7b69a502ed1854b9245c0e1a30f008aaa0b281da62585fdb025",
        ),
        (
            "test.txt",
            "v2",
            65,
            "2e8c3d3f1a2463ff9d5a048256a0879dcb1b729b947e33e96f9f97c6f33eea7f",
        ),
        (
            "test.txt",
            "v3",
            92,
            "831cabd40d200069ea18af06af3250959ad3ade7c05458a90f073e39599784fc",
        ),
        (
            "testads.txt",
            "beforeads",
            12,
            "afea994eb2c510c3deadc969eead9c0dc0b064f8647e7820a02de10c538f772b",
        ),
        (
            "xbpt_beta_sierra_570651.json",
            "copiedversion",
            71395,
            "7546d2f7e7de1dff057ede404c7a04e32ff59db4796ecdb6618a44c57481473b",
        ),
        (
            "xbpt_beta_sierra_570651.json",
            "versionmodified",
            3202036,
            "8be77fd5b5849abf88483fc3c56fbb0e908c5ee87409b94f5022b73b3ecd44d4",
        ),
    ];
    let directory = tempfile::tempdir()?;
    let output = directory.path().join("refs.dirz");
    let volume_key = format!("part:{}", partition.id);
    let built = build_volume_snapshot(
        &image,
        &volume_key,
        &output,
        1000,
        "2026-10-11T00:00:00Z".into(),
        &token,
        &mut |_| {},
    )?;
    assert!(!built.truncated);
    let snapshot = load_snapshot(&output)?;
    let listing = snapshot
        .children(&volume.node_id_for_path("/testsnapshots"))
        .context("snapshot directory")?;
    let mut session = FsSession::new(&image);
    for (file, stream, size, hash) in expected {
        let path = format!("/testsnapshots/{file}");
        let id = format!(
            "refs-snap:part:{}:{}:{}",
            partition.id,
            hex::encode(&path),
            hex::encode(stream)
        );
        let row = listing
            .iter()
            .find(|node| node.id == id)
            .context("snapshot row")?;
        assert_eq!(row.size, size);
        assert_eq!(
            row.parent_id.as_deref(),
            Some(volume.node_id_for_path(&path).as_str())
        );
        let bytes = session.read_file_range(&id, 0, size as usize + 1)?;
        assert_eq!(
            (bytes.len() as u64, hex::encode(Sha256::digest(&bytes))),
            (size, hash.to_string())
        );
        assert_eq!(
            session.read_file_range(&id, size - 2, 32)?,
            bytes[bytes.len() - 2..]
        );
        assert!(session.read_file_range(&id, size, 16)?.is_empty());
    }
    let truncated_output = directory.path().join("truncated.dirz");
    assert!(
        build_volume_snapshot(
            &image,
            &volume_key,
            &truncated_output,
            5,
            "2026-10-11T00:00:00Z".into(),
            &token,
            &mut |_| {}
        )?
        .truncated
    );
    let cancelled_output = directory.path().join("cancelled.dirz");
    let cancelled = CancellationToken::new();
    let result = build_volume_snapshot(
        &image,
        &volume_key,
        &cancelled_output,
        1000,
        "2026-10-11T00:00:00Z".into(),
        &cancelled,
        &mut |_| {
            cancelled.cancel();
        },
    );
    assert!(result.is_err());
    assert!(!cancelled_output.exists());
    Ok(())
}
