//! Bounded string results extracted from the shared seekable/streaming file reader.
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{CliError, Output};
use crate::{
    core::filesystem::{evidence_session::EvidenceSession, tree::fs_tree::FsSession},
    state::AppState,
    task_manager::TaskKind,
};

const MAX_STRING_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Encoding {
    Ascii,
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "utf-16le")]
    Utf16le,
    #[serde(rename = "utf-16be")]
    Utf16be,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Record {
    offset: u64,
    text: String,
    truncated: bool,
}

struct Scanner {
    encoding: Encoding,
    minimum: usize,
    limit: usize,
    position: u64,
    records: Vec<Record>,
    output_bytes: usize,
    text: String,
    characters: usize,
    start: u64,
    text_truncated: bool,
    truncated: bool,
    stopped: bool,
    pending: Vec<u8>,
    high_surrogate: Option<(u16, u64)>,
}

impl Scanner {
    fn new(encoding: Encoding, minimum: usize, limit: usize, offset: u64) -> Self {
        Self {
            encoding,
            minimum,
            limit,
            position: offset,
            records: Vec::new(),
            output_bytes: 0,
            text: String::new(),
            characters: 0,
            start: offset,
            text_truncated: false,
            truncated: false,
            stopped: false,
            pending: Vec::with_capacity(4),
            high_surrogate: None,
        }
    }

    fn character(&mut self, character: char, offset: u64) {
        if (character.is_control() && character != '\t') || character == '\u{feff}' {
            self.separator();
            return;
        }
        if self.characters == 0 {
            self.start = offset;
        }
        self.characters = self.characters.saturating_add(1);
        if !self.text_truncated && self.text.len() + character.len_utf8() <= MAX_STRING_BYTES {
            self.text.push(character);
        } else {
            self.text_truncated = true;
        }
    }

    fn separator(&mut self) {
        if self.characters >= self.minimum {
            if self.records.len() >= self.limit
                || self.output_bytes + self.text.len() > MAX_OUTPUT_BYTES
            {
                self.truncated = true;
                self.stopped = true;
            } else {
                self.output_bytes += self.text.len();
                self.truncated |= self.text_truncated;
                self.records.push(Record {
                    offset: self.start,
                    text: std::mem::take(&mut self.text),
                    truncated: self.text_truncated,
                });
            }
        }
        self.text.clear();
        self.characters = 0;
        self.text_truncated = false;
    }

    fn utf8(&mut self, byte: u8, offset: u64) {
        if !self.pending.is_empty() && !(0x80..=0xbf).contains(&byte) {
            self.pending.clear();
            self.separator();
        }
        if self.pending.is_empty() {
            if byte.is_ascii() {
                self.character(char::from(byte), offset);
            } else if (0xc2..=0xf4).contains(&byte) {
                self.pending.push(byte);
            } else {
                self.separator();
            }
            return;
        }
        self.pending.push(byte);
        let width = match self.pending.first() {
            Some(0xc2..=0xdf) => 2,
            Some(0xe0..=0xef) => 3,
            Some(0xf0..=0xf4) => 4,
            _ => 0,
        };
        if self.pending.len() == width {
            let character = std::str::from_utf8(&self.pending)
                .ok()
                .and_then(|text| text.chars().next());
            self.pending.clear();
            match character {
                Some(character) => self.character(character, offset + 1 - width as u64),
                None => self.separator(),
            }
        }
    }

    fn utf16(&mut self, byte: u8, offset: u64) {
        let Some(first) = self.pending.pop() else {
            self.pending.push(byte);
            return;
        };
        let unit = match self.encoding {
            Encoding::Utf16le => u16::from_le_bytes([first, byte]),
            Encoding::Utf16be => u16::from_be_bytes([first, byte]),
            Encoding::Ascii | Encoding::Utf8 => return,
        };
        if let Some((high, start)) = self.high_surrogate.take() {
            if (0xdc00..=0xdfff).contains(&unit) {
                let scalar =
                    0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                if let Some(character) = char::from_u32(scalar) {
                    self.character(character, start);
                }
                return;
            }
            self.separator();
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.high_surrogate = Some((unit, offset - 1));
        } else if let Some(character) = char::from_u32(u32::from(unit)) {
            self.character(character, offset - 1);
        } else {
            self.separator();
        }
    }

    fn push(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            let offset = self.position;
            self.position += 1;
            match self.encoding {
                Encoding::Ascii if byte.is_ascii() => self.character(char::from(byte), offset),
                Encoding::Ascii => self.separator(),
                Encoding::Utf8 => self.utf8(byte, offset),
                Encoding::Utf16le | Encoding::Utf16be => self.utf16(byte, offset),
            }
            if self.stopped {
                return false;
            }
        }
        true
    }

    fn finish(&mut self) {
        self.separator();
    }
}

#[derive(Debug)]
struct ResultLimitReached;
impl std::fmt::Display for ResultLimitReached {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("strings result limit reached")
    }
}
impl std::error::Error for ResultLimitReached {}

pub(super) async fn read(
    state: &AppState,
    image: Arc<EvidenceSession>,
    node_id: String,
    values: &Map<String, Value>,
) -> Result<Output, CliError> {
    let encoding: Encoding = serde_json::from_value(
        values
            .get("encoding")
            .cloned()
            .unwrap_or_else(|| json!("ascii")),
    )
    .map_err(|_| CliError::invalid("编码只能是 ascii、utf-8、utf-16le 或 utf-16be"))?;
    let number = |name: &str| values.get(name).and_then(Value::as_u64);
    let minimum = number("minLength").unwrap_or(4).clamp(1, 4096) as usize;
    let limit = number("limit").unwrap_or(10_000).clamp(1, 100_000) as usize;
    let offset = number("offset").unwrap_or(0);
    let length = number("length").filter(|length| *length > 0);
    let task_id = format!("cli-strings-{}", uuid::Uuid::new_v4());
    let task = state.tasks.register(
        task_id.clone(),
        TaskKind::MediaMaterialization,
        Some(image.id.clone()),
    );
    let permit = state
        .tasks
        .acquire_resources(&task)
        .await
        .map_err(crate::app::error::AppError::from)
        .map_err(super::command_error)?;
    let cancellation = task.token();
    tokio::task::spawn_blocking(move || -> Result<Output, CliError> {
        let _permit = permit;
        let mut session = FsSession::new(&image);
        let size = session
            .file_stat(&node_id)
            .map_err(super::internal_error)?
            .size;
        if offset > size {
            return Err(CliError::invalid("偏移超过文件大小"));
        }
        let length = length.unwrap_or(size - offset).min(size - offset);
        let mut scanner = Scanner::new(encoding, minimum, limit, offset);
        let (result, faults) = image.collect_read_faults(|| {
            session.stream_file_range(&node_id, offset, length, 1024 * 1024, |chunk| {
                anyhow::ensure!(!cancellation.is_cancelled(), "字符串提取已取消");
                if !scanner.push(chunk) {
                    return Err(ResultLimitReached.into());
                }
                Ok(())
            })
        });
        match result {
            Ok(bytes_read) if bytes_read == length => scanner.finish(),
            Ok(bytes_read) => {
                return Err(CliError::invalid(format!(
                    "文件未完整读取：期望 {length} 字节，实际 {bytes_read} 字节"
                )))
            }
            Err(error) if error.downcast_ref::<ResultLimitReached>().is_some() => {}
            Err(error) => return Err(super::internal_error(error)),
        }
        image
            .validate_source_file_stable()
            .map_err(super::internal_error)?;
        super::json_output(Extracted {
            strings: scanner.records,
            encoding,
            offset,
            size,
            bytes_read: scanner.position - offset,
            truncated: scanner.truncated || offset + length < size,
            task_id,
            read_faults: faults,
        })
    })
    .await
    .map_err(super::internal_error)?
}

#[derive(Serialize)]
struct Extracted<R, E, F> {
    strings: R,
    encoding: E,
    offset: u64,
    size: u64,
    bytes_read: u64,
    truncated: bool,
    task_id: String,
    read_faults: F,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(encoding: Encoding, bytes: &[u8], chunk_size: usize) -> Vec<Record> {
        let mut scanner = Scanner::new(encoding, 4, 100, 10);
        for chunk in bytes.chunks(chunk_size) {
            assert!(scanner.push(chunk));
        }
        scanner.finish();
        scanner.records
    }

    #[test]
    fn strings_and_multibyte_characters_survive_every_chunk_boundary() {
        let text = "hello中文世界😀";
        let utf8 = format!("\0{text}\0abc").into_bytes();
        let utf16le = format!("\0{text}\0abc")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let utf16be = format!("\0{text}\0abc")
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        for chunk_size in 1..=8 {
            assert_eq!(
                extract(Encoding::Utf8, &utf8, chunk_size),
                vec![Record {
                    offset: 11,
                    text: text.into(),
                    truncated: false
                }]
            );
            for (encoding, bytes) in [(Encoding::Utf16le, &utf16le), (Encoding::Utf16be, &utf16be)]
            {
                assert_eq!(
                    extract(encoding, bytes, chunk_size),
                    vec![Record {
                        offset: 12,
                        text: text.into(),
                        truncated: false
                    }]
                );
            }
            assert_eq!(
                extract(Encoding::Ascii, b"\0hello\xffworld\0abc", chunk_size),
                vec![
                    Record {
                        offset: 11,
                        text: "hello".into(),
                        truncated: false
                    },
                    Record {
                        offset: 17,
                        text: "world".into(),
                        truncated: false
                    },
                ]
            );
        }
    }

    #[test]
    fn malformed_sequences_are_separators_and_limits_are_explicit() {
        assert_eq!(
            extract(Encoding::Utf8, b"hello\xc0\xafworld\xe4", 1),
            vec![
                Record {
                    offset: 10,
                    text: "hello".into(),
                    truncated: false
                },
                Record {
                    offset: 17,
                    text: "world".into(),
                    truncated: false
                },
            ]
        );
        let mut scanner = Scanner::new(Encoding::Ascii, 4, 1, 0);
        assert!(!scanner.push(b"hello\0world\0"));
        assert_eq!((scanner.records.len(), scanner.truncated), (1, true));
        let mut scanner = Scanner::new(Encoding::Ascii, 4, 1, 0);
        assert!(scanner.push(&vec![b'x'; MAX_STRING_BYTES + 1]));
        scanner.finish();
        assert_eq!(
            (
                scanner.records[0].text.len(),
                scanner.records[0].truncated,
                scanner.truncated
            ),
            (MAX_STRING_BYTES, true, true)
        );
    }
}
