//! Journal-related filesystem IPC.
//!
//! This module keeps NTFS USN and ext4 jbd2 journal commands, cursor
//! pagination, and their focused tests out of the broad filesystem command
//! surface.

use std::time::Instant;

use crate::host::State;
use ::hex::{decode as hex_decode, encode as hex_encode};
use serde::{Deserialize, Serialize};

use crate::core::filesystem::tree::fs_tree;
use crate::core::filesystem::volumes::ext4_journal::{Ext4Journal, JournalBlockRef};
use crate::core::filesystem::volumes::ntfs_usn::UsnRecord;
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// Default cap on how much of a (possibly large) on-disk journal the journal
/// readers pull — 64 MiB covers a large amount of recent activity without
/// materialising the whole journal into memory.
const JOURNAL_READ_CAP: usize = 64 * 1024 * 1024;
const JOURNAL_PAGE_DEFAULT_SIZE: usize = 1_000;
const JOURNAL_PAGE_MAX_SIZE: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsnJournalPage {
    pub items: Vec<UsnRecord>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ext4JournalBlocksPage {
    pub block_size: u32,
    pub transactions: u32,
    pub items: Vec<JournalBlockRef>,
    pub revoked_total: usize,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    pub truncated: bool,
}

/// Mines the ext4 jbd2 journal for the volume `node_id` belongs to (pass an
/// `ext4:` node id, or a `part:N` / `lvm:N:M` volume key on an ext4 volume).
/// Returns historical block revisions and revoked-block lists recovered from
/// the journal — prior versions of inode-table / directory / bitmap blocks the
/// live filesystem has since overwritten.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_ext4_journal(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<Ext4Journal, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let started = Instant::now();
    tracing::info!("fs_ext4_journal begin image={image_id} node={node_id}");
    let result = tokio::task::spawn_blocking(move || -> Result<Ext4Journal, AppError> {
        fs_tree::ext4_journal(&image, &node_id, JOURNAL_READ_CAP).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_ext4_journal", err))?;
    match &result {
        Ok(journal) => tracing::info!(
            "fs_ext4_journal done image={image_id} elapsed={:.3?} txns={} blocks={} revoked={}",
            started.elapsed(),
            journal.transactions,
            journal.blocks.len(),
            journal.revoked.len(),
        ),
        Err(err) => tracing::warn!("fs_ext4_journal err image={image_id} err={}", err.message),
    }
    result
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_ext4_journal_blocks_page(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    cursor: Option<String>,
    page_size: Option<usize>,
) -> Result<Ext4JournalBlocksPage, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let started = Instant::now();
    tracing::debug!("fs_ext4_journal_blocks_page begin image={image_id} node={node_id}");
    let result = tokio::task::spawn_blocking(move || {
        let journal = fs_tree::ext4_journal(&image, &node_id, JOURNAL_READ_CAP)
            .map_err(AppError::from_anyhow)?;
        ext4_journal_blocks_page(journal, cursor.as_deref(), page_size)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_ext4_journal_blocks_page", err))?;
    match &result {
        Ok(page) => tracing::debug!(
            "fs_ext4_journal_blocks_page done image={image_id} elapsed={:.3?} items={} next_cursor={}",
            started.elapsed(),
            page.items.len(),
            page.next_cursor.is_some(),
        ),
        Err(err) => tracing::warn!(
            "fs_ext4_journal_blocks_page err image={image_id} err={}",
            err.message
        ),
    }
    result
}

/// Reads the NTFS `$UsnJrnl` change journal for the volume that `node_id`
/// belongs to (pass a `"part:N"` / `"lvm:N:M"` volume key or any `ntfs:` node
/// id on that volume). Returns decoded USN records — per-file create / delete
/// / rename history that survives MFT-record reuse.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_usn_journal(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<Vec<UsnRecord>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let started = Instant::now();
    tracing::info!("fs_usn_journal begin image={image_id} node={node_id}");
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<UsnRecord>, AppError> {
        fs_tree::ntfs_usn_journal(&image, &node_id, JOURNAL_READ_CAP).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_usn_journal", err))?;
    match &result {
        Ok(records) => tracing::info!(
            "fs_usn_journal done image={image_id} elapsed={:.3?} records={}",
            started.elapsed(),
            records.len(),
        ),
        Err(err) => tracing::warn!("fs_usn_journal err image={image_id} err={}", err.message),
    }
    result
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_usn_journal_page(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    cursor: Option<String>,
    page_size: Option<usize>,
) -> Result<UsnJournalPage, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let started = Instant::now();
    tracing::debug!("fs_usn_journal_page begin image={image_id} node={node_id}");
    let result = tokio::task::spawn_blocking(move || {
        let records = fs_tree::ntfs_usn_journal(&image, &node_id, JOURNAL_READ_CAP)
            .map_err(AppError::from_anyhow)?;
        usn_journal_page(records, cursor.as_deref(), page_size)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_usn_journal_page", err))?;
    match &result {
        Ok(page) => tracing::debug!(
            "fs_usn_journal_page done image={image_id} elapsed={:.3?} items={} next_cursor={}",
            started.elapsed(),
            page.items.len(),
            page.next_cursor.is_some(),
        ),
        Err(err) => tracing::warn!(
            "fs_usn_journal_page err image={image_id} err={}",
            err.message
        ),
    }
    result
}

fn usn_journal_page(
    mut records: Vec<UsnRecord>,
    cursor: Option<&str>,
    page_size: Option<usize>,
) -> Result<UsnJournalPage, AppError> {
    records.sort_by(usn_record_order);
    let page_size = journal_page_size(page_size);
    let start = match cursor {
        Some(cursor) => {
            let cursor_key = decode_usn_cursor(cursor)?;
            records
                .iter()
                .position(|record| usn_record_is_after_cursor(record, &cursor_key))
                .unwrap_or(records.len())
        }
        None => 0,
    };
    let total = records.len();
    let end = start.saturating_add(page_size).min(total);
    let items = records[start..end].to_vec();
    let next_cursor = if end < total {
        items.last().map(usn_cursor_for_record)
    } else {
        None
    };
    Ok(UsnJournalPage {
        items,
        next_cursor,
        total: Some(total),
        truncated: false,
    })
}

fn ext4_journal_blocks_page(
    journal: Ext4Journal,
    cursor: Option<&str>,
    page_size: Option<usize>,
) -> Result<Ext4JournalBlocksPage, AppError> {
    let mut blocks = journal.blocks;
    blocks.sort_by(ext4_block_order);
    let page_size = journal_page_size(page_size);
    let start = match cursor {
        Some(cursor) => {
            let cursor_key = decode_ext4_block_cursor(cursor)?;
            blocks
                .iter()
                .position(|block| ext4_block_key(block) > cursor_key)
                .unwrap_or(blocks.len())
        }
        None => 0,
    };
    let total = blocks.len();
    let end = start.saturating_add(page_size).min(total);
    let items = blocks[start..end].to_vec();
    let next_cursor = if end < total {
        items.last().map(ext4_block_cursor)
    } else {
        None
    };
    Ok(Ext4JournalBlocksPage {
        block_size: journal.block_size,
        transactions: journal.transactions,
        items,
        revoked_total: journal.revoked.len(),
        next_cursor,
        total: Some(total),
        truncated: false,
    })
}

fn journal_page_size(page_size: Option<usize>) -> usize {
    page_size
        .unwrap_or(JOURNAL_PAGE_DEFAULT_SIZE)
        .clamp(1, JOURNAL_PAGE_MAX_SIZE)
}

fn usn_record_order(left: &UsnRecord, right: &UsnRecord) -> std::cmp::Ordering {
    usn_record_key(left).cmp(&usn_record_key(right))
}

fn usn_record_key(record: &UsnRecord) -> (u64, u64, u64, &str) {
    (
        record.usn,
        record.file_record,
        record.parent_record,
        record.name.as_str(),
    )
}

fn usn_record_is_after_cursor(record: &UsnRecord, cursor: &(u64, u64, u64, String)) -> bool {
    record.usn > cursor.0
        || (record.usn == cursor.0
            && (record.file_record > cursor.1
                || (record.file_record == cursor.1
                    && (record.parent_record > cursor.2
                        || (record.parent_record == cursor.2
                            && record.name.as_str() > cursor.3.as_str())))))
}

fn usn_cursor_for_record(record: &UsnRecord) -> String {
    format!(
        "{:016x}:{:016x}:{:016x}:{}",
        record.usn,
        record.file_record,
        record.parent_record,
        hex_encode(record.name.as_bytes())
    )
}

fn decode_usn_cursor(cursor: &str) -> Result<(u64, u64, u64, String), AppError> {
    let mut parts = cursor.splitn(4, ':');
    let usn = decode_fixed_hex_u64(parts.next(), "usn")?;
    let file_record = decode_fixed_hex_u64(parts.next(), "file record")?;
    let parent_record = decode_fixed_hex_u64(parts.next(), "parent record")?;
    let name_hex = parts
        .next()
        .ok_or_else(|| AppError::invalid_input("invalid USN journal cursor"))?;
    let name = String::from_utf8(hex_decode(name_hex).map_err(|err| {
        AppError::invalid_input(format!("invalid USN journal cursor name: {err}"))
    })?)
    .map_err(|err| {
        AppError::invalid_input(format!("invalid USN journal cursor name utf8: {err}"))
    })?;
    Ok((usn, file_record, parent_record, name))
}

fn ext4_block_order(left: &JournalBlockRef, right: &JournalBlockRef) -> std::cmp::Ordering {
    ext4_block_key(left).cmp(&ext4_block_key(right))
}

fn ext4_block_key(block: &JournalBlockRef) -> (u32, u64, u64) {
    (block.sequence, block.journal_offset, block.fs_block)
}

fn ext4_block_cursor(block: &JournalBlockRef) -> String {
    format!(
        "{:08x}:{:016x}:{:016x}",
        block.sequence, block.journal_offset, block.fs_block
    )
}

fn decode_ext4_block_cursor(cursor: &str) -> Result<(u32, u64, u64), AppError> {
    let mut parts = cursor.splitn(3, ':');
    let sequence = decode_fixed_hex_u32(parts.next(), "sequence")?;
    let journal_offset = decode_fixed_hex_u64(parts.next(), "journal offset")?;
    let fs_block = decode_fixed_hex_u64(parts.next(), "filesystem block")?;
    Ok((sequence, journal_offset, fs_block))
}

fn decode_fixed_hex_u32(value: Option<&str>, label: &str) -> Result<u32, AppError> {
    let value = value.ok_or_else(|| {
        AppError::invalid_input(format!("invalid journal cursor: missing {label}"))
    })?;
    u32::from_str_radix(value, 16)
        .map_err(|err| AppError::invalid_input(format!("invalid journal cursor {label}: {err}")))
}

fn decode_fixed_hex_u64(value: Option<&str>, label: &str) -> Result<u64, AppError> {
    let value = value.ok_or_else(|| {
        AppError::invalid_input(format!("invalid journal cursor: missing {label}"))
    })?;
    u64::from_str_radix(value, 16)
        .map_err(|err| AppError::invalid_input(format!("invalid journal cursor {label}: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usn(usn: u64, file_record: u64, parent_record: u64, name: &str) -> UsnRecord {
        UsnRecord {
            usn,
            file_record,
            parent_record,
            timestamp: None,
            reasons: Vec::new(),
            name: name.to_string(),
            file_attributes: 0,
        }
    }

    #[test]
    fn usn_journal_page_cursor_resumes_after_stable_key() {
        let records = vec![
            usn(20, 1, 0, "b.txt"),
            usn(10, 1, 0, "a.txt"),
            usn(20, 1, 0, "c.txt"),
        ];

        let first = usn_journal_page(records.clone(), None, Some(2)).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );
        let second = usn_journal_page(records, first.next_cursor.as_deref(), Some(2)).unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>(),
            vec!["c.txt"]
        );
        assert!(second.next_cursor.is_none());
    }

    fn journal_block(sequence: u32, journal_offset: u64, fs_block: u64) -> JournalBlockRef {
        JournalBlockRef {
            sequence,
            journal_offset,
            fs_block,
        }
    }

    #[test]
    fn ext4_journal_blocks_page_cursor_resumes_after_stable_key() {
        let journal = Ext4Journal {
            block_size: 4096,
            transactions: 2,
            blocks: vec![
                journal_block(2, 8192, 9),
                journal_block(1, 4096, 8),
                journal_block(2, 12_288, 10),
            ],
            revoked: vec![99, 100],
        };

        let first = ext4_journal_blocks_page(journal.clone(), None, Some(2)).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|block| block.fs_block)
                .collect::<Vec<_>>(),
            vec![8, 9]
        );
        assert_eq!(first.revoked_total, 2);
        let second =
            ext4_journal_blocks_page(journal, first.next_cursor.as_deref(), Some(2)).unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|block| block.fs_block)
                .collect::<Vec<_>>(),
            vec![10]
        );
        assert!(second.next_cursor.is_none());
    }
}
