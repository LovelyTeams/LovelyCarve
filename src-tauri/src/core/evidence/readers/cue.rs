//! CUE-described read-only optical evidence. The source byte stream is the
//! exact concatenation of FILE members; INDEX positions are file-relative.
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::optical_disc::DataTrackLayout;
use crate::core::evidence::partitions::{Partition, PartitionKind, PartitionSlice, SectorSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueIndex {
    pub number: u8,
    pub file_sector: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueTrack {
    pub number: u8,
    pub session: u8,
    pub mode: String,
    pub file_index: usize,
    pub sector_bytes: usize,
    pub indexes: Vec<CueIndex>,
    pub pregap_sectors: u64,
    pub postgap_sectors: u64,
    pub run_out_sector: Option<u64>,
    /// Offset and length in the concatenated physical FILE byte stream.
    pub source_offset: u64,
    pub source_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueLeadOut {
    pub session: u8,
    pub file_index: usize,
    pub file_sector: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueInfo {
    pub files: Vec<String>,
    pub tracks: Vec<CueTrack>,
    pub lead_outs: Vec<CueLeadOut>,
}

#[derive(Debug)]
pub struct CueImage {
    pub reader: smraw::RawImage,
    pub descriptor: smraw::Segment,
    pub info: CueInfo,
}

impl CueImage {
    pub fn open(path: &Path, cancel: &CancellationToken) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open CUE {}", path.display()))?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 1024 * 1024,
            "CUE exceeds 1 MiB limit"
        );
        let descriptor = smraw::Segment {
            path: std::fs::canonicalize(path)?,
            offset: 0,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == descriptor.len,
            "CUE changed while opening"
        );
        let text =
            std::str::from_utf8(&bytes).context("CUE must be UTF-8 (optional BOM) or ASCII")?;
        let (paths, mut info) = parse(text.trim_start_matches('\u{feff}'), path, cancel)?;
        let reader = smraw::RawImage::open_segments(paths).context("open CUE data files")?;
        for (file_index, segment) in reader.segments().iter().enumerate() {
            ensure!(!cancel.is_cancelled(), "CUE load cancelled");
            let tracks: Vec<_> = info
                .tracks
                .iter()
                .filter(|track| track.file_index == file_index)
                .collect();
            ensure!(!tracks.is_empty(), "CUE FILE has no TRACK");
            let width = tracks[0].sector_bytes;
            ensure!(
                tracks.iter().all(|track| track.sector_bytes == width),
                "CUE tracks with different sector sizes in one FILE are unsupported"
            );
            ensure!(
                segment.len > 0 && segment.len.is_multiple_of(width as u64),
                "truncated CUE data file"
            );
            let sectors = segment.len / width as u64;
            let mut previous_end = 0;
            for (index, track) in tracks.iter().enumerate() {
                let start = track
                    .indexes
                    .iter()
                    .find(|index| index.number == 1)
                    .context("CUE TRACK requires INDEX 01")?
                    .file_sector;
                let first = track
                    .indexes
                    .first()
                    .context("CUE TRACK has no indexes")?
                    .file_sector;
                let next = tracks
                    .get(index + 1)
                    .and_then(|track| track.indexes.first())
                    .map_or(sectors, |index| index.file_sector);
                ensure!(
                    track
                        .run_out_sector
                        .is_none_or(|run_out| run_out > start && run_out <= next),
                    "CUE run-out exceeds track or data file"
                );
                let end = info
                    .lead_outs
                    .iter()
                    .filter(|lead| lead.file_index == file_index && lead.session == track.session)
                    .map(|lead| lead.file_sector)
                    .min()
                    .map_or(next, |lead| next.min(lead));
                let end = track.run_out_sector.map_or(end, |run_out| end.min(run_out));
                ensure!(
                    first >= previous_end && start < end && end <= sectors,
                    "CUE track indexes overlap or exceed data file"
                );
                ensure!(
                    track.indexes.iter().all(|index| index.file_sector < end),
                    "CUE index exceeds track"
                );
                previous_end = end;
            }
            for lead in info
                .lead_outs
                .iter()
                .filter(|lead| lead.file_index == file_index)
            {
                ensure!(
                    lead.file_sector <= sectors,
                    "CUE lead-out exceeds data file"
                );
            }
        }
        for index in 0..info.tracks.len() {
            let track = &info.tracks[index];
            let segment = &reader.segments()[track.file_index];
            let start = track
                .indexes
                .iter()
                .find(|index| index.number == 1)
                .context("missing INDEX 01")?
                .file_sector;
            let next = info
                .tracks
                .get(index + 1)
                .filter(|next| next.file_index == track.file_index)
                .and_then(|next| next.indexes.first())
                .map_or(segment.len / track.sector_bytes as u64, |index| {
                    index.file_sector
                });
            let end = info
                .lead_outs
                .iter()
                .filter(|lead| lead.file_index == track.file_index && lead.session == track.session)
                .map(|lead| lead.file_sector)
                .min()
                .map_or(next, |lead| next.min(lead));
            let end = track.run_out_sector.map_or(end, |run_out| end.min(run_out));
            let width = track.sector_bytes as u64;
            let offset = segment
                .offset
                .checked_add(start.checked_mul(width).context("CUE offset overflow")?)
                .context("CUE offset overflow")?;
            let length = (end - start)
                .checked_mul(width)
                .context("CUE length overflow")?;
            info.tracks[index].source_offset = offset;
            info.tracks[index].source_length = length;
        }
        info.files = reader
            .segments()
            .iter()
            .map(|file| file.path.display().to_string())
            .collect();
        Ok(Self {
            reader,
            descriptor,
            info,
        })
    }

    pub fn partitions(&self, cancel: &CancellationToken) -> Result<Vec<Partition>> {
        self.info
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                ensure!(!cancel.is_cancelled(), "CUE load cancelled");
                let source = PartitionSlice::new(self, track.source_offset, track.source_length);
                let cdfs = match track.mode.as_str() {
                    "MODE1/2048" | "MODE2/2048" => {
                        DataTrackLayout::probe_cooked(&source)?.is_some()
                    }
                    "MODE1/2352" | "MODE2/2352" => {
                        let layout = DataTrackLayout::probe(&source)?;
                        if let Some(layout) = layout {
                            ensure!(
                                layout.mode() == if track.mode == "MODE1/2352" { 1 } else { 2 },
                                "CUE track mode disagrees with sector header"
                            );
                        }
                        layout.is_some()
                    }
                    "MODE2/2336" | "CDI/2336" => DataTrackLayout::probe_xa2336(&source)?.is_some(),
                    "AUDIO" | "CDG" | "MODE2/2324" | "CDI/2352" => false,
                    mode => anyhow::bail!("unsupported CUE track mode {mode}"),
                };
                Ok(Partition {
                    id: u8::try_from(index).context("too many CUE tracks")?,
                    kind: if cdfs {
                        PartitionKind::Cdfs
                    } else {
                        PartitionKind::Unknown
                    },
                    start_lba: track.source_offset,
                    length_lba: track.source_length,
                    label: Some(format!(
                        "Session {} / Track {:02} ({})",
                        track.session, track.number, track.mode
                    )),
                    unique_guid: None,
                })
            })
            .collect()
    }
}

impl SectorSource for CueImage {
    fn size(&self) -> u64 {
        self.reader.media_size()
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let length = (length as u64).min(self.size().saturating_sub(offset)) as usize;
        let mut output = vec![0; length];
        ensure!(
            self.reader.read_at(offset, &mut output)? == length,
            "CUE source short read"
        );
        Ok(output)
    }
}

fn parse(text: &str, path: &Path, cancel: &CancellationToken) -> Result<(Vec<PathBuf>, CueInfo)> {
    let mut paths = Vec::new();
    let mut info = CueInfo {
        files: Vec::new(),
        tracks: Vec::new(),
        lead_outs: Vec::new(),
    };
    let mut session = 1;
    for (line_number, line) in text.lines().enumerate() {
        ensure!(!cancel.is_cancelled(), "CUE load cancelled");
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let result: Result<()> = (|| {
            match keyword {
                "FILE" => {
                    ensure!(paths.len() < 99, "too many CUE files");
                    let (name, file_type) = if let Some(quoted) = rest.strip_prefix('"') {
                        let (name, remainder) = quoted
                            .split_once('"')
                            .context("unterminated CUE filename")?;
                        (name, remainder.trim())
                    } else {
                        rest.rsplit_once(char::is_whitespace)
                            .context("CUE FILE requires name and type")?
                    };
                    ensure!(
                        file_type == "BINARY",
                        "unsupported CUE FILE type {file_type}; expected BINARY"
                    );
                    ensure!(
                        !name.is_empty() && !name.contains('\0'),
                        "empty or invalid CUE filename"
                    );
                    paths.push(
                        path.parent()
                            .unwrap_or(Path::new("."))
                            .join(name.replace('\\', "/")),
                    );
                }
                "TRACK" => {
                    ensure!(
                        !paths.is_empty() && info.tracks.len() < 99,
                        "CUE TRACK requires FILE; maximum 99 tracks"
                    );
                    let fields: Vec<_> = rest.split_whitespace().collect();
                    ensure!(fields.len() == 2, "invalid CUE TRACK");
                    let number = decimal(fields[0], 99)? as u8;
                    ensure!(
                        number > 0 && info.tracks.last().is_none_or(|track| track.number < number),
                        "CUE track numbers must increase"
                    );
                    let mode = match fields[1] {
                        "MODE1_RAW" => "MODE1/2352",
                        "MODE2_RAW" => "MODE2/2352",
                        other => other,
                    };
                    let sector_bytes = match mode {
                        "MODE1/2048" | "MODE2/2048" => 2048,
                        "MODE2/2324" => 2324,
                        "MODE2/2336" | "CDI/2336" => 2336,
                        "AUDIO" | "MODE1/2352" | "MODE2/2352" | "CDI/2352" => 2352,
                        "CDG" => 2448,
                        _ => anyhow::bail!("unsupported CUE mode {mode}"),
                    };
                    info.tracks.push(CueTrack {
                        number,
                        session,
                        mode: mode.into(),
                        file_index: paths.len() - 1,
                        sector_bytes,
                        indexes: Vec::new(),
                        pregap_sectors: 0,
                        postgap_sectors: 0,
                        run_out_sector: None,
                        source_offset: 0,
                        source_length: 0,
                    });
                }
                "INDEX" => {
                    let fields: Vec<_> = rest.split_whitespace().collect();
                    ensure!(fields.len() == 2, "invalid CUE INDEX");
                    let number = decimal(fields[0], 99)? as u8;
                    let file_sector = msf(fields[1])?;
                    let track = info.tracks.last_mut().context("INDEX without TRACK")?;
                    ensure!(
                        track.file_index + 1 == paths.len(),
                        "INDEX without TRACK in current FILE"
                    );
                    ensure!(
                        track.indexes.last().is_none_or(
                            |index| index.number < number && index.file_sector <= file_sector
                        ),
                        "CUE indexes must increase"
                    );
                    track.indexes.push(CueIndex {
                        number,
                        file_sector,
                    });
                }
                "PREGAP" | "POSTGAP" => {
                    let track = info.tracks.last_mut().context("gap without TRACK")?;
                    ensure!(
                        track.file_index + 1 == paths.len(),
                        "gap without current TRACK"
                    );
                    let value = msf(rest)?;
                    let gap = if keyword == "PREGAP" {
                        &mut track.pregap_sectors
                    } else {
                        &mut track.postgap_sectors
                    };
                    ensure!(*gap == 0, "duplicate CUE gap");
                    *gap = value;
                }
                "REM" => {
                    let (kind, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    match kind {
                        "SESSION" => {
                            let next = decimal(value.trim(), 99)? as u8;
                            ensure!(next > 0 && next >= session, "CUE sessions must increase");
                            session = next;
                        }
                        "LEAD-OUT" => {
                            ensure!(!paths.is_empty(), "lead-out without FILE");
                            ensure!(
                                info.tracks
                                    .last()
                                    .is_some_and(|track| track.session == session
                                        && track.file_index + 1 == paths.len()),
                                "lead-out without current TRACK"
                            );
                            ensure!(
                                !info.lead_outs.iter().any(|lead| lead.session == session),
                                "duplicate session lead-out"
                            );
                            info.lead_outs.push(CueLeadOut {
                                session,
                                file_index: paths.len() - 1,
                                file_sector: msf(value.trim())?,
                            });
                        }
                        "RUN-OUT" => {
                            let track = info.tracks.last_mut().context("run-out without TRACK")?;
                            ensure!(
                                track.session == session && track.file_index + 1 == paths.len(),
                                "run-out without current TRACK"
                            );
                            ensure!(track.run_out_sector.is_none(), "duplicate track run-out");
                            track.run_out_sector = Some(msf(value.trim())?);
                        }
                        _ => {}
                    }
                }
                // Display annotations do not change byte geometry.
                "CATALOG" | "CDTEXTFILE" | "TITLE" | "PERFORMER" | "SONGWRITER" | "ISRC"
                | "FLAGS" => {}
                _ => anyhow::bail!("unsupported CUE directive {keyword}"),
            }
            Ok(())
        })();
        result.with_context(|| format!("CUE line {}", line_number + 1))?;
    }
    ensure!(!info.tracks.is_empty(), "CUE has no tracks");
    Ok((paths, info))
}

fn decimal(value: &str, maximum: u64) -> Result<u64> {
    ensure!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid CUE decimal"
    );
    let number: u64 = value.parse()?;
    ensure!(number <= maximum, "CUE numeric value out of range");
    Ok(number)
}

fn msf(value: &str) -> Result<u64> {
    let fields: Vec<_> = value.split(':').collect();
    ensure!(
        fields.len() == 3 && fields.iter().all(|field| field.len() == 2),
        "invalid CUE MM:SS:FF"
    );
    Ok((decimal(fields[0], 99)? * 60 + decimal(fields[1], 59)?) * 75 + decimal(fields[2], 74)?)
}

#[cfg(test)]
#[path = "cue_tests.rs"]
mod tests;
