//! The plugin index format (`index.json`, format 1): a market's title and
//! the packages it offers, each version with one `.lcpkg` per platform and
//! that file's size and SHA-256.
//!
//! The reader is strict. Unknown members, a `format` other than 1 or any
//! broken rule rejects the whole index, naming the member that broke it:
//! a bad index is a publisher's error, and dropping entries quietly would
//! hide it. The SHA-256 is what installs: a downloaded package must have
//! it, and the package must then name the listed id and version.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::core::extensions::plugin::{
    is_valid_plugin_id, Locales, PluginPermission, MAX_ATTRIBUTION, MAX_DESCRIPTION,
    MAX_PACKAGE_BYTES,
};

/// The format this build reads and writes.
pub const INDEX_FORMAT: u32 = 1;
/// The index file of a market directory.
pub const INDEX_FILE: &str = "index.json";
/// Largest index read.
pub const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

const MAX_TITLE: usize = 256;
const MAX_PACKAGES: usize = 10_000;
const MAX_VERSIONS: usize = 256;
const MAX_ARTIFACTS: usize = 16;
const MAX_URL: usize = 2048;
const MAX_LOCALE_TEXT: usize = 4096;
/// The locale keys an index carries for a package.
pub const LOCALE_KEYS: [&str; 2] = ["plugin.name", "plugin.description"];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MarketIndex {
    pub format: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub packages: Vec<IndexPackage>,
}

/// One plugin and the versions offered.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IndexPackage {
    pub id: String,
    /// The Chinese default name, as in `plugin.toml`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Translations of `plugin.name` and `plugin.description` by locale.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locales: Locales,
    pub versions: Vec<IndexVersion>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IndexVersion {
    /// A semantic version, the package's `plugin.toml` version.
    pub version: String,
    /// When it was published (RFC 3339).
    pub published: String,
    /// What the package's manifest declares (the installed package's own
    /// manifest decides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<PluginPermission>,
    pub packages: Vec<IndexArtifact>,
}

/// One `.lcpkg` of a version.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IndexArtifact {
    /// `<os>-<arch>`: `windows-x86_64`, `linux-aarch64`.
    pub platform: String,
    /// The file: a path relative to the index, or an absolute `https://` URL.
    pub url: String,
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the file.
    pub sha256: String,
}

impl MarketIndex {
    /// Parses and validates an index (see the module docs).
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let index: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        index.validate()?;
        Ok(index)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != INDEX_FORMAT {
            return Err(format!(
                "format: {} is not supported; this build reads format {INDEX_FORMAT}",
                self.format
            ));
        }
        text("title", &self.title, MAX_TITLE)?;
        optional_text("description", self.description.as_deref(), MAX_DESCRIPTION)?;
        if self.packages.len() > MAX_PACKAGES {
            return Err(format!("packages: more than {MAX_PACKAGES}"));
        }
        let mut ids = HashSet::new();
        for (index, package) in self.packages.iter().enumerate() {
            let at = format!("packages[{index}]");
            package.validate(&at)?;
            if !ids.insert(package.id.as_str()) {
                return Err(format!("{at}.id: {} is listed twice", package.id));
            }
        }
        Ok(())
    }
}

impl IndexPackage {
    fn validate(&self, at: &str) -> Result<(), String> {
        if !is_valid_plugin_id(&self.id) {
            return Err(format!(
                "{at}.id: must be 1..=128 ASCII letters, digits, '.', '-' or '_'"
            ));
        }
        text(&format!("{at}.name"), &self.name, MAX_TITLE)?;
        optional_text(
            &format!("{at}.description"),
            self.description.as_deref(),
            MAX_DESCRIPTION,
        )?;
        optional_text(
            &format!("{at}.publisher"),
            self.publisher.as_deref(),
            MAX_ATTRIBUTION,
        )?;
        optional_text(
            &format!("{at}.license"),
            self.license.as_deref(),
            MAX_ATTRIBUTION,
        )?;
        for (locale, entries) in &self.locales {
            if !valid_locale(locale) {
                return Err(format!(
                    "{at}.locales.{locale}: locales are named like en or zh-CN"
                ));
            }
            for (key, value) in entries {
                if !LOCALE_KEYS.contains(&key.as_str()) {
                    return Err(format!(
                        "{at}.locales.{locale}.{key}: only {} are listed",
                        LOCALE_KEYS.join(" and ")
                    ));
                }
                text(
                    &format!("{at}.locales.{locale}.{key}"),
                    value,
                    MAX_LOCALE_TEXT,
                )?;
            }
        }
        if self.versions.is_empty() || self.versions.len() > MAX_VERSIONS {
            return Err(format!("{at}.versions: 1..={MAX_VERSIONS} versions"));
        }
        let mut versions = HashSet::new();
        for (index, version) in self.versions.iter().enumerate() {
            let at = format!("{at}.versions[{index}]");
            let parsed = semver::Version::parse(&version.version)
                .map_err(|error| format!("{at}.version: {error}"))?;
            if !versions.insert(parsed) {
                return Err(format!("{at}.version: {} is listed twice", version.version));
            }
            version.validate(&at)?;
        }
        Ok(())
    }
}

impl IndexVersion {
    fn validate(&self, at: &str) -> Result<(), String> {
        chrono::DateTime::parse_from_rfc3339(&self.published)
            .map_err(|error| format!("{at}.published: not an RFC 3339 time: {error}"))?;
        let mut permissions = HashSet::new();
        if self
            .permissions
            .iter()
            .any(|permission| !permissions.insert(*permission))
        {
            return Err(format!("{at}.permissions: listed twice"));
        }
        if self.packages.is_empty() || self.packages.len() > MAX_ARTIFACTS {
            return Err(format!("{at}.packages: 1..={MAX_ARTIFACTS} packages"));
        }
        let mut platforms = HashSet::new();
        for (index, artifact) in self.packages.iter().enumerate() {
            let at = format!("{at}.packages[{index}]");
            artifact.validate(&at)?;
            if !platforms.insert(artifact.platform.as_str()) {
                return Err(format!(
                    "{at}.platform: {} is listed twice",
                    artifact.platform
                ));
            }
        }
        Ok(())
    }
}

impl IndexArtifact {
    fn validate(&self, at: &str) -> Result<(), String> {
        if !valid_platform(&self.platform) {
            return Err(format!(
                "{at}.platform: must be <os>-<arch> such as windows-x86_64"
            ));
        }
        check_url(&self.url).map_err(|message| format!("{at}.url: {message}"))?;
        if self.size == 0 || self.size > MAX_PACKAGE_BYTES {
            return Err(format!("{at}.size: 1 byte to 2 GiB"));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "{at}.sha256: must be 64 lowercase hexadecimal digits"
            ));
        }
        Ok(())
    }
}

/// A package URL: a relative path inside the index's directory, or an
/// absolute `https://` URL without credentials or fragment.
pub fn check_url(url: &str) -> Result<(), &'static str> {
    if url.is_empty() || url.len() > MAX_URL || url.chars().any(char::is_control) {
        return Err("must be a path or an https:// URL of at most 2048 characters");
    }
    if url.contains("://") {
        let parsed = reqwest::Url::parse(url).map_err(|_| "is not a valid URL")?;
        if parsed.scheme() != "https" {
            return Err("only https:// URLs are allowed");
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("must not carry credentials");
        }
        if parsed.fragment().is_some() {
            return Err("must not carry a fragment");
        }
        return Ok(());
    }
    if url.contains(['\\', '?', '#', ':']) || url.starts_with('/') {
        return Err("a relative path uses '/' and stays inside the index's directory");
    }
    if url
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("a relative path uses '/' and stays inside the index's directory");
    }
    Ok(())
}

fn valid_platform(platform: &str) -> bool {
    let mut parts = platform.split('-');
    let (Some(os), Some(arch), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [os, arch].iter().all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

/// `en`, `zh-CN`.
fn valid_locale(locale: &str) -> bool {
    let mut parts = locale.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next();
    parts.next().is_none()
        && (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_lowercase())
        && region.is_none_or(|region| {
            region.len() == 2 && region.bytes().all(|byte| byte.is_ascii_uppercase())
        })
}

fn text(at: &str, value: &str, limit: usize) -> Result<(), String> {
    if value.trim().is_empty()
        || value.chars().count() > limit
        || value.chars().any(char::is_control)
    {
        return Err(format!("{at}: one line of 1..={limit} characters"));
    }
    Ok(())
}

fn optional_text(at: &str, value: Option<&str>, limit: usize) -> Result<(), String> {
    value.map_or(Ok(()), |value| text(at, value, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "format": 1,
            "title": "示例市场",
            "packages": [{
                "id": "org.example.demo",
                "name": "示例",
                "description": "示例插件",
                "publisher": "Example",
                "license": "MIT",
                "locales": { "en": { "plugin.name": "Demo" } },
                "versions": [{
                    "version": "1.2.0",
                    "published": "2026-11-25T08:00:00Z",
                    "permissions": ["file_materialize"],
                    "packages": [{
                        "platform": "windows-x86_64",
                        "url": "org.example.demo-1.2.0-windows-x86_64.lcpkg",
                        "size": 1024,
                        "sha256": "a".repeat(64),
                    }],
                }],
            }],
        })
    }

    fn parse(value: &serde_json::Value) -> Result<MarketIndex, String> {
        MarketIndex::parse(&serde_json::to_vec(value).expect("json"))
    }

    #[test]
    fn valid_indexes_round_trip() {
        let index = parse(&sample()).expect("index");
        assert_eq!(index.packages[0].versions[0].packages[0].size, 1024);
        assert_eq!(
            serde_json::to_value(&index).expect("serialize"),
            sample(),
            "writing gives back the same document"
        );
    }

    #[test]
    fn every_broken_rule_rejects_the_index_and_names_the_member() {
        let cases: Vec<(fn(&mut serde_json::Value), &str)> = vec![
            (|index| index["format"] = 2.into(), "format"),
            (|index| index["extra"] = true.into(), "unknown field"),
            (|index| index["title"] = "".into(), "title"),
            (
                |index| index["packages"][0]["id"] = "bad id".into(),
                "packages[0].id",
            ),
            (
                |index| {
                    let duplicate = index["packages"][0].clone();
                    index["packages"]
                        .as_array_mut()
                        .expect("packages")
                        .push(duplicate);
                },
                "packages[1].id",
            ),
            (
                |index| index["packages"][0]["versions"][0]["version"] = "1.2".into(),
                "packages[0].versions[0].version",
            ),
            (
                |index| index["packages"][0]["versions"][0]["published"] = "yesterday".into(),
                "published",
            ),
            (
                |index| {
                    index["packages"][0]["versions"][0]["packages"][0]["url"] =
                        "../elsewhere.lcpkg".into()
                },
                "url",
            ),
            (
                |index| {
                    index["packages"][0]["versions"][0]["packages"][0]["url"] =
                        "http://example.com/demo.lcpkg".into()
                },
                "url",
            ),
            (
                |index| {
                    index["packages"][0]["versions"][0]["packages"][0]["sha256"] =
                        "A".repeat(64).into()
                },
                "sha256",
            ),
            (
                |index| index["packages"][0]["versions"][0]["packages"][0]["size"] = 0.into(),
                "size",
            ),
            (
                |index| {
                    index["packages"][0]["versions"][0]["packages"][0]["platform"] =
                        "windows".into()
                },
                "platform",
            ),
            (
                |index| index["packages"][0]["locales"]["en"]["record.x"] = "X".into(),
                "locales.en.record.x",
            ),
        ];
        for (break_rule, member) in cases {
            let mut index = sample();
            break_rule(&mut index);
            let error = parse(&index).expect_err(member);
            assert!(error.contains(member), "{member}: {error}");
        }
    }

    #[test]
    fn package_urls_are_relative_paths_or_https() {
        assert!(check_url("demo.lcpkg").is_ok());
        assert!(check_url("files/demo.lcpkg").is_ok());
        assert!(check_url("https://example.com/files/demo.lcpkg?token=1").is_ok());
        for bad in [
            "",
            "/demo.lcpkg",
            "files\\demo.lcpkg",
            "a/../demo.lcpkg",
            "C:demo.lcpkg",
            "ftp://example.com/demo.lcpkg",
            "https://user:secret@example.com/demo.lcpkg",
            "https://example.com/demo.lcpkg#part",
        ] {
            assert!(check_url(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
