//! detect implementation.
use super::*;

pub(super) fn should_open_as_ewf(path: &Path) -> anyhow::Result<bool> {
    let by_extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(is_ewf_extension)
        .unwrap_or(false);

    let mut file =
        File::open(path).with_context(|| format!("failed to open image: {}", path.display()))?;
    let mut sig = [0u8; 8];
    let n = file.read(&mut sig)?;
    let by_signature = n == sig.len()
        && (sig == ewf::EVF_SIGNATURE
            || sig == ewf::sections::LVF_SIGNATURE
            || sig == EVF2_SIGNATURE
            || sig == LEF2_SIGNATURE);

    Ok(by_extension || by_signature)
}

pub(super) fn should_open_as_pmfx(path: &Path) -> anyhow::Result<bool> {
    let by_extension = lower_extension(path).as_deref() == Some("pmfx");
    Ok(by_extension || pmfx::has_pmfx_signature(path)?)
}

pub(super) fn has_qcow2_signature(path: &Path) -> anyhow::Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open image: {}", path.display()))?;
    let mut signature = [0u8; 4];
    Ok(file.read(&mut signature)? == signature.len() && signature == [b'Q', b'F', b'I', 0xfb])
}

pub(super) fn has_vdi_signature(path: &Path) -> anyhow::Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open image: {}", path.display()))?;
    let mut preheader = [0_u8; 68];
    Ok(file.read(&mut preheader)? == preheader.len()
        && preheader[64..68] == [0x7f, 0x10, 0xda, 0xbe])
}

pub(super) const VMDK_DESCRIPTOR_SCAN_LIMIT: u64 = 1024 * 1024;

pub(super) fn companion_descriptor_for_flat_extent(path: &Path) -> Option<PathBuf> {
    if !is_flat_vmdk_extent_path(path) {
        return None;
    }

    let file_name = path.file_name().and_then(|name| name.to_str())?;
    let parent = path.parent()?;

    let lower_name = file_name.to_ascii_lowercase();
    if let Some(stem) = lower_name.strip_suffix("-flat.vmdk") {
        let candidate_name = format!("{}.vmdk", &file_name[..stem.len()]);
        let candidate = parent.join(candidate_name);
        if descriptor_references_extent(&candidate, path) {
            return Some(candidate);
        }
    }

    let Ok(entries) = std::fs::read_dir(parent) else {
        return None;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let candidate = entry.path();
        if candidate == path || lower_extension(&candidate).as_deref() != Some("vmdk") {
            continue;
        }
        if descriptor_references_extent(&candidate, path) {
            return Some(candidate);
        }
    }

    None
}

pub(super) fn descriptor_references_extent(descriptor: &Path, extent: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(descriptor) else {
        return false;
    };
    if !meta.is_file() || meta.len() > VMDK_DESCRIPTOR_SCAN_LIMIT {
        return false;
    }

    for resolved in vmdk::VmdkFileReader::extent_dependencies(descriptor).unwrap_or_default() {
        if path_eq_case_insensitive(&resolved, extent) {
            return true;
        }
    }

    false
}

pub(super) fn is_flat_vmdk_extent_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase().ends_with("-flat.vmdk"))
        .unwrap_or(false)
}

pub(super) fn path_eq_case_insensitive(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

pub(super) fn has_vhdx_signature(path: &Path) -> anyhow::Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(false),
    };
    let mut sig = [0u8; 8];
    let n = file.read(&mut sig)?;
    Ok(n == sig.len() && sig == *b"vhdxfile")
}

/// Detects DMG / UDIF images by looking for the `koly` magic 512 bytes
/// before EOF. Treats any I/O error as "not a DMG" so we don't surface
/// confusing errors on regular files that happen to be smaller than 512
/// bytes.
pub(super) fn is_dmg_path(path: &Path) -> anyhow::Result<bool> {
    use std::io::{Seek, SeekFrom};
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(false),
    };
    let len = match file.seek(SeekFrom::End(0)) {
        Ok(len) => len,
        Err(_) => return Ok(false),
    };
    if len < 512 {
        return Ok(false);
    }
    if file.seek(SeekFrom::Start(len - 512)).is_err() {
        return Ok(false);
    }
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return Ok(false);
    }
    Ok(&magic == b"koly")
}

/// Open a DMG and decompress its main HFS+ / HFSX / APFS partition into a
/// tempfile. The tempfile is mmap'd and held alive inside the returned
/// `ImageSource::Dmg` variant; downstream code (partition detection,
/// volume adapters, hex view) treats the extracted bytes as a plain RAW
/// image.
///
/// Decompression runs sequentially — `udif`'s `parallel` feature is left
/// off in the vendored Cargo.toml because we already use rayon elsewhere
/// for carving and an extra worker pool here would only oversubscribe.
pub(super) fn open_dmg(path: &Path) -> anyhow::Result<ImageSource> {
    let mut archive = udif::DmgArchive::open(path)
        .with_context(|| format!("failed to open DMG: {}", path.display()))?;

    let partition_id = archive.main_partition_id().with_context(|| {
        format!(
            "DMG has no main HFS+/HFSX/APFS partition: {}",
            path.display()
        )
    })?;

    let mut tmp =
        tempfile::tempfile().with_context(|| "failed to create tempfile for DMG extraction")?;

    let extracted = archive
        .extract_partition_to(partition_id, &mut tmp)
        .with_context(|| {
            format!(
                "failed to decompress DMG main partition: {}",
                path.display()
            )
        })?;
    tmp.flush()?;

    let mmap = unsafe { Mmap::map(&tmp)? };
    let mmap_len = mmap.len() as u64;
    if mmap_len == 0 {
        anyhow::bail!(
            "DMG main partition decompressed to 0 bytes (extracted={extracted}): {}",
            path.display()
        );
    }
    Ok(ImageSource::Dmg {
        mmap,
        size_bytes: mmap_len,
        _tempfile: tmp,
    })
}

pub(super) fn is_ewf_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "e01" | "l01" | "s01" | "ex01" | "lx01"
    )
}

pub(super) fn lower_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

/// Whether `path` is the second or a later segment of an AD1 image: an
/// `.adN` extension (N > 1) on a file carrying the AD1 segment signature.
pub(super) fn is_later_ad1_segment(path: &Path) -> anyhow::Result<bool> {
    let Some(ext) = lower_extension(path) else {
        return Ok(false);
    };
    let Some(number) = ext.strip_prefix("ad") else {
        return Ok(false);
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) || number == "1" {
        return Ok(false);
    }
    let mut signature = [0u8; 15];
    match File::open(path)?.read_exact(&mut signature) {
        Ok(()) => Ok(&signature == b"ADSEGMENTEDFILE"),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn is_tar_gz_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".tar.gz") || name.ends_with(".tgz")
}

pub(super) fn has_vhd_signature(path: &Path) -> anyhow::Result<bool> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 512 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-512))?;
    let mut cookie = [0; 8];
    file.read_exact(&mut cookie)?;
    Ok(&cookie == b"conectix")
}

pub(super) const EVF2_SIGNATURE: [u8; 8] = [0x45, 0x56, 0x46, 0x32, 0x0d, 0x0a, 0x81, 0x00];
pub(super) const LEF2_SIGNATURE: [u8; 8] = [0x4c, 0x45, 0x46, 0x32, 0x0d, 0x0a, 0x81, 0x00];
