//! 插件目录指纹:插件身份的信息展示(设置页、`PluginDefinition`)。
//! 不再参与信任判定——LovelyCarve 对发现到的插件一律信任可运行,
//! 指纹仅用于标识插件版本/内容变化。

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::{PluginError, Result};

const MAX_PLUGIN_FILES: usize = 10_000;
const MAX_PLUGIN_BYTES: u64 = 512 * 1024 * 1024;

pub(super) fn fingerprint_directory(directory: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(directory, directory, &mut files)?;
    files.sort();
    if files.len() > MAX_PLUGIN_FILES {
        return Err(PluginError::ResourceLimit(format!(
            "plugin directory has more than {MAX_PLUGIN_FILES} files"
        )));
    }

    let mut total_bytes = 0_u64;
    let mut hasher = Sha256::new();
    for path in files {
        let metadata = fs::metadata(&path)?;
        total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
            PluginError::ResourceLimit("plugin directory byte count overflowed".to_owned())
        })?;
        if total_bytes > MAX_PLUGIN_BYTES {
            return Err(PluginError::ResourceLimit(format!(
                "plugin directory exceeds {} MiB",
                MAX_PLUGIN_BYTES / (1024 * 1024)
            )));
        }
        let relative = path.strip_prefix(directory).map_err(|error| {
            PluginError::Manifest(format!("plugin path escaped its directory: {error}"))
        })?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update(metadata.len().to_be_bytes());
        let mut file = fs::File::open(&path)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            hasher.update(&buffer[..length]);
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(base: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(PluginError::Manifest(format!(
                "plugin directory {} contains symbolic link {}",
                base.display(),
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(base, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
            if files.len() > MAX_PLUGIN_FILES {
                return Err(PluginError::ResourceLimit(format!(
                    "plugin directory has more than {MAX_PLUGIN_FILES} files"
                )));
            }
        } else {
            return Err(PluginError::Manifest(format!(
                "plugin directory contains unsupported entry {}",
                path.display()
            )));
        }
    }
    Ok(())
}
