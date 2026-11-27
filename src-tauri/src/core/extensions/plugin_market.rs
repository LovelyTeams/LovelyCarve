//! Plugin market: the official index of installable packages ([`index`],
//! the `index.json` of the plugin release page), listed against what is
//! installed ([`catalog`]), and downloads that keep only bytes with the
//! SHA-256 the index lists.
//!
//! The market reads that one index ([`OFFICIAL_INDEX`]): there are no other
//! sources, mirrors or fallbacks, and browsing and install links from the
//! release page read the same copy. A downloaded package is inspected like
//! any other archive and must name the listed id and version; it installs
//! through [`PluginManager::install`] with its inspected digest, after the
//! user saw the inspection. Downloads live in `plugin-market/downloads/`
//! under the application-data directory (named by their SHA-256, removed
//! after install or cancel, and swept when a day old).

pub mod catalog;
mod generate;
pub mod index;
pub mod source;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use serde::Serialize;

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
/// The index the market reads: the plugin release page's.
pub const OFFICIAL_INDEX: &str = "https://lovelyteams.github.io/LovelyCarve-Plugin-Site/index.json";

/// Why a market request failed. [`MarketError::reason`] names the kind for
/// the interface.
#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("{0}")]
    InvalidSource(String),
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

struct CachedIndex {
    index: Arc<MarketIndex>,
    fetched_at: String,
    fetched: Instant,
}

/// The index as [`PluginMarket::read`] gave it.
struct ReadIndex {
    index: Arc<MarketIndex>,
    fetched_at: String,
    /// Taken from the copy read within [`CACHE_TTL`], not read just now.
    cached: bool,
}

/// The market's state (see the module docs).
pub struct PluginMarket {
    location: SourceLocation,
    downloads: PathBuf,
    cache: tokio::sync::Mutex<Option<CachedIndex>>,
    client: OnceLock<reqwest::Client>,
}

impl PluginMarket {
    /// The market of the application-data directory `data_dir`, reading
    /// [`OFFICIAL_INDEX`]. Sweeps downloads a day old.
    pub fn new(data_dir: &Path) -> Result<Self> {
        Ok(Self::reading(
            data_dir,
            SourceLocation::parse(OFFICIAL_INDEX)?,
        ))
    }

    /// A market reading the index at `location` (tests read a market
    /// directory).
    fn reading(data_dir: &Path, location: SourceLocation) -> Self {
        let market = Self {
            location,
            downloads: data_dir.join("plugin-market").join("downloads"),
            cache: tokio::sync::Mutex::default(),
            client: OnceLock::new(),
        };
        market.sweep_downloads(SystemTime::now());
        market
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

    /// The index: the copy read within [`CACHE_TTL`], unless `refresh` or
    /// there is none.
    async fn read(&self, refresh: bool) -> Result<ReadIndex> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if !refresh && cached.fetched.elapsed() < CACHE_TTL {
                return Ok(ReadIndex {
                    index: Arc::clone(&cached.index),
                    fetched_at: cached.fetched_at.clone(),
                    cached: true,
                });
            }
        }
        let bytes = self
            .location
            .read_index(self.client()?, index::MAX_INDEX_BYTES)
            .await?;
        let index = MarketIndex::parse(&bytes).map_err(|message| MarketError::InvalidIndex {
            location: self.location.key(),
            message,
        })?;
        let index = Arc::new(index);
        let fetched_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        *cache = Some(CachedIndex {
            index: Arc::clone(&index),
            fetched_at: fetched_at.clone(),
            fetched: Instant::now(),
        });
        Ok(ReadIndex {
            index,
            fetched_at,
            cached: false,
        })
    }

    /// The index's packages against what `manager` has installed; `refresh`
    /// reads the index again.
    pub async fn catalog(&self, manager: &PluginManager, refresh: bool) -> Result<MarketCatalog> {
        let read = self.read(refresh).await?;
        Ok(catalog::catalog(
            &self.location.key(),
            &read.index,
            &read.fetched_at,
            &current_platform(),
            &manager.definitions(),
        ))
    }

    /// Downloads `plugin_id` `version` for this platform, verifies it
    /// against the index and inspects it. A cached index that does not list
    /// the version is read again first: the release page offers a version
    /// as soon as it is published, before the cached copy expires.
    pub async fn download(
        &self,
        manager: &PluginManager,
        plugin_id: &str,
        version: &str,
    ) -> Result<MarketDownload> {
        let mut read = self.read(false).await?;
        let lists = |index: &MarketIndex| {
            index.packages.iter().any(|package| {
                package.id == plugin_id
                    && package
                        .versions
                        .iter()
                        .any(|listed| listed.version == version)
            })
        };
        if read.cached && !lists(&read.index) {
            read = self.read(true).await?;
        }
        self.download_from(manager, &read.index, plugin_id, version)
            .await
    }

    async fn download_from(
        &self,
        manager: &PluginManager,
        index: &MarketIndex,
        plugin_id: &str,
        version: &str,
    ) -> Result<MarketDownload> {
        let location = &self.location;
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
