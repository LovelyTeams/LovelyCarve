//! Xiaomi `.bak` logical-backup unpacking.
//!
//! Xiaomi app backups commonly wrap an Android Backup (`ANDROID BACKUP\n`)
//! stream with a small proprietary prefix. The reference JavaScript tool drops
//! everything before the `AN` marker, parses the Android Backup header, inflates
//! the payload when requested, and extracts the resulting tar. Folder evidence
//! uses the faster path here: the original `.bak` remains untouched, the payload
//! is materialised once as a temporary tar, and individual files are read lazily
//! from that tar index.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::ZlibDecoder;
use gzipf::GzipDecoder;
use memchr::memmem::Finder;

use crate::core::evidence::readers::tar_image::TarImage;

const ANDROID_BACKUP_MAGIC: &[u8] = b"ANDROID BACKUP\n";
const BACKUP_FILE_V1: u8 = 1;
const BACKUP_FILE_V5: u8 = 5;
const ENCRYPTION_NONE: &str = "none";

#[cfg(test)]
#[path = "mi_bak/gzip_tests.rs"]
mod gzip_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiBakUnpackStats {
    pub files: usize,
    pub directories: usize,
}

/// Unpack a Xiaomi `.bak` file into `output_dir`.
///
/// The output directory is created when needed. Password-protected Android
/// Backup payloads are deliberately rejected: folder loading has no password
/// prompt today, so silently guessing would make the evidence view misleading.
pub fn unpack_bak_file(input_path: &Path, output_dir: &Path) -> Result<MiBakUnpackStats> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create bak output directory {}", output_dir.display()))?;

    let mut file = File::open(input_path)
        .with_context(|| format!("open bak file {}", input_path.display()))?;
    let backup_offset = find_android_backup_offset(&mut file)?
        .ok_or_else(|| anyhow!("ANDROID BACKUP marker not found"))?;
    file.seek(SeekFrom::Start(backup_offset))
        .with_context(|| format!("seek to Android Backup payload in {}", input_path.display()))?;

    let mut reader = BufReader::new(file);
    let magic = read_header_line(&mut reader).context("read Android Backup magic")?;
    if magic.as_bytes() != b"ANDROID BACKUP" {
        bail!("invalid Android Backup magic: {magic}");
    }

    let version_line = read_header_line(&mut reader).context("read Android Backup version")?;
    let version = version_line
        .parse::<u8>()
        .with_context(|| format!("invalid Android Backup version: {version_line}"))?;
    if !(BACKUP_FILE_V1..=BACKUP_FILE_V5).contains(&version) {
        bail!("unsupported Android Backup version: {version}");
    }

    let compressed_line =
        read_header_line(&mut reader).context("read Android Backup compression flag")?;
    let compressed = compressed_line.trim() == "1";
    let encryption = read_header_line(&mut reader).context("read Android Backup encryption")?;
    if encryption != ENCRYPTION_NONE {
        bail!("encrypted Android Backup payload is not supported during folder auto-unpack");
    }

    if compressed {
        let is_gzip = reader
            .fill_buf()
            .map(gzipf::check_signature)
            .context("probe Android Backup compression")?;
        if is_gzip {
            // tar::Archive stops at its end marker, before a GZIP trailer or
            // later members. Validate the entire stream in a temporary tar
            // before creating extracted files; this also preserves empty tar
            // support in the legacy unpack path.
            let mut decoder = GzipDecoder::new(reader);
            let mut tar = tempfile::tempfile().context("create temporary gzip backup tar")?;
            io::copy(&mut decoder, &mut tar)
                .context("decompress and validate gzip Android Backup")?;
            tar.rewind().context("rewind validated backup tar")?;
            extract_tar(tar, output_dir)
        } else {
            extract_tar(ZlibDecoder::new(reader), output_dir)
        }
    } else {
        extract_tar(reader, output_dir)
    }
}

/// Open a Xiaomi `.bak` as an indexed tar image without extracting every file.
///
/// This is the fast path used by folder evidence: the Android Backup payload is
/// decompressed once into a temporary tar, then tar headers are indexed for
/// lazy reads.
pub fn open_bak_archive(input_path: &Path, label: String) -> Result<TarImage> {
    let mut file = File::open(input_path)
        .with_context(|| format!("open bak file {}", input_path.display()))?;
    let backup_offset = find_android_backup_offset(&mut file)?
        .ok_or_else(|| anyhow!("ANDROID BACKUP marker not found"))?;
    file.seek(SeekFrom::Start(backup_offset))
        .with_context(|| format!("seek to Android Backup payload in {}", input_path.display()))?;

    let mut reader = BufReader::new(file);
    read_backup_header(&mut reader)?;

    let compressed = read_header_line(&mut reader)
        .context("read Android Backup compression flag")?
        .trim()
        == "1";
    let encryption = read_header_line(&mut reader).context("read Android Backup encryption")?;
    if encryption != ENCRYPTION_NONE {
        bail!("encrypted Android Backup payload is not supported during folder auto-unpack");
    }

    if compressed {
        let is_gzip = reader
            .fill_buf()
            .map(gzipf::check_signature)
            .context("probe Android Backup compression")?;
        if is_gzip {
            let mut decoder = GzipDecoder::new(reader);
            TarImage::open_from_tar_reader(input_path.to_path_buf(), Some(label), &mut decoder)
        } else {
            let mut decoder = ZlibDecoder::new(reader);
            TarImage::open_from_tar_reader(input_path.to_path_buf(), Some(label), &mut decoder)
        }
    } else {
        TarImage::open_from_tar_reader(input_path.to_path_buf(), Some(label), &mut reader)
    }
}

fn find_android_backup_offset(file: &mut File) -> io::Result<Option<u64>> {
    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0u64;
    let finder = Finder::new(ANDROID_BACKUP_MAGIC);
    let overlap = ANDROID_BACKUP_MAGIC.len().saturating_sub(1);
    let mut carry_len = 0usize;
    let mut buffer = vec![0u8; 1024 * 1024 + overlap];
    loop {
        let read = file.read(&mut buffer[carry_len..])?;
        if read == 0 {
            return Ok(None);
        }

        let filled = carry_len + read;
        if let Some(pos) = finder.find(&buffer[..filled]) {
            return Ok(Some(offset.saturating_sub(carry_len as u64) + pos as u64));
        }

        carry_len = overlap.min(filled);
        buffer.copy_within(filled - carry_len..filled, 0);
        offset += read as u64;
    }
}

fn read_header_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = Vec::new();
    let read = reader.read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "missing Android Backup header line",
        ));
    }
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

fn read_backup_header(reader: &mut impl BufRead) -> Result<u8> {
    let magic = read_header_line(reader).context("read Android Backup magic")?;
    if magic.as_bytes() != b"ANDROID BACKUP" {
        bail!("invalid Android Backup magic: {magic}");
    }

    let version_line = read_header_line(reader).context("read Android Backup version")?;
    let version = version_line
        .parse::<u8>()
        .with_context(|| format!("invalid Android Backup version: {version_line}"))?;
    if !(BACKUP_FILE_V1..=BACKUP_FILE_V5).contains(&version) {
        bail!("unsupported Android Backup version: {version}");
    }
    Ok(version)
}

fn extract_tar(reader: impl Read, output_dir: &Path) -> Result<MiBakUnpackStats> {
    let mut archive = tar::Archive::new(reader);
    let mut stats = MiBakUnpackStats {
        files: 0,
        directories: 0,
    };

    for entry in archive
        .entries()
        .context("read Android Backup tar entries")?
    {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!("mi-bak: skipping unreadable tar entry: {err:#}");
                continue;
            }
        };
        let header = entry.header().clone();
        let raw_path = match entry.path() {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(err) => {
                tracing::warn!("mi-bak: skipping entry with undecodable path: {err:#}");
                continue;
            }
        };
        let Some(destination) = safe_output_path(output_dir, &raw_path) else {
            tracing::warn!("mi-bak: skipping unsafe tar path {raw_path:?}");
            continue;
        };

        if header.entry_type().is_dir() {
            fs::create_dir_all(&destination)
                .with_context(|| format!("create {}", destination.display()))?;
            stats.directories += 1;
            continue;
        }

        if header.entry_type().is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            let mut out = File::create(&destination)
                .with_context(|| format!("create {}", destination.display()))?;
            io::copy(&mut entry, &mut out)
                .with_context(|| format!("extract {}", destination.display()))?;
            set_permissions(&destination, header.mode().unwrap_or(0));
            stats.files += 1;
        }
    }

    Ok(stats)
}

fn safe_output_path(output_dir: &Path, raw_path: &str) -> Option<PathBuf> {
    let mut path = PathBuf::from(output_dir);
    let trimmed = raw_path.trim_start_matches("./").trim_start_matches('/');
    let mut has_component = false;
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        has_component = true;
        path.push(sanitise_component(segment));
    }
    has_component.then_some(path)
}

fn sanitise_component(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for ch in component.chars() {
        if matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "_".to_string()
    } else {
        out
    }
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if mode != 0 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn tar_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "apps/com.example/a:b.txt", &b"hello"[..])
                .expect("append file");
            builder.finish().expect("finish tar");
        }
        bytes
    }

    fn build_bak(compressed: bool) -> tempfile::NamedTempFile {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"ANDROID BACKUP\n5\n");
        payload.extend_from_slice(if compressed { b"1\n" } else { b"0\n" });
        payload.extend_from_slice(b"none\n");
        if compressed {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&tar_bytes()).expect("write zlib");
            payload.extend_from_slice(&encoder.finish().expect("finish zlib"));
        } else {
            payload.extend_from_slice(&tar_bytes());
        }

        let mut bak = tempfile::NamedTempFile::new().expect("temp bak");
        bak.write_all(b"mi-prefix").expect("write prefix");
        bak.write_all(&payload).expect("write payload");
        bak.flush().expect("flush");
        bak
    }

    #[test]
    fn unpacks_prefixed_zlib_android_backup() {
        let bak = build_bak(true);
        let out = tempfile::tempdir().expect("temp output");

        let stats = unpack_bak_file(bak.path(), out.path()).expect("unpack bak");

        assert_eq!(stats.files, 1);
        assert_eq!(
            fs::read(out.path().join("apps/com.example/a_b.txt")).expect("read extracted"),
            b"hello"
        );
    }

    #[test]
    fn rejects_encrypted_backups_without_password() {
        let mut bak = tempfile::NamedTempFile::new().expect("temp bak");
        bak.write_all(b"ANDROID BACKUP\n5\n0\nAES-256\n")
            .expect("write encrypted header");
        bak.flush().expect("flush");
        let out = tempfile::tempdir().expect("temp output");

        let err = unpack_bak_file(bak.path(), out.path()).expect_err("encrypted backup rejected");

        assert!(err.to_string().contains("encrypted"));
    }

    #[test]
    fn finds_backup_magic_across_scan_buffer_boundary() {
        const PREFIX_LEN: usize = 1024 * 1024 - 5;
        let mut bak = tempfile::NamedTempFile::new().expect("temp bak");
        bak.write_all(&vec![b'x'; PREFIX_LEN])
            .expect("write prefix");
        bak.write_all(ANDROID_BACKUP_MAGIC).expect("write magic");
        bak.flush().expect("flush");

        assert_eq!(
            find_android_backup_offset(bak.as_file_mut()).expect("scan backup"),
            Some(PREFIX_LEN as u64)
        );
    }

    #[test]
    fn unpacks_real_bak_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_MI_BAK_TEST") else {
            return;
        };
        let out = tempfile::tempdir().expect("temp output");

        let stats = unpack_bak_file(Path::new(&path), out.path()).expect("real bak should unpack");

        assert!(stats.files > 0 || stats.directories > 0);
    }
}
