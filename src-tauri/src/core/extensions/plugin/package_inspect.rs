//! What a package archive would install, read before anything is
//! replaced: its identity and declarations, the digests of the archive and
//! of the library for this platform, and how it relates to the installed
//! version of the same plugin. Inspecting reads the archive's metadata
//! files and hashes its library; it never loads the library.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    package::{LOCALES_DIRECTORY, MANIFEST_FILE, README_FILE, SCHEMA_FILE},
    package_archive::{self, digest_file, FileDigest, PackageArchive},
    AnalyzerScope, Locales, PackageRejection, PluginDefinition, PluginError, PluginManager,
    PluginPermission, Result,
};

/// Largest metadata file read from an archive.
const MAX_METADATA_FILE: u64 = 4 * 1024 * 1024;

/// How a package relates to the installed version of its plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PackageChange {
    /// The plugin is not installed.
    New,
    /// A higher version than the installed one.
    Upgrade,
    /// A lower version than the installed one.
    Downgrade,
    /// The installed version, with the same library.
    Reinstall,
    /// The installed version number with a different (or unreadable)
    /// library: another build.
    Rebuild,
}

/// The installed version an inspection compares with.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledVersion {
    pub version: String,
    /// Bundled with the application: cannot be replaced.
    pub bundled: bool,
    /// The installed library's SHA-256, when it could be read.
    pub library_sha256: Option<String>,
}

/// One analyzer a package declares.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedAnalyzer {
    pub id: String,
    pub name: String,
    pub scope: AnalyzerScope,
}

/// Why an inspected package cannot be installed.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallBlock {
    /// [`PackageRejection::reason`].
    pub reason: &'static str,
    pub message: String,
}

/// What a package archive holds (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInspection {
    pub archive: String,
    /// The archive's SHA-256; installing requires the same bytes.
    pub sha256: String,
    pub size: u64,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub license: Option<String>,
    pub permissions: Vec<PluginPermission>,
    pub memory_limit: u64,
    pub analyzers: Vec<InspectedAnalyzer>,
    pub filesystems: Vec<String>,
    /// Record types the package's `schema.json` describes.
    pub record_types: usize,
    /// SDK the library was built with (`schema.json`).
    pub sdk: Option<String>,
    pub schema_hash: Option<String>,
    pub locales: Locales,
    /// The library for this platform, relative to the package.
    pub library: String,
    /// Its SHA-256; `None` when the package has no library for this
    /// platform.
    pub library_sha256: Option<String>,
    pub installed: Option<InstalledVersion>,
    pub change: PackageChange,
    pub blocked: Option<InstallBlock>,
}

/// The archive's own facts, before they are compared with what is
/// installed.
pub(super) struct ArchiveFacts {
    pub digest: FileDigest,
    pub definition: PluginDefinition,
    pub library: String,
    pub library_sha256: Option<String>,
}

/// Reads `archive`'s metadata into a scratch directory, loads it as a
/// package definition (the same checks as discovery) and hashes the
/// library. Blocking.
pub(super) fn read_archive(archive: &Path) -> Result<ArchiveFacts> {
    let digest = digest_file(archive).map_err(|error| {
        PackageRejection::InvalidArchive(format!("{}: {error}", archive.display()))
    })?;
    let mut package = PackageArchive::open(archive)?;
    let scratch = tempfile::tempdir()?;
    let mut metadata = vec![
        MANIFEST_FILE.to_owned(),
        SCHEMA_FILE.to_owned(),
        README_FILE.to_owned(),
    ];
    metadata.extend(package.files_in(LOCALES_DIRECTORY));
    for relative in metadata {
        if let Some(bytes) = package.read(&relative, MAX_METADATA_FILE)? {
            let path = scratch.path().join(&relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes)?;
        }
    }
    let definition = PluginDefinition::load(scratch.path(), true).map_err(invalid_package)?;
    let library = package_archive::archive_path(&definition.manifest.library_path(Path::new("")));
    let library_sha256 = package.sha256(&library)?;
    Ok(ArchiveFacts {
        digest,
        definition,
        library,
        library_sha256,
    })
}

/// A manifest, schema or locale problem of an archive, as a rejection.
pub(super) fn invalid_package(error: PluginError) -> PluginError {
    match error {
        PluginError::Manifest(message) => PackageRejection::InvalidPackage(message).into(),
        error => error,
    }
}

impl PluginManager {
    /// Inspects `archive` (see the module docs).
    pub async fn inspect(&self, archive: PathBuf) -> Result<PackageInspection> {
        let facts = {
            let archive = archive.clone();
            tokio::task::spawn_blocking(move || read_archive(&archive))
                .await
                .map_err(|error| PluginError::Unavailable(error.to_string()))??
        };
        let manifest = &facts.definition.manifest;
        let installed = match self.definition(&manifest.id).ok() {
            Some(installed) => {
                let library = installed.manifest.library_path(&installed.directory);
                let library_sha256 = tokio::task::spawn_blocking(move || {
                    digest_file(&library).ok().map(|digest| digest.sha256)
                })
                .await
                .map_err(|error| PluginError::Unavailable(error.to_string()))?;
                Some(InstalledVersion {
                    version: installed.manifest.version.clone(),
                    bundled: !installed.removable,
                    library_sha256,
                })
            }
            None => None,
        };
        let change = match &installed {
            None => PackageChange::New,
            Some(installed) => {
                let current = semver::Version::parse(&installed.version).ok();
                let offered = manifest.semantic_version()?;
                match current.map(|current| offered.cmp(&current)) {
                    Some(std::cmp::Ordering::Greater) => PackageChange::Upgrade,
                    Some(std::cmp::Ordering::Less) => PackageChange::Downgrade,
                    Some(std::cmp::Ordering::Equal)
                        if installed.library_sha256.is_some()
                            && installed.library_sha256 == facts.library_sha256 =>
                    {
                        PackageChange::Reinstall
                    }
                    Some(std::cmp::Ordering::Equal) | None => PackageChange::Rebuild,
                }
            }
        };
        let blocked = if self.install_root().is_none() {
            Some(PackageRejection::NoInstallRoot)
        } else if installed
            .as_ref()
            .is_some_and(|installed| installed.bundled)
        {
            Some(PackageRejection::Bundled {
                plugin_id: manifest.id.clone(),
            })
        } else if facts.library_sha256.is_none() {
            Some(PackageRejection::NoLibrary {
                library: facts.library.clone(),
            })
        } else {
            None
        }
        .map(|rejection| InstallBlock {
            reason: rejection.reason(),
            message: rejection.to_string(),
        });
        let definition = &facts.definition;
        Ok(PackageInspection {
            archive: archive.to_string_lossy().into_owned(),
            sha256: facts.digest.sha256.clone(),
            size: facts.digest.size,
            plugin_id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            publisher: manifest.publisher.clone(),
            license: manifest.license.clone(),
            permissions: manifest.permissions.clone(),
            memory_limit: manifest.memory_limit()?,
            analyzers: manifest
                .analyzers
                .iter()
                .map(|analyzer| InspectedAnalyzer {
                    id: analyzer.id.clone(),
                    name: analyzer.name.clone(),
                    scope: analyzer.scope,
                })
                .collect(),
            filesystems: manifest
                .filesystems
                .iter()
                .map(|filesystem| filesystem.id.clone())
                .collect(),
            record_types: definition.record_schemas().len(),
            sdk: definition
                .described
                .as_ref()
                .map(|described| described.sdk.clone()),
            schema_hash: definition
                .described
                .as_ref()
                .map(|described| described.schema_hash.clone()),
            locales: definition.locales.as_ref().clone(),
            library: facts.library.clone(),
            library_sha256: facts.library_sha256.clone(),
            installed,
            change,
            blocked,
        })
    }
}
