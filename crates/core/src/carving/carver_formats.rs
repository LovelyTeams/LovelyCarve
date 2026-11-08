//! Dispatcher for the Digler-compatible format scanners.

use tokio_util::sync::CancellationToken;

use crate::carving::carver::{CarveScanError, CarveSource, FormatKind};
use crate::carving::carver_reader::CarveReader;
use {
    crate::carving::formats::carver_format_archive, crate::carving::formats::carver_format_audio,
    crate::carving::formats::carver_format_bmp, crate::carving::formats::carver_format_document,
    crate::carving::formats::carver_format_gif_pcx,
    crate::carving::formats::carver_format_jpeg_png, crate::carving::formats::carver_format_tiff,
    crate::carving::formats::carver_format_zip,
};

pub(crate) struct FormatScanResult {
    pub size: u64,
    pub extension: Option<&'static str>,
    pub name: Option<String>,
}

impl FormatScanResult {
    pub(crate) const fn sized(size: u64) -> Self {
        Self {
            size,
            extension: None,
            name: None,
        }
    }

    pub(crate) const fn with_extension(size: u64, extension: &'static str) -> Self {
        Self {
            size,
            extension: Some(extension),
            name: None,
        }
    }
}

pub(crate) struct DetectedArtifact {
    pub length: u64,
    pub extension: String,
    pub name: Option<String>,
}

/// Where a candidate signature sits: the rest of its scan window plus the
/// evidence bounds a decoder may read up to.
pub(crate) struct Candidate<'a> {
    pub offset: u64,
    pub window: &'a [u8],
    pub region_end: u64,
    pub max_file_size: u64,
}

pub(crate) fn identify<S: CarveSource>(
    format: FormatKind,
    source: &S,
    candidate: &Candidate<'_>,
    lookahead: &mut Vec<u8>,
    cancellation: &CancellationToken,
) -> Result<Option<DetectedArtifact>, CarveScanError> {
    let size = candidate
        .region_end
        .saturating_sub(candidate.offset)
        .min(candidate.max_file_size);
    let mut reader = CarveReader::new(
        source,
        candidate.offset,
        size,
        candidate.window,
        lookahead,
        cancellation,
    );
    let result = match format {
        FormatKind::Mp3 => carver_format_audio::scan_mp3(&mut reader),
        FormatKind::Wav => carver_format_audio::scan_wav(&mut reader),
        FormatKind::Au => carver_format_audio::scan_au(&mut reader),
        FormatKind::Wma => carver_format_audio::scan_wma(&mut reader),
        FormatKind::Jpeg => carver_format_jpeg_png::scan_jpeg(&mut reader),
        FormatKind::Png => carver_format_jpeg_png::scan_png(&mut reader),
        FormatKind::Bmp => carver_format_bmp::scan_bmp(&mut reader),
        FormatKind::Gif => carver_format_gif_pcx::scan_gif(&mut reader),
        FormatKind::Pcx => carver_format_gif_pcx::scan_pcx(&mut reader),
        FormatKind::Tiff => carver_format_tiff::scan_tiff(&mut reader),
        FormatKind::Zip => carver_format_zip::scan_zip(&mut reader),
        FormatKind::Rar => carver_format_archive::scan_rar(&mut reader),
        FormatKind::Pdf => carver_format_document::scan_pdf(&mut reader),
        FormatKind::Sqlite => carver_format_document::scan_sqlite(&mut reader),
    };
    let result = match result {
        Ok(result) => result,
        Err(error)
            if matches!(
                error.downcast_ref::<CarveScanError>(),
                Some(CarveScanError::Cancelled)
            ) =>
        {
            return Err(CarveScanError::Cancelled);
        }
        Err(error) => return Err(CarveScanError::Failed(error)),
    };
    let Some(result) = result else {
        return Ok(None);
    };
    if result.size == 0 {
        return Ok(None);
    }
    // Header-declared sizes (BMP, WMA, SQLite...) never reach past the region
    // end or the configured per-file limit.
    Ok(Some(DetectedArtifact {
        length: result.size.min(size),
        extension: result.extension.unwrap_or(format.id()).to_string(),
        name: result.name,
    }))
}
