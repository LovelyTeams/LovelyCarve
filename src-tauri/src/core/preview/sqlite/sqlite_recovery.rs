//! Deleted-record recovery for a SQLite database inside an image — the
//! 「已删除记录」 view of the SQLite browser.
//!
//! The database (and its `-wal`, or failing that its `-journal`) is parsed by
//! the vendored `sqlite-core` reader, not by SQLite: freelist pages, in-page
//! free space, freeblock remnants, overflow chains and uncheckpointed WAL
//! frames are carved, each record is attributed to a table where the page
//! linkage or the row shape allows, dropped `sqlite_master` rows are
//! recovered, and a rollback journal is diffed against the current rows. Live
//! rows are never reported. An encrypted database is first decrypted page by
//! page with the key the browser unlocked it with (a logical export would
//! drop the free pages that hold the evidence); its sidecars are skipped.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use serde::Serialize;
use sqlite_core::recovery::{self, Attribution, CarvedRecord, RecoverySource};
use sqlite_core::sqlcipher::{self, CipherParams, DecryptError, SqlCipherKey, SqlCipherVersion};
use sqlite_core::{Database, Value};

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::preview::sqlite::sqlite_preview::{self, SqliteCell, SqliteKey};

/// Records returned at most; the rest are counted in `omitted`.
const MAX_RECORDS: usize = 20_000;

/// Everything recovered from one database.
#[derive(Debug, Serialize)]
pub struct SqliteRecovery {
    pub page_size: u32,
    pub page_count: u32,
    pub freelist_pages: u32,
    /// The SQLCipher profile that decrypted the database, if it was encrypted.
    pub cipher: Option<String>,
    /// Sidecar files that took part (`-wal` or `-journal`).
    pub sidecars: Vec<String>,
    /// Caveats the examiner should see next to the results.
    pub notes: Vec<String>,
    pub records: Vec<RecoveredRecord>,
    /// Records beyond [`MAX_RECORDS`] that were not returned.
    pub omitted: usize,
    pub schemas: Vec<RecoveredSchema>,
}

/// How a record was tied to a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableLink {
    /// The page still belongs to the table's b-tree.
    Known,
    /// Freed page; the row shape matches exactly one live table.
    Inferred,
    /// Freed page; the row shape matches several live tables.
    Ambiguous,
    /// Dropped-table residue, or a shape matching no live table.
    Unattributed,
}

/// Where a record was recovered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordOrigin {
    /// A page freed onto the freelist.
    FreelistPage,
    /// Free space of a page still in use.
    InPage,
    /// A freed cell rebuilt after its header was overwritten (rowid lost).
    Freeblock,
    /// The pre-update version of a row that is still live.
    PriorVersion,
    /// A freed page of a table that no longer exists.
    DroppedTable,
    /// A page image in the uncheckpointed `-wal`.
    WalFrame,
    /// A WAL commit snapshot.
    WalCommit,
    /// A row the last transaction deleted, from the rollback journal.
    JournalDeleted,
    /// The pre-update version of a row the last transaction changed.
    JournalPrior,
}

/// One recovered row.
#[derive(Debug, Serialize)]
pub struct RecoveredRecord {
    /// Owning or best-matching table; `None` when unattributed.
    pub table: Option<String>,
    pub link: TableLink,
    pub origin: RecordOrigin,
    /// 1-based page the record was carved from (journal: the restored page).
    pub page: Option<u32>,
    pub offset: Option<usize>,
    /// `None` when freeblock conversion destroyed it.
    pub rowid: Option<i64>,
    /// Heuristic confidence of a carved record, `(0, 1]`.
    pub confidence: Option<f32>,
    /// Source frame of a WAL record.
    pub wal_frame: Option<usize>,
    /// Overflow pages a record was reassembled across.
    pub overflow_pages: Vec<u32>,
    pub values: Vec<SqliteCell>,
    /// The row's current values, for a journal prior version.
    pub current_values: Option<Vec<SqliteCell>>,
}

/// A dropped or replaced `sqlite_master` row recovered from free space.
#[derive(Debug, Serialize)]
pub struct RecoveredSchema {
    pub object_type: String,
    pub name: String,
    pub table_name: String,
    pub root_page: Option<i64>,
    pub sql: String,
}

fn cell(value: &Value) -> SqliteCell {
    match value {
        Value::Null => SqliteCell::Null,
        Value::Integer(number) => SqliteCell::Integer(*number),
        Value::Real(number) => SqliteCell::Real(*number),
        Value::Text(text) => SqliteCell::Text(sqlite_preview::truncate_text(text)),
        Value::Blob(bytes) => SqliteCell::Blob { size: bytes.len() },
    }
}

fn cells(values: &[Value]) -> Vec<SqliteCell> {
    values.iter().map(cell).collect()
}

fn origin(source: RecoverySource) -> RecordOrigin {
    match source {
        RecoverySource::FreelistPage => RecordOrigin::FreelistPage,
        RecoverySource::InPageFreeBlock => RecordOrigin::InPage,
        RecoverySource::DroppedTable => RecordOrigin::DroppedTable,
        RecoverySource::PriorVersion => RecordOrigin::PriorVersion,
        RecoverySource::FreeblockReconstructed => RecordOrigin::Freeblock,
        RecoverySource::WalFrame => RecordOrigin::WalFrame,
        RecoverySource::CommitSnapshot => RecordOrigin::WalCommit,
        RecoverySource::RollbackJournal(_) => RecordOrigin::JournalDeleted,
    }
}

fn link(attribution: Attribution) -> (Option<String>, TableLink) {
    match attribution {
        Attribution::Known(table) => (Some(table), TableLink::Known),
        Attribution::Inferred {
            guess,
            ambiguous: false,
        } => (Some(guess), TableLink::Inferred),
        Attribution::Inferred {
            guess,
            ambiguous: true,
        } => (Some(guess), TableLink::Ambiguous),
        Attribution::Unattributed => (None, TableLink::Unattributed),
    }
}

fn carved_record(record: CarvedRecord, attribution: Attribution) -> RecoveredRecord {
    let (table, link) = link(attribution);
    RecoveredRecord {
        table,
        link,
        origin: origin(record.source),
        page: Some(record.page),
        offset: Some(record.offset),
        // Freeblock conversion overwrites the rowid; the reader reports 0.
        rowid: (record.rowid != 0).then_some(record.rowid),
        confidence: Some(record.confidence),
        wal_frame: record.wal.map(|wal| wal.frame_index),
        overflow_pages: record
            .overflow
            .map(|overflow| overflow.chain)
            .unwrap_or_default(),
        values: cells(&record.values),
        current_values: None,
    }
}

/// The SQLCipher key material of `key`, or `None` when it carries none.
fn cipher_key(key: &SqliteKey) -> anyhow::Result<Option<SqlCipherKey>> {
    if let Some(raw) = key
        .raw_hex_key
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        let cleaned: String = raw
            .strip_prefix("0x")
            .unwrap_or(raw)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if cleaned.len() == 96 {
            bail!("暂不支持用带 salt 的原始密钥恢复已删除记录");
        }
        let bytes = hex::decode(&cleaned).context("原始密钥不是有效的十六进制字符串")?;
        let raw: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow!("原始密钥必须是 32 字节（64 个十六进制字符）"))?;
        return Ok(Some(SqlCipherKey::RawKey(raw)));
    }
    Ok(key
        .passphrase
        .as_deref()
        .filter(|passphrase| !passphrase.is_empty())
        .map(|passphrase| SqlCipherKey::Passphrase(passphrase.as_bytes().to_vec())))
}

fn cipher_label(version: SqlCipherVersion, page_size: u32) -> String {
    let version = match version {
        SqlCipherVersion::V4 => "SQLCipher 4",
        SqlCipherVersion::V3 => "SQLCipher 3",
        SqlCipherVersion::V2 => "SQLCipher 2",
        SqlCipherVersion::V1 => "SQLCipher 1",
        SqlCipherVersion::Custom => "SQLCipher（自定义参数）",
    };
    format!("{version} · 页大小 {page_size}")
}

/// Decrypt every page with the browser's key: explicit parameters when the
/// key names any, otherwise the version defaults are detected.
fn decrypt(bytes: &[u8], key: &SqliteKey) -> anyhow::Result<sqlcipher::Decrypted> {
    let material =
        cipher_key(key)?.ok_or_else(|| anyhow!("SQLITE_LOCKED: 数据库已加密，请先输入密钥解密"))?;
    let explicit = key.compatibility.is_some() || key.page_size.is_some() || key.kdf_iter.is_some();
    let result = if explicit {
        let mut params = CipherParams::compatibility(key.compatibility.unwrap_or(4))
            .context("不支持的 SQLCipher 兼容版本")?;
        if let Some(page_size) = key.page_size {
            params.page_size = usize::try_from(page_size).context("页大小无效")?;
        }
        if let Some(kdf_iter) = key.kdf_iter {
            params.kdf_iter = kdf_iter;
        }
        sqlcipher::decrypt_with(bytes, &material, &params)
    } else {
        sqlcipher::decrypt(bytes, &material)
    };
    result.map_err(|error| match error {
        DecryptError::KeyOrParametersMismatch | DecryptError::TooSmall => {
            anyhow!("SQLITE_LOCKED: 密钥或兼容参数无法按页解密该数据库")
        }
        DecryptError::PageAuthFailed(page) => {
            anyhow!("第 {page} 页校验失败，数据库可能损坏或被篡改，无法完整解密")
        }
        DecryptError::TooLarge => anyhow!("数据库页数超出可寻址范围"),
    })
}

fn read_sidecar(
    image: &Arc<EvidenceSession>,
    sidecar: &sqlite_preview::SidecarNode,
    notes: &mut Vec<String>,
) -> Option<Vec<u8>> {
    match sqlite_preview::read_node_prefix(image, &sidecar.node_id) {
        Ok((_, true)) => {
            notes.push(format!("{} 超过 512 MiB，未参与恢复", sidecar.name));
            None
        }
        Ok((bytes, false)) => Some(bytes),
        Err(error) => {
            notes.push(format!("{} 读取失败，未参与恢复：{error:#}", sidecar.name));
            None
        }
    }
}

/// Recover deleted records from the SQLite database at `node_id`; `parent_id`
/// is its directory, where the `-wal` / `-journal` sidecars are looked up.
pub fn recover(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    parent_id: Option<&str>,
    key: &SqliteKey,
) -> anyhow::Result<SqliteRecovery> {
    let (bytes, truncated) = sqlite_preview::read_node_prefix(image, node_id)?;
    if truncated {
        bail!("数据库超过 512 MiB，暂不支持恢复已删除记录");
    }
    let mut notes = Vec::new();
    let mut sidecars = Vec::new();
    let sidecar_nodes = sqlite_preview::sidecar_nodes(image, node_id, parent_id);
    let find_sidecar = |suffix: &str| sidecar_nodes.iter().find(|node| node.suffix == suffix);

    let encrypted = !sqlite_preview::looks_like_sqlite(&bytes);
    let (plaintext, cipher) = if encrypted {
        let decrypted = decrypt(&bytes, key)?;
        let label = cipher_label(decrypted.version, decrypted.page_size);
        (decrypted.plaintext, Some(label))
    } else {
        (bytes, None)
    };

    let (wal, journal) = if encrypted {
        if find_sidecar("-wal").is_some() || find_sidecar("-journal").is_some() {
            notes.push("加密库的 -wal / -journal 也是密文，暂未参与恢复".to_string());
        }
        (None, None)
    } else {
        let wal = find_sidecar("-wal").and_then(|node| {
            read_sidecar(image, node, &mut notes)
                .filter(|bytes| !bytes.is_empty())
                .map(|bytes| (node.name.clone(), bytes))
        });
        // A WAL and a rollback journal are exclusive journal modes; with a WAL
        // in play the journal (if any) is stale.
        let journal = if wal.is_some() {
            None
        } else {
            find_sidecar("-journal").and_then(|node| {
                read_sidecar(image, node, &mut notes)
                    .filter(|bytes| !bytes.is_empty())
                    .map(|bytes| (node.name.clone(), bytes))
            })
        };
        (wal, journal)
    };

    let database = match &wal {
        Some((name, wal)) => {
            sidecars.push(name.clone());
            Database::open_with_wal(plaintext, wal)
        }
        None => Database::open(plaintext),
    }
    .map_err(|error| anyhow!("无法按 SQLite 格式解析数据库：{error:?}"))?;

    let carved = recovery::carve_all_deleted_records(&database);
    let attributions = recovery::attribute_records(&database, &carved);
    let mut records: Vec<RecoveredRecord> = carved
        .into_iter()
        .zip(attributions)
        .map(|(record, attribution)| carved_record(record, attribution))
        .collect();

    if let Some((name, journal)) = &journal {
        sidecars.push(name.clone());
        let recovered = recovery::carve_rollback_journal(&database, journal);
        let journal_page = |source: &RecoverySource| {
            if let RecoverySource::RollbackJournal(journal) = source {
                Some(journal.pgno)
            } else {
                None
            }
        };
        records.extend(recovered.deleted.into_iter().map(|row| RecoveredRecord {
            page: journal_page(&row.source),
            table: Some(row.table),
            link: TableLink::Known,
            origin: RecordOrigin::JournalDeleted,
            offset: None,
            rowid: Some(row.rowid),
            confidence: None,
            wal_frame: None,
            overflow_pages: Vec::new(),
            values: cells(&row.values),
            current_values: None,
        }));
        records.extend(recovered.modified.into_iter().map(|row| RecoveredRecord {
            page: journal_page(&row.source),
            table: Some(row.table),
            link: TableLink::Known,
            origin: RecordOrigin::JournalPrior,
            offset: None,
            rowid: Some(row.rowid),
            confidence: None,
            wal_frame: None,
            overflow_pages: Vec::new(),
            values: cells(&row.prior_values),
            current_values: Some(cells(&row.current_values)),
        }));
    }

    // Attributed tables first (by name), then by where the bytes sit.
    records.sort_by(|left, right| {
        (left.table.is_none(), &left.table, left.page, left.offset).cmp(&(
            right.table.is_none(),
            &right.table,
            right.page,
            right.offset,
        ))
    });
    let omitted = records.len().saturating_sub(MAX_RECORDS);
    records.truncate(MAX_RECORDS);

    let schemas = recovery::recover_dropped_schemas(&database)
        .into_iter()
        .map(|schema| RecoveredSchema {
            object_type: schema.object_type,
            name: schema.name,
            table_name: schema.tbl_name,
            root_page: schema.rootpage,
            sql: schema.sql,
        })
        .collect();

    Ok(SqliteRecovery {
        page_size: database.header().page_size,
        page_count: database.page_count(),
        freelist_pages: database.freelist_count(),
        cipher,
        sidecars,
        notes,
        records,
        omitted,
        schemas,
    })
}
