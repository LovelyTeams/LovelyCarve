//! A source's packages as the market page lists them: newest version
//! first, the version this platform would install, and the state against
//! the installed package of the same id.

use std::cmp::Ordering;

use serde::Serialize;

use super::index::{IndexPackage, MarketIndex};
use crate::core::extensions::plugin::{Locales, PluginDefinition, PluginPermission};

/// A source's listing.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCatalog {
    pub source: String,
    pub title: String,
    pub description: Option<String>,
    /// When the index was read (RFC 3339).
    pub fetched_at: String,
    /// This build's `<os>-<arch>`.
    pub platform: String,
    pub packages: Vec<MarketPackage>,
}

/// How a listed package stands against the installed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MarketStatus {
    /// Not installed; `latest` can be installed.
    NotInstalled,
    /// The installed version is `latest`.
    Installed,
    /// `latest` is newer than the installed version.
    UpdateAvailable,
    /// The installed version is newer than anything listed for this
    /// platform.
    InstalledNewer,
    /// A bundled package of the same id; it cannot be replaced.
    Bundled,
    /// Not installed, and no listed version has a package for this
    /// platform.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPackage {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub license: Option<String>,
    pub locales: Locales,
    /// The version an install takes: the highest release with a package
    /// for this platform, else the highest pre-release.
    pub latest: Option<String>,
    pub installed: Option<String>,
    pub status: MarketStatus,
    /// Newest first.
    pub versions: Vec<MarketVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketVersion {
    pub version: String,
    pub published: String,
    pub permissions: Vec<PluginPermission>,
    pub prerelease: bool,
    /// This platform's package, when the version has one.
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// The listing of `index` (see the module docs).
pub fn catalog(
    source: &str,
    index: &MarketIndex,
    fetched_at: &str,
    platform: &str,
    installed: &[PluginDefinition],
) -> MarketCatalog {
    MarketCatalog {
        source: source.to_owned(),
        title: index.title.clone(),
        description: index.description.clone(),
        fetched_at: fetched_at.to_owned(),
        platform: platform.to_owned(),
        packages: index
            .packages
            .iter()
            .map(|package| {
                let definition = installed
                    .iter()
                    .find(|definition| definition.manifest.id == package.id);
                listed(package, platform, definition)
            })
            .collect(),
    }
}

fn listed(
    package: &IndexPackage,
    platform: &str,
    installed: Option<&PluginDefinition>,
) -> MarketPackage {
    let mut versions: Vec<(semver::Version, MarketVersion)> = package
        .versions
        .iter()
        .filter_map(|version| {
            let parsed = semver::Version::parse(&version.version).ok()?;
            let artifact = version
                .packages
                .iter()
                .find(|artifact| artifact.platform == platform);
            Some((
                parsed.clone(),
                MarketVersion {
                    version: version.version.clone(),
                    published: version.published.clone(),
                    permissions: version.permissions.clone(),
                    prerelease: !parsed.pre.is_empty(),
                    size: artifact.map(|artifact| artifact.size),
                    sha256: artifact.map(|artifact| artifact.sha256.clone()),
                },
            ))
        })
        .collect();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    let available = |release: bool| {
        versions
            .iter()
            .find(|(parsed, version)| version.size.is_some() && parsed.pre.is_empty() == release)
    };
    let latest = available(true).or_else(|| available(false));
    let installed_version = installed.map(|definition| definition.manifest.version.clone());
    let status = match (installed, latest) {
        (Some(definition), _) if !definition.removable => MarketStatus::Bundled,
        (Some(definition), Some((offered, _))) => {
            match semver::Version::parse(&definition.manifest.version)
                .ok()
                .map(|current| offered.cmp(&current))
            {
                Some(Ordering::Equal) => MarketStatus::Installed,
                Some(Ordering::Less) => MarketStatus::InstalledNewer,
                Some(Ordering::Greater) | None => MarketStatus::UpdateAvailable,
            }
        }
        (Some(_), None) => MarketStatus::Installed,
        (None, Some(_)) => MarketStatus::NotInstalled,
        (None, None) => MarketStatus::Unavailable,
    };
    MarketPackage {
        id: package.id.clone(),
        name: package.name.clone(),
        description: package.description.clone(),
        publisher: package.publisher.clone(),
        license: package.license.clone(),
        locales: package.locales.clone(),
        latest: latest.map(|(_, version)| version.version.clone()),
        installed: installed_version,
        status,
        versions: versions.into_iter().map(|(_, version)| version).collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::core::extensions::plugin::PluginManifest;

    fn index(versions: &[(&str, &[&str])]) -> MarketIndex {
        let versions: Vec<serde_json::Value> = versions
            .iter()
            .map(|(version, platforms)| {
                serde_json::json!({
                    "version": version,
                    "published": "2026-11-25T08:00:00Z",
                    "packages": platforms.iter().map(|platform| serde_json::json!({
                        "platform": platform,
                        "url": format!("demo-{version}-{platform}.lcpkg"),
                        "size": 10,
                        "sha256": "b".repeat(64),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        MarketIndex::parse(
            &serde_json::to_vec(&serde_json::json!({
                "format": 1,
                "title": "Market",
                "packages": [{ "id": "org.example.demo", "name": "示例", "versions": versions }],
            }))
            .expect("json"),
        )
        .expect("index")
    }

    fn installed(version: &str, removable: bool) -> PluginDefinition {
        let mut definition = PluginDefinition::from_manifest(
            PathBuf::from("demo"),
            PluginManifest::parse(&format!(
                "id = \"org.example.demo\"\nname = \"示例\"\nversion = \"{version}\"\nprotocol = 2\nlibrary = \"demo\"\n"
            ))
            .expect("manifest"),
        );
        definition.removable = removable;
        definition
    }

    fn status(
        index: &MarketIndex,
        installed: &[PluginDefinition],
    ) -> (Option<String>, MarketStatus) {
        let listing = catalog("source", index, "now", "windows-x86_64", installed);
        let package = &listing.packages[0];
        (package.latest.clone(), package.status)
    }

    #[test]
    fn latest_is_the_newest_release_for_this_platform() {
        let offered = index(&[
            ("1.0.0", &["windows-x86_64"]),
            ("1.1.0", &["windows-x86_64", "linux-x86_64"]),
            ("1.2.0", &["linux-x86_64"]),
            ("2.0.0-rc.1", &["windows-x86_64"]),
        ]);
        let listing = catalog("source", &offered, "now", "windows-x86_64", &[]);
        assert_eq!(
            listing.packages[0]
                .versions
                .iter()
                .map(|version| (version.version.as_str(), version.size.is_some()))
                .collect::<Vec<_>>(),
            vec![
                ("2.0.0-rc.1", true),
                ("1.2.0", false),
                ("1.1.0", true),
                ("1.0.0", true)
            ]
        );
        assert_eq!(
            status(&offered, &[]),
            (Some("1.1.0".to_owned()), MarketStatus::NotInstalled)
        );
        // Only a pre-release: it is the latest.
        let pre = index(&[("2.0.0-rc.1", &["windows-x86_64"])]);
        assert_eq!(status(&pre, &[]).0.as_deref(), Some("2.0.0-rc.1"));
        let elsewhere = index(&[("1.0.0", &["linux-x86_64"])]);
        assert_eq!(status(&elsewhere, &[]), (None, MarketStatus::Unavailable));
    }

    #[test]
    fn status_compares_with_the_installed_version() {
        let offered = index(&[("1.1.0", &["windows-x86_64"])]);
        for (version, removable, expected) in [
            ("1.1.0", true, MarketStatus::Installed),
            ("1.0.0", true, MarketStatus::UpdateAvailable),
            ("2.0.0", true, MarketStatus::InstalledNewer),
            ("1.0.0", false, MarketStatus::Bundled),
        ] {
            assert_eq!(
                status(&offered, &[installed(version, removable)]).1,
                expected,
                "{version}"
            );
        }
    }
}
