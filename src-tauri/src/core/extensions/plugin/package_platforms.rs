//! The platforms a package archive can run on: which of the platform
//! library files its manifest names it carries, and the CPU architecture
//! each one was built for (read from the PE, ELF or Mach-O header).
//! Platforms are named `<os>-<arch>` with Rust's names: `windows-x86_64`,
//! `linux-aarch64`, `macos-aarch64`.

use std::path::{Path, PathBuf};

use super::{
    package_archive::{archive_path, FileDigest, PackageArchive},
    package_inspect::invalid_package,
    Locales, PluginManifest, Result,
};

/// Header bytes read to tell an architecture.
const HEADER_BYTES: usize = 4096;

/// The platform this build runs on.
pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What an archive offers, for an index ([`describe_archive`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveDescription {
    pub digest: FileDigest,
    pub manifest: PluginManifest,
    pub locales: Locales,
    /// `<os>-<arch>` of every library the package carries, sorted.
    pub platforms: Vec<String>,
}

/// Reads `archive`'s manifest and locales (with the same checks as
/// discovery) and the platforms of its libraries. Blocking; runs nothing.
pub fn describe_archive(archive: &Path) -> Result<ArchiveDescription> {
    let facts = super::package_inspect::read_archive(archive)?;
    let digest = facts.digest.clone();
    let manifest = facts.definition.manifest.clone();
    let mut package = PackageArchive::open(archive)?;
    let mut platforms = Vec::new();
    for (os, file) in library_files(&manifest) {
        let Some(header) = package.read_prefix(&archive_path(&file), HEADER_BYTES)? else {
            continue;
        };
        for arch in architectures(&header) {
            platforms.push(format!("{os}-{arch}"));
        }
    }
    platforms.sort();
    platforms.dedup();
    if platforms.is_empty() {
        return Err(invalid_package(super::PluginError::Manifest(format!(
            "{} holds no library {} for any platform",
            archive.display(),
            manifest.library
        ))));
    }
    Ok(ArchiveDescription {
        digest,
        manifest,
        locales: facts.definition.locales.as_ref().clone(),
        platforms,
    })
}

/// The library file the manifest names on each OS: its own extension when
/// it has one, else the platform's prefix and suffix.
fn library_files(manifest: &PluginManifest) -> Vec<(&'static str, PathBuf)> {
    let relative = Path::new(&manifest.library);
    let extension = relative
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let named = |os: &'static str, prefix: &str, suffix: &str| {
        let file_name = relative
            .file_name()
            .map(|name| format!("{prefix}{}{suffix}", name.to_string_lossy()))
            .unwrap_or_default();
        (os, relative.with_file_name(file_name))
    };
    match extension.as_deref() {
        Some("dll") => vec![("windows", relative.to_owned())],
        Some("so") => vec![("linux", relative.to_owned())],
        Some("dylib") => vec![("macos", relative.to_owned())],
        _ => vec![
            named("windows", "", ".dll"),
            named("linux", "lib", ".so"),
            named("macos", "lib", ".dylib"),
        ],
    }
}

/// Architectures of a PE, ELF or Mach-O (thin or universal) header.
fn architectures(header: &[u8]) -> Vec<&'static str> {
    let u16_le = |offset: usize| {
        header
            .get(offset..offset + 2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    };
    let u32_at = |offset: usize, big_endian: bool| {
        header.get(offset..offset + 4).map(|bytes| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        })
    };
    // PE: "MZ", e_lfanew, "PE\0\0", Machine.
    if header.starts_with(b"MZ") {
        let machine = u32_at(0x3C, false)
            .map(|offset| offset as usize)
            .filter(|offset| header.get(*offset..*offset + 4) == Some(b"PE\0\0".as_slice()))
            .and_then(|offset| u16_le(offset + 4));
        return match machine {
            Some(0x8664) => vec!["x86_64"],
            Some(0xAA64) => vec!["aarch64"],
            Some(0x014C) => vec!["x86"],
            _ => Vec::new(),
        };
    }
    // ELF: e_machine at 18, in the byte order EI_DATA names.
    if header.starts_with(b"\x7fELF") {
        let big_endian = header.get(5) == Some(&2);
        let machine = header.get(18..20).map(|bytes| {
            if big_endian {
                u16::from_be_bytes([bytes[0], bytes[1]])
            } else {
                u16::from_le_bytes([bytes[0], bytes[1]])
            }
        });
        return match machine {
            Some(0x3E) => vec!["x86_64"],
            Some(0xB7) => vec!["aarch64"],
            Some(0x03) => vec!["x86"],
            _ => Vec::new(),
        };
    }
    let mach_cpu = |cpu: u32| match cpu {
        0x0100_0007 => Some("x86_64"),
        0x0100_000C => Some("aarch64"),
        _ => None,
    };
    match u32_at(0, false) {
        // 64-bit Mach-O, little endian: cputype follows the magic.
        Some(0xFEED_FACF) => u32_at(4, false).and_then(mach_cpu).into_iter().collect(),
        // Universal binary: big-endian count, then 20-byte architecture
        // records whose first field is the cputype.
        Some(0xBEBA_FECA) => {
            let count = u32_at(4, true).unwrap_or_default().min(16) as usize;
            (0..count)
                .filter_map(|index| u32_at(8 + index * 20, true).and_then(mach_cpu))
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pe(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0_u8; 0x100];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    #[test]
    fn headers_name_their_architecture() {
        assert_eq!(architectures(&pe(0x8664)), vec!["x86_64"]);
        assert_eq!(architectures(&pe(0xAA64)), vec!["aarch64"]);
        let mut elf = vec![0_u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[5] = 1;
        elf[18..20].copy_from_slice(&0xB7_u16.to_le_bytes());
        assert_eq!(architectures(&elf), vec!["aarch64"]);
        let mut macho = vec![0_u8; 32];
        macho[..4].copy_from_slice(&0xFEED_FACF_u32.to_le_bytes());
        macho[4..8].copy_from_slice(&0x0100_0007_u32.to_le_bytes());
        assert_eq!(architectures(&macho), vec!["x86_64"]);
        let mut universal = vec![0_u8; 48];
        universal[..4].copy_from_slice(&0xCAFE_BABE_u32.to_be_bytes());
        universal[4..8].copy_from_slice(&2_u32.to_be_bytes());
        universal[8..12].copy_from_slice(&0x0100_0007_u32.to_be_bytes());
        universal[28..32].copy_from_slice(&0x0100_000C_u32.to_be_bytes());
        assert_eq!(architectures(&universal), vec!["x86_64", "aarch64"]);
        assert!(architectures(b"not a library").is_empty());
    }

    #[test]
    fn library_names_follow_each_platform() {
        let manifest = PluginManifest::parse(
            "id = \"org.test.lib\"\nname = \"L\"\nversion = \"1.0.0\"\nprotocol = 2\nlibrary = \"bin/demo\"\n",
        )
        .expect("manifest");
        assert_eq!(
            library_files(&manifest),
            vec![
                ("windows", PathBuf::from("bin/demo.dll")),
                ("linux", PathBuf::from("bin/libdemo.so")),
                ("macos", PathBuf::from("bin/libdemo.dylib")),
            ]
        );
    }

    #[test]
    fn archives_describe_their_platforms() {
        let directory = tempfile::tempdir().expect("archives");
        let path = directory.path().join("demo.lcpkg");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).expect("archive"));
        let options = zip::write::SimpleFileOptions::default();
        use std::io::Write as _;
        for (name, content) in [
            (
                "plugin.toml",
                b"id = \"org.test.demo\"\nname = \"Demo\"\nversion = \"1.2.0\"\nprotocol = 2\nlibrary = \"demo\"\n".to_vec(),
            ),
            ("demo.dll", pe(0x8664)),
            ("locales/en.toml", b"\"plugin.name\" = \"Demo\"\n".to_vec()),
        ] {
            writer.start_file(name, options).expect("entry");
            writer.write_all(&content).expect("content");
        }
        writer.finish().expect("finish");
        let description = describe_archive(&path).expect("description");
        assert_eq!(description.platforms, vec!["windows-x86_64"]);
        assert_eq!(description.manifest.version, "1.2.0");
        assert_eq!(description.locales["en"]["plugin.name"], "Demo");
    }
}
