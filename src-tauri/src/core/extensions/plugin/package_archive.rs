//! Reading a `.lcpkg` archive: a ZIP of one package directory, with
//! `plugin.toml` at the archive root or inside its single top-level
//! directory.
//!
//! Every entry is checked before anything is read or written: at most
//! [`MAX_ENTRIES`] entries, plain relative paths that stay inside the
//! package, names Windows can hold (no device names, reserved characters or
//! trailing dots and spaces), no links, no two entries for one path, and
//! bounded sizes. Inspecting reads single files from the archive; installing
//! unpacks all of it. Neither runs anything from the package.

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

use super::{package::MANIFEST_FILE, PackageRejection, Result};
use crate::core::filesystem::tree::fs_tree_export::is_windows_device_name;

/// Most entries one archive holds.
pub const MAX_ENTRIES: usize = 4096;
/// Largest single entry, unpacked.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// Largest archive, and largest unpacked total.
pub const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A file's SHA-256 (lowercase hex) and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDigest {
    pub sha256: String,
    pub size: u64,
}

/// Hashes a whole file.
pub fn digest_file(path: &Path) -> std::io::Result<FileDigest> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut size = 0_u64;
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hasher.update(&buffer[..length]);
        size += length as u64;
    }
    Ok(FileDigest {
        sha256: hex::encode(hasher.finalize()),
        size,
    })
}

/// An opened archive whose entries all passed the checks.
pub struct PackageArchive {
    zip: zip::ZipArchive<fs::File>,
    /// Entry index by path relative to the package root (`/`-separated,
    /// as written in the archive).
    files: HashMap<String, usize>,
    /// Entries that are directories, by the same paths.
    directories: Vec<String>,
}

impl PackageArchive {
    pub fn open(path: &Path) -> Result<Self> {
        let invalid = |message: String| invalid_archive(path, message);
        let file = fs::File::open(path).map_err(|error| invalid(error.to_string()))?;
        let length = file
            .metadata()
            .map_err(|error| invalid(error.to_string()))?
            .len();
        if length > MAX_PACKAGE_BYTES {
            return Err(invalid("the archive is larger than 2 GiB".to_owned()));
        }
        let mut zip = zip::ZipArchive::new(file).map_err(|error| invalid(error.to_string()))?;
        if zip.len() > MAX_ENTRIES {
            return Err(invalid(format!("more than {MAX_ENTRIES} entries")));
        }
        let mut entries = Vec::with_capacity(zip.len());
        let mut seen = HashMap::new();
        let mut total = 0_u64;
        for index in 0..zip.len() {
            let entry = zip
                .by_index(index)
                .map_err(|error| invalid(error.to_string()))?;
            let raw = entry
                .name()
                .map_err(|error| invalid(error.to_string()))?
                .into_owned();
            let path =
                entry_path(&raw).map_err(|message| invalid(format!("{raw:?}: {message}")))?;
            if entry.is_symlink() {
                return Err(invalid(format!("entry {raw:?} is a link")));
            }
            if entry.size() > MAX_ENTRY_BYTES {
                return Err(invalid(format!("entry {raw:?} is too large")));
            }
            total = total.saturating_add(entry.size());
            if total > MAX_PACKAGE_BYTES {
                return Err(invalid("the package unpacks to more than 2 GiB".to_owned()));
            }
            // One entry per path, also where Windows folds case.
            if let Some(previous) = seen.insert(path.to_ascii_lowercase(), raw.clone()) {
                return Err(invalid(format!(
                    "entries {previous:?} and {raw:?} name the same file"
                )));
            }
            entries.push((path, entry.is_dir(), index));
        }
        let root = package_root(&entries)
            .ok_or_else(|| invalid(format!("no {MANIFEST_FILE} at the package root")))?;
        let mut files = HashMap::new();
        let mut directories = Vec::new();
        for (path, is_directory, index) in entries {
            let Some(relative) = path.strip_prefix(&root) else {
                // Files next to the single top-level directory are not part
                // of the package.
                continue;
            };
            if relative.is_empty() {
                continue;
            }
            if is_directory {
                directories.push(relative.to_owned());
            } else {
                files.insert(relative.to_owned(), index);
            }
        }
        Ok(Self {
            zip,
            files,
            directories,
        })
    }

    /// Whether the package holds `relative`.
    pub fn contains(&self, relative: &str) -> bool {
        self.files.contains_key(relative)
    }

    /// Files of the package directly inside `directory` (no trailing `/`).
    pub fn files_in(&self, directory: &str) -> Vec<String> {
        let prefix = format!("{directory}/");
        let mut names: Vec<String> = self
            .files
            .keys()
            .filter(|path| {
                path.strip_prefix(&prefix)
                    .is_some_and(|name| !name.is_empty() && !name.contains('/'))
            })
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The bytes of `relative`, if the package holds it and it fits `limit`.
    pub fn read(&mut self, relative: &str, limit: u64) -> Result<Option<Vec<u8>>> {
        let Some(&index) = self.files.get(relative) else {
            return Ok(None);
        };
        let mut entry = self
            .zip
            .by_index(index)
            .map_err(|error| PackageRejection::InvalidArchive(error.to_string()))?;
        if entry.size() > limit {
            return Err(PackageRejection::InvalidPackage(format!(
                "{relative} is larger than {limit} bytes"
            ))
            .into());
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| PackageRejection::InvalidArchive(format!("{relative}: {error}")))?;
        if bytes.len() as u64 > limit {
            return Err(PackageRejection::InvalidPackage(format!(
                "{relative} is larger than {limit} bytes"
            ))
            .into());
        }
        Ok(Some(bytes))
    }

    /// Up to the first `length` bytes of `relative`, if the package holds it.
    pub fn read_prefix(&mut self, relative: &str, length: usize) -> Result<Option<Vec<u8>>> {
        let Some(&index) = self.files.get(relative) else {
            return Ok(None);
        };
        let entry = self
            .zip
            .by_index(index)
            .map_err(|error| PackageRejection::InvalidArchive(error.to_string()))?;
        let mut bytes = Vec::with_capacity(length);
        entry
            .take(length as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| PackageRejection::InvalidArchive(format!("{relative}: {error}")))?;
        Ok(Some(bytes))
    }

    /// The SHA-256 of `relative`, if the package holds it.
    pub fn sha256(&mut self, relative: &str) -> Result<Option<String>> {
        let Some(&index) = self.files.get(relative) else {
            return Ok(None);
        };
        let mut entry = self
            .zip
            .by_index(index)
            .map_err(|error| PackageRejection::InvalidArchive(error.to_string()))?;
        let mut hasher = Sha256::new();
        let mut limited = (&mut entry).take(MAX_ENTRY_BYTES + 1);
        let copied = std::io::copy(&mut limited, &mut HashWriter(&mut hasher))
            .map_err(|error| PackageRejection::InvalidArchive(format!("{relative}: {error}")))?;
        if copied > MAX_ENTRY_BYTES {
            return Err(
                PackageRejection::InvalidArchive(format!("{relative} is too large")).into(),
            );
        }
        Ok(Some(hex::encode(hasher.finalize())))
    }

    /// Writes the whole package into `directory` (which must be empty).
    pub fn unpack(&mut self, directory: &Path) -> Result<()> {
        fs::create_dir_all(directory)?;
        for relative in &self.directories {
            fs::create_dir_all(directory.join(relative))?;
        }
        let mut files: Vec<(String, usize)> = self
            .files
            .iter()
            .map(|(path, index)| (path.clone(), *index))
            .collect();
        files.sort();
        for (relative, index) in files {
            let path = directory.join(&relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut entry = self
                .zip
                .by_index(index)
                .map_err(|error| PackageRejection::InvalidArchive(error.to_string()))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            let mut limited = (&mut entry).take(MAX_ENTRY_BYTES + 1);
            let copied = std::io::copy(&mut limited, &mut output)?;
            if copied > MAX_ENTRY_BYTES {
                return Err(
                    PackageRejection::InvalidArchive(format!("{relative} is too large")).into(),
                );
            }
            output.flush()?;
        }
        Ok(())
    }
}

/// Writes into a SHA-256.
struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn invalid_archive(path: &Path, message: String) -> super::PluginError {
    PackageRejection::InvalidArchive(format!("{}: {message}", path.display())).into()
}

/// The entry's path, `/`-separated, once it is a plain relative path every
/// platform can create.
fn entry_path(raw: &str) -> std::result::Result<String, &'static str> {
    // Older Windows tools separate with backslashes.
    let normalized = raw.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("is not a plain path");
    }
    if trimmed.starts_with('/') {
        return Err("leaves the package");
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("leaves the package");
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("ends with a dot or a space");
        }
        if part
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') || ch.is_control())
        {
            return Err("holds a character file names cannot");
        }
        if is_windows_device_name(part.split('.').next().unwrap_or(part)) {
            return Err("is a Windows device name");
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

/// `""` when `plugin.toml` is at the archive root, else `"<directory>/"`
/// for the single top-level directory that holds it.
fn package_root(entries: &[(String, bool, usize)]) -> Option<String> {
    if entries
        .iter()
        .any(|(path, is_directory, _)| !is_directory && path == MANIFEST_FILE)
    {
        return Some(String::new());
    }
    let mut top_level: Vec<&str> = entries
        .iter()
        .filter_map(|(path, is_directory, _)| match path.split_once('/') {
            Some((directory, _)) => Some(directory),
            None if *is_directory => Some(path.as_str()),
            None => None,
        })
        .collect();
    top_level.sort_unstable();
    top_level.dedup();
    let [only] = top_level.as_slice() else {
        return None;
    };
    let manifest = format!("{only}/{MANIFEST_FILE}");
    entries
        .iter()
        .any(|(path, is_directory, _)| !is_directory && *path == manifest)
        .then(|| format!("{only}/"))
}

/// A relative library path as the archive names it.
pub fn archive_path(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Unpacks `archive` into `staging`; returns the package directory, which
/// is `staging` itself.
pub fn unpack(archive: &Path, staging: &Path) -> Result<PathBuf> {
    PackageArchive::open(archive)?.unpack(staging)?;
    Ok(staging.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).expect("archive"));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).expect("directory");
            } else {
                writer.start_file(*name, options).expect("entry");
                writer.write_all(content).expect("content");
            }
        }
        writer.finish().expect("finish");
    }

    #[test]
    fn entry_paths_must_be_portable() {
        assert_eq!(
            entry_path("demo/plugin.toml"),
            Ok("demo/plugin.toml".to_owned())
        );
        assert_eq!(entry_path("locales/"), Ok("locales".to_owned()));
        assert_eq!(
            entry_path("demo\\plugin.toml"),
            Ok("demo/plugin.toml".to_owned())
        );
        for bad in [
            "../plugin.toml",
            "/plugin.toml",
            "demo//plugin.toml",
            "demo/./plugin.toml",
            "locales/CON.toml",
            "aux",
            "notes.",
            "a:b",
            "",
        ] {
            assert!(entry_path(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn the_package_root_is_the_archive_root_or_its_single_directory() {
        let directory = tempfile::tempdir().expect("archives");
        let nested = directory.path().join("nested.lcpkg");
        write_archive(
            &nested,
            &[
                ("demo/", b""),
                ("demo/plugin.toml", b"id"),
                ("demo/locales/en.toml", b"x"),
                ("demo/README.md", b"# Demo"),
            ],
        );
        let mut archive = PackageArchive::open(&nested).expect("nested package");
        assert!(archive.contains(MANIFEST_FILE));
        assert_eq!(archive.files_in("locales"), vec!["locales/en.toml"]);
        assert_eq!(
            archive.read("README.md", 64).expect("readme"),
            Some(b"# Demo".to_vec())
        );
        assert!(archive.read("README.md", 2).is_err(), "size limit");

        let two = directory.path().join("two.lcpkg");
        write_archive(&two, &[("a/plugin.toml", b"id"), ("b/plugin.toml", b"id")]);
        assert!(PackageArchive::open(&two).is_err());

        let duplicate = directory.path().join("duplicate.lcpkg");
        write_archive(
            &duplicate,
            &[("plugin.toml", b"id"), ("PLUGIN.toml", b"id")],
        );
        assert!(PackageArchive::open(&duplicate).is_err());
    }

    #[test]
    fn archives_hash_and_unpack_what_they_hold() {
        let directory = tempfile::tempdir().expect("archives");
        let path = directory.path().join("demo.lcpkg");
        write_archive(
            &path,
            &[("plugin.toml", b"id"), ("bin/demo.dll", b"library")],
        );
        let mut archive = PackageArchive::open(&path).expect("archive");
        assert_eq!(
            archive.sha256("bin/demo.dll").expect("hash"),
            Some(hex::encode(Sha256::digest(b"library")))
        );
        assert_eq!(archive.sha256("missing.dll").expect("hash"), None);
        let target = directory.path().join("unpacked");
        archive.unpack(&target).expect("unpack");
        assert_eq!(
            fs::read(target.join("bin/demo.dll")).expect("library"),
            b"library"
        );
        assert_eq!(
            digest_file(&target.join("plugin.toml")).expect("digest"),
            FileDigest {
                sha256: hex::encode(Sha256::digest(b"id")),
                size: 2,
            }
        );
    }
}
