//! Plugin market: indexes of installable packages ([`index`]) at sources the
//! user configured ([`source`]), listed against what is installed
//! ([`catalog`]), and downloads that keep only bytes with the SHA-256 the
//! index lists.
//!
//! Market browsing and downloads use configured sources. Opening an explicit
//! install request from the release page uses the fixed official index,
//! without adding it to the user's sources. A downloaded package is
//! inspected like any other archive and must
//! name the listed id and version; it installs through
//! [`PluginManager::install`] with its inspected digest, after the user saw
//! the inspection. Sources live in `plugin-market/sources.json` under the
//! application-data directory, downloads in `plugin-market/downloads/`
//! (named by their SHA-256, removed after install or cancel, and swept
//! when a day old).

pub mod catalog;
mod generate;
pub mod index;
pub mod source;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

pub use catalog::{MarketCatalog, MarketPackage, MarketStatus, MarketVersion};
pub use generate::{generate_index, GeneratedIndex};
pub use index::{IndexArtifact, IndexPackage, IndexVersion, MarketIndex, INDEX_FILE};
pub use source::SourceLocation;

use crate::core::extensions::plugin::{
    current_platform, PackageInspection, PluginError, PluginManager,
};

/// How long a fetched index answers listings before it is read again.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Downloads older than this were left by an install that never finished.
const DOWNLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Most sources kept.
const MAX_SOURCES: usize = 32;
/// The only index used for release-page requests.
const OFFICIAL_INDEX: &str = "https://lovelyteams.github.io/LovelyCarve-Plugin-Site/index.json";

/// Why a market request failed. [`MarketError::reason`] names the kind for
/// the interface.
#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("{0}")]
    InvalidSource(String),
    #[error("{0} is not a configured plugin source")]
    UnknownSource(String),
    #[error("reading {location} failed: {message}")]
    Fetch { location: String, message: String },
    #[error("{location} is not a valid plugin index: {message}")]
    InvalidIndex { location: String, message: String },
    #[error("{location} does not list {plugin_id}")]
    UnknownPackage { location: String, plugin_id: String },
    #[error("{plugin_id} {version} is not listed")]
    UnknownVersion { plugin_id: String, version: String },
    #[error("{plugin_id} {version} has no package for {platform}")]
    NoPackageForPlatform {
        plugin_id: String,
        version: String,
        platform: String,
    },
    #[error("downloading {url} failed: {message}")]
    Download { url: String, message: String },
    #[error("the package from {url} does not match the index: {message}")]
    Integrity { url: String, message: String },
    #[error("the downloaded package is {actual}, the index lists {expected}")]
    Mismatch { expected: String, actual: String },
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MarketError {
    /// The stable name of the kind, carried to the interface; `None` for
    /// failures that come from elsewhere.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::InvalidSource(_) => Some("invalidSource"),
            Self::UnknownSource(_) => Some("unknownSource"),
            Self::Fetch { .. } => Some("fetchFailed"),
            Self::InvalidIndex { .. } => Some("invalidIndex"),
            Self::UnknownPackage { .. } => Some("unknownPackage"),
            Self::UnknownVersion { .. } => Some("unknownVersion"),
            Self::NoPackageForPlatform { .. } => Some("unavailable"),
            Self::Download { .. } => Some("downloadFailed"),
            Self::Integrity { .. } => Some("integrity"),
            Self::Mismatch { .. } => Some("mismatch"),
            Self::Plugin(PluginError::Package(rejection)) => Some(rejection.reason()),
            Self::Plugin(_) | Self::Io(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, MarketError>;

/// A downloaded, verified and inspected package.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketDownload {
    pub source: String,
    /// The archive to install (`plugin_install`) and then release.
    pub archive: String,
    pub sha256: String,
    pub inspection: PackageInspection,
}

#[derive(Default, Deserialize, Serialize)]
struct SourcesFile {
    sources: Vec<String>,
}

struct CachedIndex {
    index: Arc<MarketIndex>,
    fetched_at: String,
    fetched: Instant,
}

/// The market's state (see the module docs).
pub struct PluginMarket {
    sources_file: PathBuf,
    downloads: PathBuf,
    cache: tokio::sync::Mutex<HashMap<String, CachedIndex>>,
    client: OnceLock<reqwest::Client>,
}

impl PluginMarket {
    /// The market of the application-data directory `data_dir`. Sweeps
    /// downloads a day old.
    pub fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("plugin-market");
        let market = Self {
            sources_file: root.join("sources.json"),
            downloads: root.join("downloads"),
            cache: tokio::sync::Mutex::default(),
            client: OnceLock::new(),
        };
        market.sweep_downloads(SystemTime::now());
        market
    }

    /// The configured sources, in the user's order.
    pub fn sources(&self) -> Result<Vec<String>> {
        match std::fs::read(&self.sources_file) {
            Ok(bytes) => {
                let file: SourcesFile = serde_json::from_slice(&bytes).map_err(|error| {
                    MarketError::InvalidSource(format!("{}: {error}", self.sources_file.display()))
                })?;
                Ok(file.sources)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Replaces the sources; each is checked and stored in its stable
    /// spelling ([`SourceLocation::key`]). Returns the stored list.
    pub async fn set_sources(&self, sources: &[String]) -> Result<Vec<String>> {
        if sources.len() > MAX_SOURCES {
            return Err(MarketError::InvalidSource(format!(
                "at most {MAX_SOURCES} sources"
            )));
        }
        let mut keys = Vec::with_capacity(sources.len());
        for source in sources {
            let key = SourceLocation::parse(source)?.key();
            if keys.contains(&key) {
                return Err(MarketError::InvalidSource(format!("{key} is listed twice")));
            }
            keys.push(key);
        }
        if let Some(directory) = self.sources_file.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let temporary = self
            .sources_file
            .with_extension(format!("json.{}", uuid::Uuid::new_v4().simple()));
        let bytes = serde_json::to_vec_pretty(&SourcesFile {
            sources: keys.clone(),
        })
        .map_err(|error| MarketError::InvalidSource(error.to_string()))?;
        std::fs::write(&temporary, bytes)?;
        if let Err(error) = std::fs::rename(&temporary, &self.sources_file) {
            if let Err(cleanup) = std::fs::remove_file(&temporary) {
                tracing::warn!(path = %temporary.display(), "market sources temporary file not removed: {cleanup}");
            }
            return Err(error.into());
        }
        self.cache
            .lock()
            .await
            .retain(|source, _| keys.contains(source));
        Ok(keys)
    }

    /// The configured source spelled `source`, parsed.
    fn configured(&self, source: &str) -> Result<SourceLocation> {
        let location = SourceLocation::parse(source)?;
        if !self.sources()?.contains(&location.key()) {
            return Err(MarketError::UnknownSource(location.key()));
        }
        Ok(location)
    }

    fn client(&self) -> Result<&reqwest::Client> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            .user_agent(concat!("LovelyCarve/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| MarketError::Fetch {
                location: "https".to_owned(),
                message: error.to_string(),
            })?;
        Ok(self.client.get_or_init(|| client))
    }

    /// The index of a configured source, read again after [`CACHE_TTL`] or
    /// on `refresh`.
    pub async fn index(&self, source: &str, refresh: bool) -> Result<(Arc<MarketIndex>, String)> {
        let location = self.configured(source)?;
        self.index_at(&location, refresh).await
    }

    async fn index_at(
        &self,
        location: &SourceLocation,
        refresh: bool,
    ) -> Result<(Arc<MarketIndex>, String)> {
        let key = location.key();
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&key) {
            if !refresh && cached.fetched.elapsed() < CACHE_TTL {
                return Ok((Arc::clone(&cached.index), cached.fetched_at.clone()));
            }
        }
        let bytes = location
            .read_index(self.client()?, index::MAX_INDEX_BYTES)
            .await?;
        let index = MarketIndex::parse(&bytes).map_err(|message| MarketError::InvalidIndex {
            location: key.clone(),
            message,
        })?;
        let index = Arc::new(index);
        let fetched_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        cache.insert(
            key,
            CachedIndex {
                index: Arc::clone(&index),
                fetched_at: fetched_at.clone(),
                fetched: Instant::now(),
            },
        );
        Ok((index, fetched_at))
    }

    /// The source's packages against what `manager` has installed.
    pub async fn catalog(
        &self,
        manager: &PluginManager,
        source: &str,
        refresh: bool,
    ) -> Result<MarketCatalog> {
        let (index, fetched_at) = self.index(source, refresh).await?;
        let location = self.configured(source)?;
        Ok(catalog::catalog(
            &location.key(),
            &index,
            &fetched_at,
            &current_platform(),
            &manager.definitions(),
        ))
    }

    /// Downloads `plugin_id` `version` for this platform from a configured
    /// source, verifies it against the index and inspects it.
    pub async fn download(
        &self,
        manager: &PluginManager,
        source: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<MarketDownload> {
        let (index, _) = self.index(source, false).await?;
        let location = self.configured(source)?;
        self.download_from(manager, &location, &index, plugin_id, version)
            .await
    }

    /// Inspects a version from the official release page without modifying
    /// the list of market sources.
    pub async fn download_official(
        &self,
        manager: &PluginManager,
        plugin_id: &str,
        version: &str,
    ) -> Result<MarketDownload> {
        let location = SourceLocation::parse(OFFICIAL_INDEX)?;
        let (index, _) = self.index_at(&location, false).await?;
        self.download_from(manager, &location, &index, plugin_id, version)
            .await
    }

    async fn download_from(
        &self,
        manager: &PluginManager,
        location: &SourceLocation,
        index: &MarketIndex,
        plugin_id: &str,
        version: &str,
    ) -> Result<MarketDownload> {
        let package = index
            .packages
            .iter()
            .find(|package| package.id == plugin_id)
            .ok_or_else(|| MarketError::UnknownPackage {
                location: location.key(),
                plugin_id: plugin_id.to_owned(),
            })?;
        let listed = package
            .versions
            .iter()
            .find(|listed| listed.version == version)
            .ok_or_else(|| MarketError::UnknownVersion {
                plugin_id: plugin_id.to_owned(),
                version: version.to_owned(),
            })?;
        let platform = current_platform();
        let artifact = listed
            .packages
            .iter()
            .find(|artifact| artifact.platform == platform)
            .ok_or_else(|| MarketError::NoPackageForPlatform {
                plugin_id: plugin_id.to_owned(),
                version: version.to_owned(),
                platform: platform.clone(),
            })?;
        let package_location = location.resolve(&artifact.url)?;
        std::fs::create_dir_all(&self.downloads)?;
        let archive = self.downloads.join(format!("{}.lcpkg", artifact.sha256));
        let have = {
            let archive = archive.clone();
            tokio::task::spawn_blocking(move || {
                crate::core::extensions::plugin::digest_file(&archive).ok()
            })
            .await
            .map_err(|error| MarketError::Download {
                url: package_location.to_string(),
                message: error.to_string(),
            })?
        };
        if have
            .is_none_or(|digest| digest.sha256 != artifact.sha256 || digest.size != artifact.size)
        {
            if archive.exists() {
                std::fs::remove_file(&archive)?;
            }
            source::fetch_package(
                self.client()?,
                &package_location,
                artifact.size,
                &artifact.sha256,
                &archive,
            )
            .await?;
        }
        let inspection = match manager.inspect(archive.clone()).await {
            Ok(inspection) => inspection,
            Err(error) => {
                self.remove_download(&archive);
                return Err(error.into());
            }
        };
        if inspection.plugin_id != plugin_id || inspection.version != version {
            self.remove_download(&archive);
            return Err(MarketError::Mismatch {
                expected: format!("{plugin_id} {version}"),
                actual: format!("{} {}", inspection.plugin_id, inspection.version),
            });
        }
        Ok(MarketDownload {
            source: location.key(),
            archive: archive.to_string_lossy().into_owned(),
            sha256: artifact.sha256.clone(),
            inspection,
        })
    }

    /// Removes the download with SHA-256 `sha256` (installed or declined).
    pub fn release(&self, sha256: &str) -> Result<()> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(MarketError::InvalidSource(format!(
                "{sha256} is not a SHA-256"
            )));
        }
        let archive = self
            .downloads
            .join(format!("{}.lcpkg", sha256.to_ascii_lowercase()));
        match std::fs::remove_file(&archive) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn remove_download(&self, archive: &Path) {
        if let Err(error) = std::fs::remove_file(archive) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %archive.display(), "rejected download not removed: {error}");
            }
        }
    }

    fn sweep_downloads(&self, now: SystemTime) {
        let Ok(entries) = std::fs::read_dir(&self.downloads) else {
            return;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= DOWNLOAD_TTL);
            if stale {
                self.remove_download(&entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests;
