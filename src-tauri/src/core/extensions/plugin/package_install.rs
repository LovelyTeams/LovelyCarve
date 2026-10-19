//! Installing and removing packages.
//!
//! A `.lcpkg` file is a ZIP archive of a package directory: `plugin.toml`
//! at the archive root (or inside one top-level directory), the library,
//! `schema.json` and `locales/`. Installing unpacks into a staging
//! directory under the install root, loads the package from there
//! (manifest, schema and locales are validated before anything is
//! replaced), then swaps it in as `<install root>/<plugin id>`. Bundled
//! packages next to the executable are never replaced or removed.

use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::{package::MANIFEST_FILE, PluginDefinition, PluginError, PluginManager, Result};
use crate::core::filesystem::tree::fs_tree_export::is_windows_device_name;

pub const PACKAGE_EXTENSION: &str = "lcpkg";
const MAX_ENTRIES: usize = 4096;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// What an installation did.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub plugin_id: String,
    pub version: String,
    /// The version it replaced.
    pub replaced: Option<String>,
}

impl PluginManager {
    /// Installs the package archive at `archive` (see the module docs).
    pub async fn install(&self, archive: PathBuf) -> Result<InstalledPackage> {
        let install_root = self
            .install_root()
            .ok_or_else(|| PluginError::Unavailable("no plugin install directory".to_owned()))?
            .to_owned();
        let staging = install_root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        let unpacked = {
            let staging = staging.clone();
            tokio::task::spawn_blocking(move || unpack(&archive, &staging))
                .await
                .map_err(|error| PluginError::Unavailable(error.to_string()))?
        };
        let definition = match unpacked {
            Ok(package_directory) => match PluginDefinition::load(&package_directory, true) {
                Ok(definition) => definition,
                Err(error) => {
                    remove(&staging);
                    return Err(error);
                }
            },
            Err(error) => {
                remove(&staging);
                return Err(error);
            }
        };
        let plugin_id = definition.manifest.id.clone();
        let existing = self.definition(&plugin_id).ok();
        if existing
            .as_ref()
            .is_some_and(|existing| !existing.removable)
        {
            remove(&staging);
            return Err(PluginError::Manifest(format!(
                "{plugin_id} is a bundled plugin and cannot be replaced"
            )));
        }
        if !definition
            .manifest
            .library_path(&definition.directory)
            .is_file()
        {
            remove(&staging);
            return Err(PluginError::Manifest(format!(
                "the package has no library {} for this platform",
                definition.manifest.library
            )));
        }
        // The shared instance holds the old library open.
        self.stop_shared(&plugin_id).await;
        let target = install_root.join(directory_name(&plugin_id));
        let swapped = tokio::task::spawn_blocking({
            let package_directory = definition.directory.clone();
            let staging = staging.clone();
            let target = target.clone();
            move || swap_in(&package_directory, &staging, &target)
        })
        .await
        .map_err(|error| PluginError::Unavailable(error.to_string()))?;
        remove(&staging);
        swapped?;
        self.rediscover();
        Ok(InstalledPackage {
            plugin_id,
            version: definition.manifest.version,
            replaced: existing.map(|existing| existing.manifest.version),
        })
    }

    /// Removes an installed package. Bundled packages cannot be removed.
    pub async fn uninstall(&self, plugin_id: &str) -> Result<()> {
        let definition = self.definition(plugin_id)?;
        if !definition.removable {
            return Err(PluginError::Manifest(format!(
                "{plugin_id} is a bundled plugin and cannot be removed"
            )));
        }
        self.stop_shared(plugin_id).await;
        let directory = definition.directory.clone();
        tokio::task::spawn_blocking(move || fs::remove_dir_all(&directory))
            .await
            .map_err(|error| PluginError::Unavailable(error.to_string()))?
            .map_err(|error| {
                PluginError::Unavailable(format!(
                    "{plugin_id} could not be removed (is one of its tasks still running?): {error}"
                ))
            })?;
        self.rediscover();
        Ok(())
    }
}

/// Unpacks `archive` into `staging`; returns the package directory inside
/// it (the staging directory, or its single top-level directory).
fn unpack(archive: &Path, staging: &Path) -> Result<PathBuf> {
    let invalid =
        |message: String| PluginError::Manifest(format!("{}: {message}", archive.display()));
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| invalid(error.to_string()))?;
    if zip.len() > MAX_ENTRIES {
        return Err(invalid(format!("more than {MAX_ENTRIES} entries")));
    }
    fs::create_dir_all(staging)?;
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| invalid(error.to_string()))?;
        let Some(name) = entry.enclosed_name() else {
            return Err(invalid(format!(
                "entry {:?} leaves the package",
                entry.name()
            )));
        };
        if name
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(invalid(format!(
                "entry {:?} is not a plain path",
                entry.name()
            )));
        }
        if entry.is_symlink() {
            return Err(invalid(format!("entry {:?} is a link", entry.name())));
        }
        let path = staging.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(invalid(format!("entry {:?} is too large", entry.name())));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_PACKAGE_BYTES {
            return Err(invalid("the package unpacks to more than 2 GiB".to_owned()));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut limited = (&mut entry).take(MAX_ENTRY_BYTES + 1);
        let copied = std::io::copy(&mut limited, &mut output)?;
        if copied > MAX_ENTRY_BYTES {
            return Err(invalid(format!("entry {:?} is too large", entry.name())));
        }
        output.flush()?;
    }
    if staging.join(MANIFEST_FILE).is_file() {
        return Ok(staging.to_owned());
    }
    let mut directories = fs::read_dir(staging)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir());
    match (directories.next(), directories.next()) {
        (Some(only), None) if only.path().join(MANIFEST_FILE).is_file() => Ok(only.path()),
        _ => Err(invalid(format!("no {MANIFEST_FILE} at the package root"))),
    }
}

/// Moves `package_directory` (inside `staging`) to `target`, replacing an
/// existing directory only once the new one is in place.
fn swap_in(package_directory: &Path, staging: &Path, target: &Path) -> Result<()> {
    let old = staging.with_file_name(format!(".old-{}", uuid::Uuid::new_v4().simple()));
    let had_old = target.exists();
    if had_old {
        fs::rename(target, &old).map_err(|error| {
            PluginError::Unavailable(format!(
                "the installed version is in use (stop its tasks and retry): {error}"
            ))
        })?;
    }
    if let Err(error) = fs::rename(package_directory, target) {
        if had_old {
            if let Err(restore) = fs::rename(&old, target) {
                tracing::error!(target = %target.display(), "restoring the previous package failed: {restore}");
            }
        }
        return Err(error.into());
    }
    if had_old {
        remove(&old);
    }
    Ok(())
}

fn directory_name(plugin_id: &str) -> String {
    if is_windows_device_name(plugin_id.split('.').next().unwrap_or(plugin_id)) {
        format!("_{plugin_id}")
    } else {
        plugin_id.to_owned()
    }
}

fn remove(directory: &Path) {
    if let Err(error) = fs::remove_dir_all(directory) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %directory.display(), "package staging cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).expect("archive"));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).expect("entry");
            writer.write_all(content).expect("content");
        }
        writer.finish().expect("finish");
    }

    fn manifest(version: &str) -> Vec<u8> {
        format!(
            "id = \"org.test.installed\"\nname = \"Installed\"\nversion = \"{version}\"\nprotocol = 2\nlibrary = \"plugin.dll\"\n"
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn packages_install_replace_and_uninstall() {
        let install = tempfile::tempdir().expect("install root");
        let sources = tempfile::tempdir().expect("archives");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");

        let first = sources.path().join("first.lcpkg");
        archive(
            &first,
            &[
                ("demo/plugin.toml", &manifest("1.0")),
                ("demo/plugin.dll", b"library"),
                ("demo/locales/en.toml", b"\"plugin.name\" = \"Installed\"\n"),
            ],
        );
        let installed = manager.install(first).await.expect("install");
        assert_eq!(
            installed,
            InstalledPackage {
                plugin_id: "org.test.installed".to_owned(),
                version: "1.0".to_owned(),
                replaced: None,
            }
        );
        let definition = manager.definition("org.test.installed").expect("installed");
        assert!(definition.removable);
        assert_eq!(definition.locales["en"]["plugin.name"], "Installed");

        let second = sources.path().join("second.lcpkg");
        archive(
            &second,
            &[
                ("plugin.toml", &manifest("2.0")),
                ("plugin.dll", b"library v2"),
            ],
        );
        let replaced = manager.install(second).await.expect("replace");
        assert_eq!(replaced.replaced.as_deref(), Some("1.0"));
        assert_eq!(
            manager
                .definition("org.test.installed")
                .expect("replaced")
                .manifest
                .version,
            "2.0"
        );
        // Only the package directory remains: no staging or old copies.
        assert_eq!(
            fs::read_dir(install.path()).expect("root").count(),
            1,
            "staging and old directories are removed"
        );

        manager
            .uninstall("org.test.installed")
            .await
            .expect("uninstall");
        assert!(manager.definition("org.test.installed").is_err());
    }

    /// A package `just pack-plugins` built (`LOVELYCARVE_TEST_LCPKG`) installs
    /// into a scratch root, and its library starts and matches the package's
    /// `schema.json`. Needs the `lc-plugin-host` of a plugin build.
    #[tokio::test]
    async fn built_package_installs_and_starts_when_env_path_is_set() {
        let Some(archive) = std::env::var_os("LOVELYCARVE_TEST_LCPKG") else {
            return;
        };
        let install = tempfile::tempdir().expect("install root");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
        let installed = manager.install(archive.into()).await.expect("install");
        let definition = manager.definition(&installed.plugin_id).expect("installed");
        assert!(definition.removable);
        assert!(
            !definition.record_schemas().is_empty(),
            "schema.json is packed"
        );
        assert!(definition.locales.contains_key("en"), "locales are packed");
        let worker = manager
            .start_worker(&installed.plugin_id)
            .await
            .expect("the library starts and matches schema.json");
        assert_eq!(worker.hello().plugin, installed.plugin_id);
        worker.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn broken_archives_change_nothing() {
        let install = tempfile::tempdir().expect("install root");
        let sources = tempfile::tempdir().expect("archives");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
        for (name, entries) in [
            (
                "escape.lcpkg",
                vec![("../plugin.toml", manifest("1").as_slice())],
            ),
            (
                "nolib.lcpkg",
                vec![("plugin.toml", manifest("1").as_slice())],
            ),
            ("empty.lcpkg", vec![("readme.txt", b"nothing".as_slice())]),
        ] {
            let path = sources.path().join(name);
            let entries: Vec<(&str, &[u8])> = entries.into_iter().collect();
            archive(&path, &entries);
            assert!(manager.install(path).await.is_err(), "{name}");
        }
        assert_eq!(fs::read_dir(install.path()).expect("root").count(), 0);
    }
}
