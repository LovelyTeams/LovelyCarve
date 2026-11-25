//! Installing and removing packages.
//!
//! A `.lcpkg` file is a ZIP archive of a package directory (see
//! [`super::package_archive`]). Installing takes the SHA-256 of the archive
//! the user inspected ([`super::package_inspect`]) and refuses other bytes.
//! It unpacks into a staging directory under the install root, loads the
//! package from there (manifest, schema and locales), starts the staged
//! library once in `lc-plugin-host` and checks its `plugin.hello` against
//! the package — the same check every run makes — and only then swaps it
//! in as `<install root>/<plugin id>`. A package that fails any step leaves
//! the installed version in place. Bundled packages next to the executable
//! are never replaced or removed.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::Serialize;

use super::{
    package_archive::{self, digest_file},
    package_inspect::invalid_package,
    PackageRejection, PluginConnection, PluginDefinition, PluginError, PluginManager, Result,
    SourceRegistry,
};
use crate::core::filesystem::tree::fs_tree_export::is_windows_device_name;

pub const PACKAGE_EXTENSION: &str = "lcpkg";

/// Staging and replaced directories older than this belong to an install
/// that was interrupted (another process's install is younger).
const ABANDONED_AFTER: Duration = Duration::from_secs(60 * 60);

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
    /// Installs the package archive at `archive`, whose SHA-256 must be
    /// `sha256` (see the module docs).
    pub async fn install(&self, archive: PathBuf, sha256: &str) -> Result<InstalledPackage> {
        let install_root = self
            .install_root()
            .ok_or(PackageRejection::NoInstallRoot)?
            .to_owned();
        let staging = install_root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        let installed = self
            .install_staged(&archive, sha256, &install_root, &staging)
            .await;
        remove(&staging);
        installed
    }

    async fn install_staged(
        &self,
        archive: &Path,
        sha256: &str,
        install_root: &Path,
        staging: &Path,
    ) -> Result<InstalledPackage> {
        let package_directory = {
            let archive = archive.to_owned();
            let staging = staging.to_owned();
            let expected = sha256.to_ascii_lowercase();
            tokio::task::spawn_blocking(move || {
                let digest = digest_file(&archive).map_err(|error| {
                    PackageRejection::InvalidArchive(format!("{}: {error}", archive.display()))
                })?;
                if digest.sha256 != expected {
                    return Err(PackageRejection::Changed {
                        expected,
                        actual: digest.sha256,
                    }
                    .into());
                }
                package_archive::unpack(&archive, &staging)
            })
            .await
            .map_err(|error| PluginError::Unavailable(error.to_string()))??
        };
        let definition =
            PluginDefinition::load(&package_directory, true).map_err(invalid_package)?;
        let plugin_id = definition.manifest.id.clone();
        let existing = self.definition(&plugin_id).ok();
        if existing
            .as_ref()
            .is_some_and(|existing| !existing.removable)
        {
            return Err(PackageRejection::Bundled { plugin_id }.into());
        }
        if !definition
            .manifest
            .library_path(&definition.directory)
            .is_file()
        {
            return Err(PackageRejection::NoLibrary {
                library: definition.manifest.library.clone(),
            }
            .into());
        }
        self.check_library(&definition).await?;
        // The shared instance holds the old library open.
        self.stop_shared(&plugin_id).await;
        let target = install_root.join(directory_name(&plugin_id));
        tokio::task::spawn_blocking({
            let package_directory = definition.directory.clone();
            let target = target.clone();
            move || swap_in(&package_directory, &target)
        })
        .await
        .map_err(|error| PluginError::Unavailable(error.to_string()))??;
        self.rediscover();
        Ok(InstalledPackage {
            plugin_id,
            version: definition.manifest.version,
            replaced: existing.map(|existing| existing.manifest.version),
        })
    }

    /// Starts the staged library once and checks its `plugin.hello`
    /// against the package, then lets it exit.
    async fn check_library(&self, definition: &PluginDefinition) -> Result<()> {
        #[cfg(test)]
        if let Some(entry) = self.loopback_entry(definition.id()) {
            let connection =
                PluginConnection::loopback(entry, definition, SourceRegistry::default())
                    .await
                    .map_err(|error| PackageRejection::LibraryRejected(error.to_string()))?;
            return finish_check(definition, connection).await;
        }
        let connection =
            PluginConnection::open(definition, SourceRegistry::default(), self.log_store())
                .await
                .map_err(|error| PackageRejection::LibraryRejected(error.to_string()))?;
        finish_check(definition, connection).await
    }

    /// Removes an installed package. Bundled packages cannot be removed.
    pub async fn uninstall(&self, plugin_id: &str) -> Result<()> {
        let definition = self.definition(plugin_id)?;
        if !definition.removable {
            return Err(PackageRejection::Bundled {
                plugin_id: plugin_id.to_owned(),
            }
            .into());
        }
        self.stop_shared(plugin_id).await;
        let directory = definition.directory.clone();
        tokio::task::spawn_blocking(move || fs::remove_dir_all(&directory))
            .await
            .map_err(|error| PluginError::Unavailable(error.to_string()))?
            .map_err(|error| PackageRejection::InUse(error.to_string()))?;
        self.rediscover();
        Ok(())
    }
}

/// Ends the check instance and waits for its process, which holds the
/// staged library open until it exits.
async fn finish_check(definition: &PluginDefinition, connection: PluginConnection) -> Result<()> {
    if let Err(error) = connection.shutdown().await {
        tracing::warn!(plugin_id = %definition.id(), "plugin check instance did not shut down cleanly: {error}");
    }
    Ok(())
}

/// Moves `package_directory` to `target`, replacing an existing directory
/// only once the new one is in place.
fn swap_in(package_directory: &Path, target: &Path) -> Result<()> {
    let old = target.with_file_name(format!(".old-{}", uuid::Uuid::new_v4().simple()));
    let had_old = target.exists();
    if had_old {
        fs::rename(target, &old).map_err(|error| PackageRejection::InUse(error.to_string()))?;
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

/// Removes what interrupted installs left in `install_root` (`.staging-*`
/// and `.old-*` directories) once they are [`ABANDONED_AFTER`] older than
/// `now`.
pub(super) fn sweep_abandoned(install_root: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(install_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(".staging-") || name.starts_with(".old-")) {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ABANDONED_AFTER);
        if abandoned {
            remove(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use lovelycarve_plugin::{
        abi::{HostApiV1, PluginApiV1},
        runtime, Plugin,
    };

    use super::*;
    use crate::core::extensions::plugin::PackageChange;

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

    unsafe extern "C" fn installed_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
        unsafe { runtime::entry(host, plugin, || Plugin::new("org.test.installed")) }
    }

    unsafe extern "C" fn impostor_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
        unsafe { runtime::entry(host, plugin, || Plugin::new("org.test.impostor")) }
    }

    fn sha256(path: &Path) -> String {
        digest_file(path).expect("digest").sha256
    }

    #[tokio::test]
    async fn packages_install_replace_and_uninstall() {
        let install = tempfile::tempdir().expect("install root");
        let sources = tempfile::tempdir().expect("archives");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
        manager.serve_in_process("org.test.installed", installed_entry);

        let first = sources.path().join("first.lcpkg");
        archive(
            &first,
            &[
                ("demo/plugin.toml", &manifest("1.0.0")),
                ("demo/plugin.dll", b"library"),
                ("demo/locales/en.toml", b"\"plugin.name\" = \"Installed\"\n"),
            ],
        );
        let inspection = manager.inspect(first.clone()).await.expect("inspect");
        assert_eq!(
            (
                inspection.change,
                inspection.blocked.clone(),
                inspection.installed.clone()
            ),
            (PackageChange::New, None, None)
        );
        let installed = manager
            .install(first, &inspection.sha256)
            .await
            .expect("install");
        assert_eq!(
            installed,
            InstalledPackage {
                plugin_id: "org.test.installed".to_owned(),
                version: "1.0.0".to_owned(),
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
                ("plugin.toml", &manifest("2.0.0")),
                ("plugin.dll", b"library v2"),
            ],
        );
        let inspection = manager.inspect(second.clone()).await.expect("inspect");
        assert_eq!(inspection.change, PackageChange::Upgrade);
        assert_eq!(
            inspection.installed.map(|installed| installed.version),
            Some("1.0.0".to_owned())
        );
        let replaced = manager
            .install(second.clone(), &inspection.sha256)
            .await
            .expect("replace");
        assert_eq!(replaced.replaced.as_deref(), Some("1.0.0"));
        assert_eq!(
            manager
                .definition("org.test.installed")
                .expect("replaced")
                .manifest
                .version,
            "2.0.0"
        );
        // Only the package directory remains: no staging or old copies.
        assert_eq!(
            fs::read_dir(install.path()).expect("root").count(),
            1,
            "staging and old directories are removed"
        );
        assert_eq!(
            manager.inspect(second).await.expect("again").change,
            PackageChange::Reinstall
        );
        let rebuilt = sources.path().join("rebuilt.lcpkg");
        archive(
            &rebuilt,
            &[
                ("plugin.toml", &manifest("2.0.0")),
                ("plugin.dll", b"library v2, rebuilt"),
            ],
        );
        assert_eq!(
            manager.inspect(rebuilt).await.expect("rebuilt").change,
            PackageChange::Rebuild
        );
        let older = sources.path().join("older.lcpkg");
        archive(
            &older,
            &[("plugin.toml", &manifest("1.5.0")), ("plugin.dll", b"old")],
        );
        assert_eq!(
            manager.inspect(older).await.expect("older").change,
            PackageChange::Downgrade
        );

        manager
            .uninstall("org.test.installed")
            .await
            .expect("uninstall");
        assert!(manager.definition("org.test.installed").is_err());
    }

    #[tokio::test]
    async fn a_package_that_fails_its_check_leaves_the_installed_version() {
        let install = tempfile::tempdir().expect("install root");
        let sources = tempfile::tempdir().expect("archives");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
        manager.serve_in_process("org.test.installed", installed_entry);
        let first = sources.path().join("first.lcpkg");
        archive(
            &first,
            &[("plugin.toml", &manifest("1.0.0")), ("plugin.dll", b"v1")],
        );
        manager
            .install(first.clone(), &sha256(&first))
            .await
            .expect("install");

        // A library that answers as another plugin.
        manager.serve_in_process("org.test.installed", impostor_entry);
        let second = sources.path().join("second.lcpkg");
        archive(
            &second,
            &[("plugin.toml", &manifest("2.0.0")), ("plugin.dll", b"v2")],
        );
        let error = manager
            .install(second.clone(), &sha256(&second))
            .await
            .expect_err("impostor library");
        assert!(
            matches!(
                &error,
                PluginError::Package(PackageRejection::LibraryRejected(_))
            ),
            "{error}"
        );
        assert_eq!(
            manager
                .definition("org.test.installed")
                .expect("kept")
                .manifest
                .version,
            "1.0.0"
        );

        // Other bytes than the inspected ones.
        let error = manager
            .install(second, &sha256(&first))
            .await
            .expect_err("changed archive");
        assert!(
            matches!(
                &error,
                PluginError::Package(PackageRejection::Changed { .. })
            ),
            "{error}"
        );
        assert_eq!(fs::read_dir(install.path()).expect("root").count(), 1);
    }

    /// A package `just pack-plugins` built (`LOVELYCARVE_TEST_LCPKG`) installs
    /// into a scratch root, and its library starts and matches the package's
    /// `schema.json`. Needs the `lc-plugin-host` of a plugin build.
    #[tokio::test]
    async fn built_package_installs_and_starts_when_env_path_is_set() {
        let Some(archive) = std::env::var_os("LOVELYCARVE_TEST_LCPKG") else {
            return;
        };
        let archive = PathBuf::from(archive);
        let install = tempfile::tempdir().expect("install root");
        let manager =
            PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
        let inspection = manager.inspect(archive.clone()).await.expect("inspect");
        assert!(inspection.blocked.is_none(), "{:?}", inspection.blocked);
        let installed = manager
            .install(archive, &inspection.sha256)
            .await
            .expect("install");
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
        manager.serve_in_process("org.test.installed", installed_entry);
        for (name, entries, reason) in [
            (
                "escape.lcpkg",
                vec![("../plugin.toml", manifest("1.0.0").as_slice())],
                "invalidArchive",
            ),
            (
                "nolib.lcpkg",
                vec![("plugin.toml", manifest("1.0.0").as_slice())],
                "noLibrary",
            ),
            (
                "version.lcpkg",
                vec![
                    ("plugin.toml", manifest("1").as_slice()),
                    ("plugin.dll", b"library".as_slice()),
                ],
                "invalidPackage",
            ),
            (
                "empty.lcpkg",
                vec![("readme.txt", b"nothing".as_slice())],
                "invalidArchive",
            ),
        ] {
            let path = sources.path().join(name);
            let entries: Vec<(&str, &[u8])> = entries.into_iter().collect();
            archive(&path, &entries);
            let error = manager
                .install(path.clone(), &sha256(&path))
                .await
                .expect_err(name);
            let PluginError::Package(rejection) = &error else {
                panic!("{name}: {error}");
            };
            assert_eq!(rejection.reason(), reason, "{name}: {error}");
        }
        assert_eq!(fs::read_dir(install.path()).expect("root").count(), 0);
    }

    #[test]
    fn abandoned_staging_is_swept_and_recent_staging_kept() {
        let install = tempfile::tempdir().expect("install root");
        let staging = install.path().join(".staging-interrupted");
        let replaced = install.path().join(".old-interrupted");
        let package = install.path().join("org.test.package");
        for directory in [&staging, &replaced, &package] {
            fs::create_dir_all(directory).expect("directory");
        }
        // Another process may still be installing from a recent one.
        sweep_abandoned(install.path(), SystemTime::now());
        assert!(staging.exists() && replaced.exists());
        sweep_abandoned(install.path(), SystemTime::now() + 2 * ABANDONED_AFTER);
        assert!(!staging.exists() && !replaced.exists());
        assert!(package.exists());
    }
}
