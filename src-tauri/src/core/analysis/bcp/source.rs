//! Read the root index and payload members from a folder or its BCP ZIP.

use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::core::common::error::Failure;
use crate::core::evidence::{image::EvidenceCache, readers::zip_image::ZipImage};

const INDEX: &str = "GAB_ZIP_INDEX.xml";
const MAX_INDEX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub(super) enum BcpSource {
    Folder(PathBuf),
    Zip(Arc<ZipImage>),
}

fn is_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

pub(super) fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.contains(['/', '\\', ':', '\0'])
            && Path::new(name).file_name().is_some()
            && is_extension(Path::new(name), "bcp"),
        "BCP 索引包含不安全的文件名：{name}"
    );
    Ok(())
}

impl BcpSource {
    pub(super) fn open(
        path: &Path,
        zip: Option<Arc<ZipImage>>,
        token: &CancellationToken,
    ) -> Result<(Self, Vec<String>)> {
        if let Some(zip) = zip {
            return Ok((Self::Zip(zip), Vec::new()));
        }
        let folder = Self::Folder(path.to_owned());
        let mut loose_files = Vec::new();
        let mut archives = Vec::new();
        for entry in fs::read_dir(path).with_context(|| format!("读取 {} 失败", path.display()))?
        {
            anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if is_extension(&entry.path(), "bcp") {
                loose_files.push(
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("BCP 文件名不是 Unicode"))?,
                );
            } else if is_extension(&entry.path(), "zip") {
                archives.push(entry.path());
            }
        }
        loose_files.sort_unstable();
        if !loose_files.is_empty() || path.join(INDEX).is_file() {
            return Ok((folder, loose_files));
        }
        match archives.as_slice() {
            [archive] => {
                let zip = ZipImage::open_cancellable(
                    archive,
                    EvidenceCache::Disabled,
                    &|_, _, _| {},
                    token,
                )?;
                Ok((Self::Zip(Arc::new(zip)), Vec::new()))
            }
            [] => Ok((folder, Vec::new())),
            _ => anyhow::bail!("目录包含多个 ZIP，请直接打开要读取的 BCP ZIP"),
        }
    }

    pub(super) fn path(&self) -> &Path {
        match self {
            Self::Folder(path) => path,
            Self::Zip(zip) => zip.path(),
        }
    }

    pub(super) fn index(&self) -> Result<Option<String>> {
        let read_index = |reader: &mut dyn Read, size| {
            anyhow::ensure!(size <= MAX_INDEX_BYTES, "BCP 索引超过 32 MiB");
            let mut xml = String::new();
            reader.take(MAX_INDEX_BYTES + 1).read_to_string(&mut xml)?;
            anyhow::ensure!(xml.len() as u64 == size, "BCP 索引读取不完整");
            Ok(Some(xml))
        };
        match self {
            Self::Zip(zip) if zip.lookup(INDEX).is_some() => {
                zip.with_member_reader(INDEX, read_index)
            }
            Self::Folder(root) if root.join(INDEX).is_file() => {
                let (mut file, size) = open_file(root, INDEX)?;
                read_index(&mut file, size)
            }
            _ => Ok(None),
        }
    }

    pub(super) fn with_rows<T>(
        &self,
        name: &str,
        offset: u64,
        read: impl FnOnce(&mut dyn BufRead, u64) -> Result<T>,
    ) -> Result<T> {
        validate_name(name)?;
        match self {
            Self::Folder(root) => {
                let (mut file, size) = open_file(root, name)?;
                anyhow::ensure!(offset <= size, Failure::invalid_input("BCP 分页偏移无效"));
                file.seek(SeekFrom::Start(offset))?;
                read(&mut BufReader::new(file), size)
            }
            Self::Zip(zip) => zip.with_member_reader(name, |member, size| {
                anyhow::ensure!(offset <= size, Failure::invalid_input("BCP 分页偏移无效"));
                let skipped = io::copy(&mut member.take(offset), &mut io::sink())?;
                anyhow::ensure!(skipped == offset, "BCP 分片读取不完整：{name}");
                read(&mut BufReader::new(member), size)
            }),
        }
    }

    pub(super) fn file_size(&self, name: &str) -> Result<u64> {
        validate_name(name)?;
        match self {
            Self::Folder(root) => Ok(open_file(root, name)?.1),
            Self::Zip(zip) => zip.with_member_reader(name, |_member, size| Ok(size)),
        }
    }
}

fn open_file(root: &Path, name: &str) -> Result<(File, u64)> {
    let path = root.join(name);
    let metadata =
        fs::symlink_metadata(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "BCP 索引引用了非常规文件：{}",
        path.display()
    );
    let file = File::open(&path).with_context(|| format!("打开 {} 失败", path.display()))?;
    let size = file.metadata()?.len();
    Ok((file, size))
}
