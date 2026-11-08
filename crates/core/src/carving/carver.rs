//! Digler-compatible raw file carving.
//!
//! The scan loop intentionally follows Digler's block-oriented model: a 4 MiB
//! buffer is examined at 512-byte block boundaries, matching format signatures
//! are tried in registry order, and a successful decoder advances by the
//! carved file's rounded block count. Format decoders are Rust ports of Digler's
//! MIT-licensed implementations; attribution is recorded in THIRD_PARTY_NOTICES.md.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Context;
use serde::Serialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::evidence::image::DiskImage;

pub(crate) const SCAN_BUFFER_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const BLOCK_BYTES: u64 = 512;
/// Worker cap for automatic sizing; decoding is CPU-bound but every worker
/// also streams evidence, and wider pools stop paying off on large images.
const MAX_AUTO_WORKERS: usize = 8;
pub const DEFAULT_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024;
pub const MAX_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_RESULTS: usize = 20_000;
pub const MAX_RESULTS: usize = 100_000;

#[derive(Debug, Clone, Serialize)]
pub struct CarveFormatInfo {
    pub id: String,
    pub extension: String,
    pub label: String,
    pub category: String,
    pub description: String,
    pub signatures: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarveArtifact {
    pub id: String,
    pub name: String,
    pub format_id: String,
    pub extension: String,
    pub category: String,
    pub offset: u64,
    pub length: u64,
    pub confidence: u8,
    pub region: String,
    pub read_fault: bool,
    #[serde(skip_serializing)]
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarveRegion {
    pub offset: u64,
    pub length: u64,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct CarveScanConfig {
    pub format_ids: Vec<String>,
    pub max_file_size: u64,
    pub max_results: usize,
    /// Scan worker threads; `None` sizes the pool from the available cores.
    pub worker_threads: Option<usize>,
}

impl Default for CarveScanConfig {
    fn default() -> Self {
        Self {
            format_ids: Vec::new(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_results: DEFAULT_MAX_RESULTS,
            worker_threads: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CarveScanProgress {
    pub bytes_scanned: u64,
    pub bytes_total: u64,
    pub signatures_found: u64,
    pub artifacts_found: u64,
    pub current_offset: u64,
}

#[derive(Debug, Clone)]
pub enum CarveScanEvent {
    Artifact(CarveArtifact),
    Progress(CarveScanProgress),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarveScanOutput {
    pub bytes_scanned: u64,
    pub bytes_total: u64,
    pub signatures_found: u64,
    pub artifacts_found: u64,
    pub truncated: bool,
}

#[derive(Debug, Error)]
pub enum CarveScanError {
    #[error("carve scan cancelled")]
    Cancelled,
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Random-access byte source consumed by the carver. Implementations must
/// return a short read at end-of-source and must not mutate evidence bytes.
pub trait CarveSource: Send + Sync {
    fn len(&self) -> u64;
    fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize>;

    /// Handle for one parallel scan worker. Sources whose decoder serializes
    /// every read behind a single lock should hand out independent handles so
    /// workers read concurrently; others can share themselves.
    fn worker_handle(&self) -> Self
    where
        Self: Sized;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn has_read_fault(&self, _offset: u64, _length: u64) -> bool {
        false
    }
}

impl CarveSource for Arc<DiskImage> {
    fn len(&self) -> u64 {
        DiskImage::len(self)
    }

    fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
        DiskImage::read_into(self, offset, destination)
    }

    fn worker_handle(&self) -> Self {
        self.concurrent_reader()
    }

    fn has_read_fault(&self, offset: u64, length: u64) -> bool {
        self.has_bad_region_in_range(offset, length)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FormatKind {
    Mp3,
    Wav,
    Au,
    Wma,
    Jpeg,
    Png,
    Bmp,
    Gif,
    Pcx,
    Tiff,
    Zip,
    Rar,
    Pdf,
    Sqlite,
}

const ALL_FORMATS: [FormatKind; 14] = [
    FormatKind::Mp3,
    FormatKind::Wav,
    FormatKind::Au,
    FormatKind::Wma,
    FormatKind::Jpeg,
    FormatKind::Png,
    FormatKind::Bmp,
    FormatKind::Gif,
    FormatKind::Pcx,
    FormatKind::Tiff,
    FormatKind::Zip,
    FormatKind::Rar,
    FormatKind::Pdf,
    FormatKind::Sqlite,
];

impl FormatKind {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Au => "au",
            Self::Wma => "wma",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Pcx => "pcx",
            Self::Tiff => "tif",
            Self::Zip => "zip",
            Self::Rar => "rar",
            Self::Pdf => "pdf",
            Self::Sqlite => "sqlite",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Wav => "WAV",
            Self::Au => "Sun AU",
            Self::Wma => "WMA",
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Bmp => "BMP",
            Self::Gif => "GIF",
            Self::Pcx => "PCX",
            Self::Tiff => "TIFF",
            Self::Zip => "ZIP / Office",
            Self::Rar => "RAR",
            Self::Pdf => "PDF",
            Self::Sqlite => "SQLite",
        }
    }

    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::Mp3 | Self::Wav | Self::Au | Self::Wma => "audio",
            Self::Jpeg | Self::Png | Self::Bmp | Self::Gif | Self::Pcx | Self::Tiff => "image",
            Self::Zip | Self::Rar => "archive",
            Self::Pdf => "document",
            Self::Sqlite => "database",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Mp3 => "MPEG Audio Layer III audio format",
            Self::Wav => "Waveform Audio File Format",
            Self::Au => "Audio file format developed by Sun Microsystems",
            Self::Wma => "Windows Media Audio Format",
            Self::Jpeg => "Joint Photographic Experts Group Format",
            Self::Png => "Portable Network Graphics Format",
            Self::Bmp => "Bitmap Image File Format",
            Self::Gif => "Graphics Interchange Format",
            Self::Pcx => "Picture Exchange Format",
            Self::Tiff => "Tagged Image File Format",
            Self::Zip => "Archive File Format for Lossless Data Compression",
            Self::Rar => "Rar Archive Format",
            Self::Pdf => "Portable Document Format",
            Self::Sqlite => "SQLite Database Format",
        }
    }

    pub(crate) const fn signatures(self) -> &'static [&'static [u8]] {
        match self {
            Self::Mp3 => &[
                b"\xff\xfa",
                b"\xff\xfb",
                b"\xff\xf2",
                b"\xff\xf3",
                b"\xff\xe2",
                b"\xff\xe3",
                b"ID3",
            ],
            Self::Wav => &[b"RIFF", b"RIFX", b"RF64"],
            Self::Au => &[b".snd"],
            Self::Wma => &[b"\x30\x26\xb2\x75\x8e\x66\xcf\x11\xa6\xd9\x00\xaa\x00\x62\xce\x6c"],
            Self::Jpeg => &[b"\xff\xd8\xff"],
            Self::Png => &[b"\x89PNG\r\n\x1a\n"],
            Self::Bmp => &[b"BM"],
            Self::Gif => &[b"GIF87a", b"GIF89a"],
            Self::Pcx => &[b"\x0a"],
            Self::Tiff => &[b"II\x2a\x00", b"MM\x00\x2a"],
            Self::Zip => &[b"PK\x03\x04", b"PK00PK\x03\x04"],
            Self::Rar => &[b"Rar!\x1a\x07\x00", b"Rar!\x1a\x07\x01\x00"],
            Self::Pdf => &[b"%PDF-"],
            Self::Sqlite => &[b"SQLite format 3\x00"],
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "mp3" => Some(Self::Mp3),
            "wav" => Some(Self::Wav),
            "au" => Some(Self::Au),
            "wma" => Some(Self::Wma),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "bmp" => Some(Self::Bmp),
            "gif" => Some(Self::Gif),
            "pcx" => Some(Self::Pcx),
            "tif" | "tiff" => Some(Self::Tiff),
            "zip" => Some(Self::Zip),
            "rar" => Some(Self::Rar),
            "pdf" => Some(Self::Pdf),
            "sqlite" => Some(Self::Sqlite),
            _ => None,
        }
    }
}

pub fn available_formats() -> Vec<CarveFormatInfo> {
    ALL_FORMATS
        .iter()
        .map(|kind| CarveFormatInfo {
            id: kind.id().to_string(),
            extension: kind.id().to_string(),
            label: kind.label().to_string(),
            category: kind.category().to_string(),
            description: kind.description().to_string(),
            signatures: kind.signatures().iter().map(hex::encode_upper).collect(),
        })
        .collect()
}

pub struct Carver {
    pub(crate) signature_candidates: Vec<Vec<FormatKind>>,
    pub(crate) max_file_size: u64,
    pub(crate) max_results: usize,
    pub(crate) worker_threads: usize,
}

impl Carver {
    pub fn try_new(config: &CarveScanConfig) -> anyhow::Result<Self> {
        let formats = selected_formats(&config.format_ids)?;
        let mut signature_candidates = vec![Vec::new(); 256];
        for format in &formats {
            for signature in format.signatures() {
                let Some(first_byte) = signature.first() else {
                    continue;
                };
                let candidates = &mut signature_candidates[*first_byte as usize];
                if !candidates.contains(format) {
                    candidates.push(*format);
                }
            }
        }
        let worker_threads = config.worker_threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(MAX_AUTO_WORKERS)
        });
        Ok(Self {
            signature_candidates,
            max_file_size: config.max_file_size.clamp(1, MAX_MAX_FILE_SIZE),
            max_results: config.max_results.clamp(1, MAX_RESULTS),
            worker_threads: worker_threads.max(1),
        })
    }

    /// Carves `regions` of `source` on a pool of worker threads. Events reach
    /// `observer` on the calling thread in scan order; while it blocks (for
    /// example a paused scan) workers stop a few windows ahead.
    pub fn scan_regions<S: CarveSource>(
        &self,
        source: &S,
        regions: &[CarveRegion],
        cancellation: &CancellationToken,
        observer: impl FnMut(CarveScanEvent),
    ) -> Result<CarveScanOutput, CarveScanError> {
        self.scan_parallel(source, regions, cancellation, observer)
    }
}

fn selected_formats(ids: &[String]) -> anyhow::Result<Vec<FormatKind>> {
    if ids.is_empty() {
        return Ok(ALL_FORMATS.to_vec());
    }
    let mut selected = BTreeSet::new();
    for id in ids {
        let format = FormatKind::from_id(&id.to_ascii_lowercase())
            .with_context(|| format!("unsupported carve format: {id}"))?;
        selected.insert(format);
    }
    Ok(ALL_FORMATS
        .iter()
        .copied()
        .filter(|format| selected.contains(format))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct SliceSource(Arc<Vec<u8>>);

    impl CarveSource for SliceSource {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }

        fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(self.0.len());
            let length = destination.len().min(self.0.len() - start);
            destination[..length].copy_from_slice(&self.0[start..start + length]);
            Ok(length)
        }

        fn worker_handle(&self) -> Self {
            self.clone()
        }
    }

    #[test]
    fn only_checks_signatures_at_512_byte_boundaries() {
        let mut bytes = vec![0u8; 1024];
        bytes[1..6].copy_from_slice(b"%PDF-");
        bytes[32..37].copy_from_slice(b"%%EOF");
        let carver = Carver::try_new(&CarveScanConfig {
            format_ids: vec!["pdf".to_string()],
            ..CarveScanConfig::default()
        })
        .expect("carver");
        let output = carver
            .scan_regions(
                &SliceSource(Arc::new(bytes)),
                &[CarveRegion {
                    offset: 0,
                    length: 1024,
                    label: "whole".to_string(),
                }],
                &CancellationToken::new(),
                |_| {},
            )
            .expect("scan");
        assert_eq!(output.signatures_found, 0);
        assert_eq!(output.artifacts_found, 0);
    }

    #[test]
    fn rejects_unknown_requested_format() {
        let error = Carver::try_new(&CarveScanConfig {
            format_ids: vec!["nope".to_string()],
            ..CarveScanConfig::default()
        })
        .err()
        .expect("unknown format should fail");
        assert!(error.to_string().contains("unsupported carve format"));
    }

    #[test]
    fn format_catalogue_has_unique_ids() {
        let formats = available_formats();
        let mut ids = formats
            .iter()
            .map(|format| format.id.as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), formats.len());
    }

    #[test]
    fn signature_candidates_preserve_registry_order() {
        let carver = Carver::try_new(&CarveScanConfig::default()).expect("carver");
        assert_eq!(
            carver.signature_candidates[0xff],
            vec![FormatKind::Mp3, FormatKind::Jpeg]
        );
        assert_eq!(
            carver.signature_candidates[b'I' as usize],
            vec![FormatKind::Mp3, FormatKind::Tiff]
        );
        assert!(carver.signature_candidates[0].is_empty());
    }
}
