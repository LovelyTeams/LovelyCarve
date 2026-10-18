//! Vim swap (`.swp`/`.swo`/`.swn`) preview.
//!
//! Vim writes the unsaved buffer state to a swap file as the user edits.
//! Forensically this is gold — the swap reflects vim's in-memory buffer
//! at last sync, which can be *newer* than (and contradict) whatever the
//! file on disk says. The classic case is `/root/.env.swp` left over
//! when an admin's session died mid-edit: the disk `.env` may be the
//! prior committed version while the swap holds the half-typed secret.
//!
//! What we do here:
//! - Validate the `b0VIM` magic and parse block 0 metadata (original
//!   filename, user, host, mtime, page size, pid).
//! - Walk subsequent pages. Data blocks (`ad` id) hold lines as
//!   NUL-terminated bytes packed inside a text region rooted at
//!   `db_txt_start`; we extract those.
//! - Pointer blocks (`dp` / `tp` id) are accounted for but not chased —
//!   we just want every reachable line, not the precise tree.
//!
//! Caveats baked into the UI string output:
//! - Vim stores text *downward* inside a data block (new inserts move
//!   `db_txt_start` lower), so the on-disk byte order is roughly
//!   newest-first within a block. We surface lines as-found and let the
//!   user judge order from content.
//! - Encrypted swap files (vim 7.3+ blowfish/blowfish2, vim 8.2+ sodium)
//!   keep a readable `b0VIM` header but ciphered data blocks. We detect
//!   "header present but zero recoverable data blocks" and flag the
//!   digest as encrypted.

use std::io::{Cursor, SeekFrom};
use std::sync::Arc;

use binrw::BinRead;
use chrono::{TimeZone, Utc};
use serde::Serialize;

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;

/// Hard cap on bytes streamed from the image. A real `.swp` is bounded
/// by `'maxmem'` (default a few MB) — 64 MiB leaves margin for tampered
/// or unusually large editor sessions without letting a malicious /
/// damaged file balloon our memory.
const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECOVERED_LINES: usize = 20_000;
const MAX_LINE_BYTES: usize = 16 * 1024;
const HEADER_BYTES: usize = 1024;

#[derive(Debug, Serialize)]
pub struct VimSwapDigest {
    pub meta: VimSwapMeta,
    pub lines: Vec<String>,
    pub bytes_read: u64,
    pub truncated: bool,
    pub data_block_count: usize,
    pub pointer_block_count: usize,
    pub unknown_block_count: usize,
    pub encrypted: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VimSwapMeta {
    pub version: String,
    pub page_size: u32,
    pub mtime: u32,
    pub mtime_iso: Option<String>,
    pub inode: u32,
    pub pid: u32,
    pub user: String,
    pub host: String,
    pub fname: String,
}

#[derive(Debug, BinRead)]
#[br(little)]
struct VimSwapHeader {
    magic: [u8; 6],
    #[br(seek_before = SeekFrom::Start(2))]
    version: [u8; 10],
    page_size: u32,
    mtime: u32,
    inode: u32,
    pid: u32,
    user: [u8; 40],
    host: [u8; 40],
    fname: [u8; 910],
}

#[derive(Debug, BinRead)]
#[br(little, magic = b"ad")]
struct VimSwapDataBlockHeader {
    #[br(seek_before = SeekFrom::Start(8))]
    txt_start: u32,
}

pub fn looks_like_vim_swap(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[..6] == b"b0VIM "
}

pub fn preview_vim_swap(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<VimSwapDigest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    fs_tree::stream_file_chunks(image, node_id, 1024 * 1024, |chunk| {
        if (buf.len() as u64) >= MAX_PREVIEW_BYTES {
            truncated = true;
            return Ok(());
        }
        let remaining = (MAX_PREVIEW_BYTES as usize).saturating_sub(buf.len());
        let take = chunk.len().min(remaining);
        if take == 0 {
            truncated = true;
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            truncated = true;
        }
        Ok(())
    })?;
    parse_swap(&buf, truncated)
}

pub fn parse_swap(bytes: &[u8], truncated: bool) -> anyhow::Result<VimSwapDigest> {
    if !looks_like_vim_swap(bytes) {
        anyhow::bail!("not a vim swap file (missing b0VIM magic)");
    }
    if bytes.len() < HEADER_BYTES {
        anyhow::bail!("vim swap header truncated ({} bytes)", bytes.len());
    }

    let header = VimSwapHeader::read(&mut Cursor::new(&bytes[..HEADER_BYTES]))
        .map_err(|err| anyhow::anyhow!("vim swap header parse failed: {err}"))?;
    if header.magic != *b"b0VIM " {
        anyhow::bail!("not a vim swap file (missing b0VIM magic)");
    }
    let version = c_string(&header.version);
    let raw_page_size = header.page_size;
    let mtime = header.mtime;
    let inode = header.inode;
    let pid = header.pid;
    let user = c_string(&header.user);
    let host = c_string(&header.host);
    let fname = c_string(&header.fname);

    let mut notes: Vec<String> = Vec::new();
    let page_size: u32 = if (1024..=65536).contains(&raw_page_size) {
        raw_page_size
    } else {
        notes.push(format!(
            "block0 page_size={raw_page_size} 不合理，回退到 4096"
        ));
        4096
    };

    let page = page_size as usize;
    let mut lines: Vec<String> = Vec::new();
    let mut data_blocks: usize = 0;
    let mut pointer_blocks: usize = 0;
    let mut unknown_blocks: usize = 0;
    let mut hit_line_cap = false;

    let mut offset = page;
    while offset + 2 <= bytes.len() {
        let end = (offset + page).min(bytes.len());
        let block = &bytes[offset..end];
        let id = &block[..2.min(block.len())];
        if id == b"ad" {
            data_blocks += 1;
            extract_lines_from_data_block(block, &mut lines);
            if lines.len() >= MAX_RECOVERED_LINES {
                hit_line_cap = true;
                break;
            }
        } else if id == b"dp" || id == b"tp" {
            pointer_blocks += 1;
        } else {
            unknown_blocks += 1;
        }
        offset += page;
    }

    if hit_line_cap {
        notes.push(format!(
            "已提取 {MAX_RECOVERED_LINES} 行，后续 data block 不再展开"
        ));
        lines.truncate(MAX_RECOVERED_LINES);
    }

    let encrypted = data_blocks == 0 && (pointer_blocks > 0 || unknown_blocks > 0);
    if encrypted {
        notes.push(
            "未发现 data block（id 'ad'）。swap 文件可能使用 blowfish / sodium 加密，或已被覆盖损坏。"
                .to_string(),
        );
    }

    Ok(VimSwapDigest {
        meta: VimSwapMeta {
            version,
            page_size,
            mtime,
            mtime_iso: mtime_iso(mtime),
            inode,
            pid,
            user,
            host,
            fname,
        },
        lines,
        bytes_read: bytes.len() as u64,
        truncated,
        data_block_count: data_blocks,
        pointer_block_count: pointer_blocks,
        unknown_block_count: unknown_blocks,
        encrypted,
        notes,
    })
}

fn extract_lines_from_data_block(block: &[u8], out: &mut Vec<String>) {
    // data_block layout (memline.c):
    //   off 0  : id "ad"       (2 bytes, then 2 bytes padding)
    //   off 4  : db_free       (u32 LE)
    //   off 8  : db_txt_start  (u32 LE)
    //   off 12 : db_txt_end    (u32 LE)
    //   off 16 : db_line_count (linenr_T, 4 or 8 bytes — platform-dep)
    //   off N  : db_index[]    (linenr_T entries growing toward txt_start)
    //   ... free space ...
    //   [db_txt_start .. db_txt_end] : NUL-terminated line bytes
    //
    // We trust db_txt_start to locate the text region. Lines are written
    // bottom-up inside that region, so the byte order is approximately
    // newest-first — fine for content recovery, the UI flags the order
    // caveat.
    if block.len() < 16 {
        return;
    }
    let txt_start = VimSwapDataBlockHeader::read(&mut Cursor::new(block))
        .map(|header| header.txt_start as usize)
        .unwrap_or(0);
    let region = if (16..block.len()).contains(&txt_start) {
        &block[txt_start..]
    } else {
        // Header field unreadable — fall back to scanning the back half
        // of the block, which is where text always lives.
        let fallback = block.len() / 4;
        &block[fallback..]
    };
    scan_nul_terminated(region, out);
}

fn scan_nul_terminated(buf: &[u8], out: &mut Vec<String>) {
    let mut start = 0usize;
    for idx in 0..buf.len() {
        if buf[idx] != 0 {
            continue;
        }
        emit_line(&buf[start..idx], out);
        start = idx + 1;
        if out.len() >= MAX_RECOVERED_LINES {
            return;
        }
    }
    if start < buf.len() {
        emit_line(&buf[start..], out);
    }
}

fn emit_line(slice: &[u8], out: &mut Vec<String>) {
    if slice.is_empty() || slice.len() > MAX_LINE_BYTES {
        return;
    }
    if !is_mostly_printable(slice) {
        return;
    }
    out.push(String::from_utf8_lossy(slice).into_owned());
}

fn is_mostly_printable(slice: &[u8]) -> bool {
    if slice.is_empty() {
        return false;
    }
    let printable = slice
        .iter()
        .filter(|&&b| {
            b == b'\t' || b == b'\n' || b == b'\r' || (0x20..0x7f).contains(&b) || b >= 0x80
        })
        .count();
    printable * 100 / slice.len() >= 85
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end])
        .trim_matches(char::from(0))
        .trim()
        .to_string()
}

fn mtime_iso(mtime: u32) -> Option<String> {
    if mtime == 0 {
        return None;
    }
    Utc.timestamp_opt(mtime as i64, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_header() -> [u8; HEADER_BYTES] {
        let mut h = [0u8; HEADER_BYTES];
        h[..6].copy_from_slice(b"b0VIM ");
        h[6..10].copy_from_slice(b"9.0\0");
        // page_size = 4096
        h[12..16].copy_from_slice(&4096u32.to_le_bytes());
        // mtime
        h[16..20].copy_from_slice(&1_700_000_000u32.to_le_bytes());
        // user "root"
        h[28..32].copy_from_slice(b"root");
        // host "web2"
        h[68..72].copy_from_slice(b"web2");
        // fname "/root/.env"
        h[108..118].copy_from_slice(b"/root/.env");
        h
    }

    #[test]
    fn detects_magic() {
        assert!(looks_like_vim_swap(b"b0VIM 9.0\0..."));
        assert!(!looks_like_vim_swap(b"not vim"));
        assert!(!looks_like_vim_swap(b"b0VIM"));
    }

    #[test]
    fn parses_header_and_recovers_lines() {
        let mut buf = vec![0u8; 4096 * 2];
        buf[..HEADER_BYTES].copy_from_slice(&build_header());

        // Build a fake data block at offset 4096.
        let block_off = 4096usize;
        buf[block_off..block_off + 2].copy_from_slice(b"ad");
        // db_txt_start = block_off + 64 => relative 64
        buf[block_off + 8..block_off + 12].copy_from_slice(&64u32.to_le_bytes());
        // text region: write three NUL-terminated strings
        let region_start = block_off + 64;
        let payload = b"DB_PASSWORD=hunter2\0SECRET_KEY=abc123\0LAST_LINE\0";
        buf[region_start..region_start + payload.len()].copy_from_slice(payload);

        let digest = parse_swap(&buf, false).expect("parse ok");
        assert_eq!(digest.meta.user, "root");
        assert_eq!(digest.meta.host, "web2");
        assert_eq!(digest.meta.fname, "/root/.env");
        assert_eq!(digest.meta.page_size, 4096);
        assert!(digest.meta.mtime_iso.is_some());
        assert_eq!(digest.data_block_count, 1);
        assert!(digest.lines.iter().any(|l| l == "DB_PASSWORD=hunter2"));
        assert!(digest.lines.iter().any(|l| l == "SECRET_KEY=abc123"));
        assert!(digest.lines.iter().any(|l| l == "LAST_LINE"));
        assert!(!digest.encrypted);
    }

    #[test]
    fn flags_encrypted_when_no_data_blocks() {
        let mut buf = vec![0u8; 4096 * 3];
        buf[..HEADER_BYTES].copy_from_slice(&build_header());
        // Two pages of garbage that do not match any known block id
        buf[4096] = b'X';
        buf[4096 + 1] = b'Y';
        buf[8192] = b'Q';
        buf[8192 + 1] = b'Z';
        let digest = parse_swap(&buf, false).expect("parse ok");
        assert_eq!(digest.data_block_count, 0);
        assert!(digest.encrypted);
        assert!(digest.notes.iter().any(|n| n.contains("加密")));
    }

    #[test]
    fn rejects_non_swap() {
        let err = parse_swap(b"hello world", false).unwrap_err();
        assert!(err.to_string().contains("not a vim swap"));
    }
}
