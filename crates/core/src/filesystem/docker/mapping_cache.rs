//! Persisted OverlayFS source mappings, validated against evidence identity.
//! Directory snapshots and filedb publication belong to catalog; this cache
//! only restores the filesystem's own merged source-node map.
use super::host_docker_overlay::MergedContainerFs;
use crate::filesystem::evidence_session::EvidenceSession;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const MAPPING_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Mapping<T> {
    version: u32,
    image_id: String,
    container_id: String,
    fingerprint: String,
    tree: T,
}

pub(crate) fn mapping_path(directory: &Path, container_id: &str) -> PathBuf {
    directory.join(format!(
        "{}.zst",
        hex::encode(Sha256::digest(container_id.as_bytes()))
    ))
}

fn read_mapping(
    path: &Path,
    image_id: &str,
    container_id: &str,
    fingerprint: &str,
) -> Result<MergedContainerFs> {
    let decoder = zstd::stream::read::Decoder::new(BufReader::new(File::open(path)?))?;
    let cached: Mapping<MergedContainerFs> = serde_json::from_reader(decoder)?;
    ensure!(
        cached.version == MAPPING_VERSION
            && cached.image_id == image_id
            && cached.container_id == container_id
            && cached.fingerprint == fingerprint,
        "Docker source mapping is stale"
    );
    Ok(cached.tree)
}

pub(crate) fn load_mapping(
    directory: &Path,
    image: &EvidenceSession,
    container_id: &str,
) -> Option<MergedContainerFs> {
    let path = mapping_path(directory, container_id);
    if !path.exists() {
        return None;
    }
    match read_mapping(
        &path,
        &image.id,
        container_id,
        &image.evidence_fingerprint(),
    ) {
        Ok(tree) => Some(tree),
        Err(error) => {
            tracing::warn!("Docker mapping for {container_id} will be rebuilt: {error:#}");
            None
        }
    }
}

pub(crate) fn save_mapping(
    directory: &Path,
    image_id: &str,
    container_id: &str,
    fingerprint: &str,
    tree: &MergedContainerFs,
) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    let mut encoder =
        zstd::stream::write::Encoder::new(BufWriter::new(temporary.as_file_mut()), 3)?;
    serde_json::to_writer(
        &mut encoder,
        &Mapping {
            version: MAPPING_VERSION,
            image_id: image_id.to_owned(),
            container_id: container_id.to_owned(),
            fingerprint: fingerprint.to_owned(),
            tree,
        },
    )?;
    encoder.finish()?.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(mapping_path(directory, container_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_bound_to_image_container_fingerprint_and_format_version() {
        let directory = tempfile::tempdir().expect("cache directory");
        let tree: MergedContainerFs = serde_json::from_value(serde_json::json!({
            "entries": {}, "children": {}
        }))
        .expect("empty container");
        save_mapping(directory.path(), "image", "container", "original", &tree).expect("save");
        let path = mapping_path(directory.path(), "container");
        assert_eq!(
            read_mapping(&path, "image", "container", "original")
                .expect("load")
                .entry_count(),
            0
        );
        for (image, container, fingerprint) in [
            ("other", "container", "original"),
            ("image", "other", "original"),
            ("image", "container", "changed"),
        ] {
            assert!(read_mapping(&path, image, container, fingerprint).is_err());
        }
        let obsolete = Mapping {
            version: 0,
            image_id: "image".into(),
            container_id: "container".into(),
            fingerprint: "original".into(),
            tree,
        };
        let mut encoder = zstd::stream::write::Encoder::new(File::create(&path).expect("file"), 3)
            .expect("encoder");
        serde_json::to_writer(&mut encoder, &obsolete).expect("old mapping");
        encoder.finish().expect("finish");
        assert!(read_mapping(&path, "image", "container", "original").is_err());
    }
}
