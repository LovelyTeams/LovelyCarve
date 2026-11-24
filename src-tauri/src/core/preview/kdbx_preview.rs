//! KeePass KDBX databases in the original-type view: the outer header without
//! a key, the decrypted database with one, and attachment bytes. Parsing and
//! decryption are the vendored `kdbx` reader; this module reads the evidence
//! and keyfile bytes and shapes the reports.

mod header;
mod report;

use std::fmt;
use std::io::Read;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::core::common::error::Failure;
use crate::core::common::ids::NodeId;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;

pub use header::{header_report, KdbxHeaderReport};
pub use report::{database_report, KdbxDatabaseReport, KdbxKeyUsed};

/// The largest database the view reads whole.
pub const KDBX_READ_CAP: usize = 256 * 1024 * 1024;
/// The header is read from this much of the file first.
const HEADER_PREFIX: usize = 1024 * 1024;
/// The largest keyfile read; KeePass hashes keyfiles of any size.
pub const KEYFILE_READ_CAP: usize = 64 * 1024 * 1024;
/// The largest attachment slice one request returns.
pub const ATTACHMENT_READ_CAP: usize = 1024 * 1024;

/// Where a keyfile is.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum KdbxKeyfile {
    /// A file on the host, picked in a dialog or named on the command line.
    Host { path: String },
    /// A file in the same evidence as the database.
    #[serde(rename_all = "camelCase")]
    Evidence { node_id: NodeId },
}

/// The credentials of the unlock form.
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdbxCredentials {
    /// The master password. Empty or absent tries the keyfile alone, then
    /// an empty password with it, as KeePass distinguishes the two.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub keyfile: Option<KdbxKeyfile>,
    /// A transformed key (the KDF output) in hex; replaces the password and
    /// keyfile and skips the key derivation.
    #[serde(default)]
    pub transformed_key: Option<String>,
}

impl fmt::Debug for KdbxCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KdbxCredentials")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("keyfile", &self.keyfile)
            .field(
                "transformed_key",
                &self.transformed_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// An attachment of an entry or of one of its history versions.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KdbxAttachmentRef {
    /// The entry UUID, 32 hex digits.
    pub entry: String,
    /// The history version (oldest first); `None` for the current entry.
    #[serde(default)]
    pub history: Option<usize>,
    /// The attachment's position in that version.
    pub index: usize,
}

/// The reader's failure as the application vocabulary.
pub fn failure(error: kdbx::Error) -> Failure {
    match error {
        kdbx::Error::NotKeePass => Failure::unsupported("不是 KeePass 数据库"),
        kdbx::Error::KeePass1 => Failure::unsupported("KeePass 1.x（KDB）数据库"),
        kdbx::Error::UnsupportedVersion { major, minor } => {
            Failure::unsupported(format!("不支持的 KDBX 版本 {major}.{minor}"))
        }
        kdbx::Error::Unsupported(what) => {
            Failure::unsupported(format!("不支持的 KDBX 特性：{what}"))
        }
        kdbx::Error::Truncated(what) => Failure::corrupt(format!("KDBX 数据截断：{what}")),
        kdbx::Error::Malformed(what) => Failure::corrupt(format!("KDBX 结构无效：{what}")),
        kdbx::Error::HeaderChecksum => Failure::corrupt("KDBX 头部 SHA-256 不符"),
        kdbx::Error::BlockChecksum { index } => {
            Failure::corrupt(format!("KDBX 数据块 {index} 校验失败"))
        }
        kdbx::Error::Xml(what) => Failure::corrupt(format!("KDBX XML 无效：{what}")),
        kdbx::Error::InvalidCredentials => Failure::invalid_input("密码、密钥文件或转换密钥不正确"),
        kdbx::Error::Keyfile(what) => Failure::invalid_input(format!("密钥文件无效：{what}")),
        kdbx::Error::Limit(what) => Failure::resource_limit(format!("超出 KDBX 读取上限：{what}")),
    }
}

/// Read a whole evidence file, refusing files over `cap` and short reads.
fn read_whole(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    cap: usize,
    what: &str,
) -> anyhow::Result<(Vec<u8>, u64)> {
    let mut session = fs_tree::FsSession::new(image);
    let stat = session.file_stat(node_id)?;
    if stat.size > cap as u64 {
        anyhow::bail!(Failure::resource_limit(format!(
            "{what}大于 {} MiB 读取上限",
            cap / (1024 * 1024)
        )));
    }
    let data = session.read_file_limited(node_id, cap)?;
    if data.len() as u64 != stat.size {
        anyhow::bail!(Failure::corrupt(format!(
            "{what}只读到 {} / {} 字节",
            data.len(),
            stat.size
        )));
    }
    Ok((data, stat.size))
}

/// The database header, read from the start of the file (the whole file
/// when a header is larger than the prefix).
pub fn read_header(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<KdbxHeaderReport> {
    let mut session = fs_tree::FsSession::new(image);
    let stat = session.file_stat(node_id)?;
    let prefix = session.read_file_limited(node_id, HEADER_PREFIX)?;
    let header = match kdbx::Header::parse(&prefix) {
        Err(kdbx::Error::Truncated(_)) if stat.size > prefix.len() as u64 => {
            let (data, _) = read_whole(image, node_id, KDBX_READ_CAP, "数据库")?;
            kdbx::Header::parse(&data).map_err(failure)?
        }
        parsed => parsed.map_err(failure)?,
    };
    Ok(header_report(&header, stat.size))
}

/// The keyfile's bytes from the host or the evidence.
fn read_keyfile(
    image: &Arc<EvidenceSession>,
    keyfile: Option<&KdbxKeyfile>,
) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    let Some(keyfile) = keyfile else {
        return Ok(None);
    };
    let data = match keyfile {
        KdbxKeyfile::Host { path } => {
            let file = std::fs::File::open(path).with_context(|| format!("打开密钥文件 {path}"))?;
            let size = file
                .metadata()
                .with_context(|| format!("读取密钥文件 {path}"))?
                .len();
            if size > KEYFILE_READ_CAP as u64 {
                anyhow::bail!(Failure::resource_limit(format!(
                    "密钥文件大于 {} MiB 读取上限",
                    KEYFILE_READ_CAP / (1024 * 1024)
                )));
            }
            let mut data = Vec::with_capacity(size as usize);
            file.take(KEYFILE_READ_CAP as u64 + 1)
                .read_to_end(&mut data)
                .with_context(|| format!("读取密钥文件 {path}"))?;
            data
        }
        KdbxKeyfile::Evidence { node_id } => {
            read_whole(image, node_id, KEYFILE_READ_CAP, "密钥文件")?.0
        }
    };
    Ok(Some(Zeroizing::new(data)))
}

fn parse_transformed(text: &str) -> anyhow::Result<[u8; 32]> {
    let invalid = || Failure::invalid_input("转换密钥应为 64 个十六进制字符");
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = hex::decode(&digits).map_err(|_| invalid())?;
    Ok(<[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| invalid())?)
}

/// Decrypt with the form's credentials. An empty password with a keyfile
/// tries the keyfile alone first, then an empty password with the keyfile.
pub fn open_database(
    data: &[u8],
    credentials: &KdbxCredentials,
    keyfile: Option<&[u8]>,
) -> anyhow::Result<(kdbx::Database, KdbxKeyUsed)> {
    if let Some(text) = credentials
        .transformed_key
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        let key = kdbx::Key::from_transformed(parse_transformed(text)?);
        let database = kdbx::Database::open(data, &key).map_err(failure)?;
        return Ok((
            database,
            KdbxKeyUsed {
                password: "none",
                keyfile: None,
                transformed_key: true,
            },
        ));
    }
    let password = credentials.password.as_deref().unwrap_or_default();
    let attempts: &[Option<&str>] = match (password.is_empty(), keyfile.is_some()) {
        (false, _) => &[Some(password)],
        (true, true) => &[None, Some("")],
        (true, false) => &[Some("")],
    };
    for attempt in attempts {
        let mut key = kdbx::Key::new();
        if let Some(password) = attempt {
            key = key.with_password(password);
        }
        let mut keyfile_kind = None;
        if let Some(contents) = keyfile {
            let parsed = kdbx::Keyfile::parse(contents).map_err(failure)?;
            keyfile_kind = Some(parsed.kind.name());
            key = key.with_parsed_keyfile(parsed);
        }
        match kdbx::Database::open(data, &key) {
            Ok(database) => {
                let used = KdbxKeyUsed {
                    password: match attempt {
                        None => "none",
                        Some("") => "empty",
                        Some(_) => "set",
                    },
                    keyfile: keyfile_kind,
                    transformed_key: false,
                };
                return Ok((database, used));
            }
            Err(kdbx::Error::InvalidCredentials) => continue,
            Err(error) => return Err(failure(error).into()),
        }
    }
    Err(failure(kdbx::Error::InvalidCredentials).into())
}

/// Read the database and keyfile and decrypt them.
pub fn open_evidence(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    credentials: &KdbxCredentials,
) -> anyhow::Result<(kdbx::Database, KdbxKeyUsed, u64)> {
    let (data, size) = read_whole(image, node_id, KDBX_READ_CAP, "数据库")?;
    let keyfile = read_keyfile(image, credentials.keyfile.as_ref())?;
    let (database, used) =
        open_database(&data, credentials, keyfile.as_deref().map(Vec::as_slice))?;
    Ok((database, used, size))
}

/// Up to `max_bytes` of an attachment's data.
pub fn attachment_bytes(
    database: &kdbx::Database,
    reference: &KdbxAttachmentRef,
    max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    let uuid = kdbx::Uuid::parse_hex(&reference.entry)
        .ok_or_else(|| Failure::invalid_input("条目 UUID 应为 32 个十六进制字符"))?;
    let entry = database
        .entry_by_uuid(&uuid)
        .ok_or_else(|| Failure::not_found(format!("没有 UUID 为 {uuid} 的条目")))?;
    let version = match reference.history {
        Some(index) => entry
            .history
            .get(index)
            .ok_or_else(|| Failure::not_found(format!("条目 {uuid} 没有第 {index} 个历史版本")))?,
        None => entry,
    };
    let attachment = version.attachments.get(reference.index).ok_or_else(|| {
        Failure::not_found(format!("条目 {uuid} 没有第 {} 个附件", reference.index))
    })?;
    let data = database.attachment_data(attachment).ok_or_else(|| {
        Failure::not_found(format!("附件 {} 的数据不在数据库中", attachment.name))
    })?;
    Ok(data[..data.len().min(max_bytes)].to_vec())
}

#[cfg(test)]
mod tests;
