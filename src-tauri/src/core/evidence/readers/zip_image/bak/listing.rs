//! Saved backup projection of a ZIP evidence archive, kept in the evidence
//! cache as zstd-compressed JSON (`evidence/<uid>/cache/zip-backups.zst`).
//!
//! For every backup candidate member the listing records either the members
//! its decoded TAR projected or that it stayed a file. It is bound to the
//! archive length and to each candidate's path, central-directory index, local
//! header offset, CRC-32 and sizes, so a changed archive never restores a stale
//! tree. An unreadable or mismatched listing only costs a full decode.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::core::evidence::readers::zip_image::ZipEntryKind;

pub(super) const LISTING_FILE: &str = "zip-backups.zst";
const LISTING_VERSION: u32 = 1;

/// Central-directory identity of a backup candidate member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MemberIdentity {
    pub(super) path: String,
    pub(super) index: usize,
    pub(super) header_start: u64,
    pub(super) crc32: u32,
    pub(super) compressed_size: u64,
    pub(super) size: u64,
}

/// One member of a decoded backup TAR, relative to the backup root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ListedEntry {
    pub(super) path: String,
    pub(super) kind: ZipEntryKind,
    pub(super) size: u64,
    pub(super) mode: u32,
    pub(super) mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) link_target: Option<String>,
}

/// What a decoded backup projects beneath its member path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Expansion {
    pub(super) total_size: u64,
    pub(super) entries: Vec<ListedEntry>,
}

#[derive(Serialize, Deserialize)]
struct ListedMember<E> {
    member: MemberIdentity,
    /// `None`: the member stayed a file (not a backup, encrypted, corrupt, or
    /// over the entry cap).
    expansion: Option<E>,
}

#[derive(Serialize, Deserialize)]
struct Listing<E> {
    version: u32,
    archive_len: u64,
    members: Vec<ListedMember<E>>,
}

/// The saved expansions, aligned with `identities`, when the listing at `path`
/// describes exactly this archive.
pub(super) fn load(
    path: &Path,
    archive_len: u64,
    identities: &[MemberIdentity],
) -> Option<Vec<Option<Expansion>>> {
    if !path.is_file() {
        return None;
    }
    match read(path, archive_len, identities) {
        Ok(expansions) => Some(expansions),
        Err(error) => {
            tracing::info!(
                "mi-bak: backup listing {} not reused: {error:#}",
                path.display()
            );
            None
        }
    }
}

fn read(
    path: &Path,
    archive_len: u64,
    identities: &[MemberIdentity],
) -> Result<Vec<Option<Expansion>>> {
    let mut json = Vec::new();
    zstd::stream::read::Decoder::new(File::open(path)?)?.read_to_end(&mut json)?;
    let listing: Listing<Expansion> = serde_json::from_slice(&json)?;
    ensure!(
        listing.version == LISTING_VERSION,
        "listing format {} is not {LISTING_VERSION}",
        listing.version
    );
    ensure!(
        listing.archive_len == archive_len,
        "archive length changed from {}",
        listing.archive_len
    );
    ensure!(
        listing.members.len() == identities.len()
            && listing
                .members
                .iter()
                .zip(identities)
                .all(|(listed, identity)| listed.member == *identity),
        "backup candidate members changed"
    );
    Ok(listing
        .members
        .into_iter()
        .map(|member| member.expansion)
        .collect())
}

/// Atomically replaces the listing at `path`.
pub(super) fn save(
    path: &Path,
    archive_len: u64,
    identities: &[MemberIdentity],
    expansions: &[Option<Expansion>],
) -> Result<()> {
    ensure!(
        identities.len() == expansions.len(),
        "{} identities for {} expansions",
        identities.len(),
        expansions.len()
    );
    let directory = path.parent().context("listing path has no directory")?;
    std::fs::create_dir_all(directory)?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    let mut encoder =
        zstd::stream::write::Encoder::new(BufWriter::new(temporary.as_file_mut()), 3)?;
    serde_json::to_writer(
        &mut encoder,
        &Listing {
            version: LISTING_VERSION,
            archive_len,
            members: identities
                .iter()
                .zip(expansions)
                .map(|(member, expansion)| ListedMember {
                    member: member.clone(),
                    expansion: expansion.as_ref(),
                })
                .collect(),
        },
    )?;
    encoder.finish()?.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(crc32: u32) -> MemberIdentity {
        MemberIdentity {
            path: "MIUI/backup/app.bak".into(),
            index: 3,
            header_start: 128,
            crc32,
            compressed_size: 40,
            size: 64,
        }
    }

    fn expansion() -> Expansion {
        Expansion {
            total_size: 7,
            entries: vec![ListedEntry {
                path: "apps/com.example/link".into(),
                kind: ZipEntryKind::Symlink,
                size: 0,
                mode: 0o777,
                mtime: 1_700_000_000,
                link_target: Some("db/data".into()),
            }],
        }
    }

    #[test]
    fn listing_is_bound_to_archive_members_and_format_version() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("nested").join(LISTING_FILE);
        let expansions = vec![Some(expansion()), None];
        let identities = [identity(1), identity(2)];
        save(&path, 4096, &identities, &expansions).expect("save listing");
        assert_eq!(load(&path, 4096, &identities), Some(expansions.clone()));

        assert_eq!(load(&path, 4097, &identities), None);
        assert_eq!(load(&path, 4096, &[identity(1), identity(9)]), None);
        assert_eq!(load(&path, 4096, &identities[..1]), None);
        assert_eq!(
            load(&directory.path().join("missing.zst"), 4096, &identities),
            None
        );

        let mut encoder =
            zstd::stream::write::Encoder::new(File::create(&path).expect("file"), 3).expect("zstd");
        serde_json::to_writer(
            &mut encoder,
            &Listing {
                version: LISTING_VERSION + 1,
                archive_len: 4096,
                members: identities
                    .iter()
                    .zip(&expansions)
                    .map(|(member, expansion)| ListedMember {
                        member: member.clone(),
                        expansion: expansion.as_ref(),
                    })
                    .collect(),
            },
        )
        .expect("future listing");
        encoder.finish().expect("finish");
        assert_eq!(load(&path, 4096, &identities), None);

        std::fs::write(&path, b"not zstd").expect("corrupt listing");
        assert_eq!(load(&path, 4096, &identities), None);
    }
}
