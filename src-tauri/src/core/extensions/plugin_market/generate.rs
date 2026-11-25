//! Writing a market directory's `index.json` from the `.lcpkg` files in it
//! (`plugin market index`). Each package is read like an install reads it
//! (manifest, locales and the platforms of its libraries, nothing run);
//! packages of one id become versions of one entry, named and described by
//! the highest version. A package listed in the previous index with the
//! same SHA-256 keeps its publication time, so an unchanged directory
//! gives an unchanged index.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{
    index::{
        IndexArtifact, IndexPackage, IndexVersion, MarketIndex, INDEX_FILE, INDEX_FORMAT,
        LOCALE_KEYS,
    },
    MarketError, Result,
};
use crate::core::extensions::plugin::{describe_archive, ArchiveDescription, PACKAGE_EXTENSION};

/// What [`generate_index`] wrote.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedIndex {
    pub index: String,
    pub packages: usize,
    pub versions: usize,
    pub files: usize,
}

/// Writes `<directory>/index.json` (see the module docs).
pub fn generate_index(
    directory: &Path,
    title: &str,
    description: Option<&str>,
) -> Result<GeneratedIndex> {
    let index_path = directory.join(INDEX_FILE);
    let previous: HashMap<String, String> = std::fs::read(&index_path)
        .ok()
        .and_then(|bytes| MarketIndex::parse(&bytes).ok())
        .map(|index| {
            index
                .packages
                .into_iter()
                .flat_map(|package| package.versions)
                .flat_map(|version| {
                    let published = version.published;
                    version
                        .packages
                        .into_iter()
                        .map(move |artifact| (artifact.sha256, published.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut archives: Vec<PathBuf> = std::fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case(PACKAGE_EXTENSION))
        })
        .collect();
    archives.sort();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // id → version → (description of the package, files).
    let mut grouped: BTreeMap<
        String,
        BTreeMap<semver::Version, Vec<(ArchiveDescription, String)>>,
    > = BTreeMap::new();
    for archive in &archives {
        let description = describe_archive(archive).map_err(MarketError::Plugin)?;
        let file_name = archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        super::index::check_url(&file_name).map_err(|message| {
            MarketError::InvalidSource(format!("{file_name}: the file name {message}"))
        })?;
        let version = description
            .manifest
            .semantic_version()
            .map_err(MarketError::Plugin)?;
        grouped
            .entry(description.manifest.id.clone())
            .or_default()
            .entry(version)
            .or_default()
            .push((description, file_name));
    }
    let mut packages = Vec::new();
    let mut version_count = 0;
    for (id, versions) in grouped {
        let Some(newest) = versions
            .values()
            .next_back()
            .and_then(|files| files.first())
        else {
            continue;
        };
        let manifest = &newest.0.manifest;
        let locales = newest
            .0
            .locales
            .iter()
            .filter_map(|(locale, entries)| {
                let kept: BTreeMap<String, String> = entries
                    .iter()
                    .filter(|(key, _)| LOCALE_KEYS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                (!kept.is_empty()).then(|| (locale.clone(), kept))
            })
            .collect();
        let mut listed = Vec::new();
        for files in versions.values().rev() {
            let mut artifacts = Vec::new();
            for (description, file_name) in files {
                for platform in &description.platforms {
                    if artifacts
                        .iter()
                        .any(|artifact: &IndexArtifact| artifact.platform == *platform)
                    {
                        return Err(MarketError::InvalidSource(format!(
                            "{id} {} has two packages for {platform}",
                            description.manifest.version
                        )));
                    }
                    artifacts.push(IndexArtifact {
                        platform: platform.clone(),
                        url: file_name.clone(),
                        size: description.digest.size,
                        sha256: description.digest.sha256.clone(),
                    });
                }
            }
            let first = &files[0].0;
            let published = files
                .iter()
                .find_map(|(description, _)| previous.get(&description.digest.sha256).cloned())
                .unwrap_or_else(|| now.clone());
            listed.push(IndexVersion {
                version: first.manifest.version.clone(),
                published,
                permissions: first.manifest.permissions.clone(),
                packages: artifacts,
            });
        }
        version_count += listed.len();
        packages.push(IndexPackage {
            id,
            name: manifest.name.clone(),
            description: manifest.description.clone(),
            publisher: manifest.publisher.clone(),
            license: manifest.license.clone(),
            locales,
            versions: listed,
        });
    }
    let index = MarketIndex {
        format: INDEX_FORMAT,
        title: title.trim().to_owned(),
        description: description
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(str::to_owned),
        packages,
    };
    index
        .validate()
        .map_err(|message| MarketError::InvalidIndex {
            location: index_path.to_string_lossy().into_owned(),
            message,
        })?;
    let mut bytes = serde_json::to_vec_pretty(&index)
        .map_err(|error| MarketError::InvalidSource(error.to_string()))?;
    bytes.push(b'\n');
    if std::fs::read(&index_path).ok().as_deref() != Some(bytes.as_slice()) {
        let temporary =
            index_path.with_extension(format!("json.{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&temporary, &bytes)?;
        if let Err(error) = std::fs::rename(&temporary, &index_path) {
            if let Err(cleanup) = std::fs::remove_file(&temporary) {
                tracing::warn!(path = %temporary.display(), "index temporary file not removed: {cleanup}");
            }
            return Err(error.into());
        }
    }
    Ok(GeneratedIndex {
        index: index_path.to_string_lossy().into_owned(),
        packages: index.packages.len(),
        versions: version_count,
        files: archives.len(),
    })
}
