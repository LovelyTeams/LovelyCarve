//! Bounded GPT recovery. A verified copy always wins over a damaged copy.

use anyhow::{ensure, Context};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    GptEntry, GptHeader, Partition, GPT_ENTRY_ARRAY_CAP, GPT_FALLBACK_LBAS, GPT_HEADER_BYTES,
};
use crate::evidence::partitions::SectorSource;

/// Exact header locations and selected entries, with explicit recovery warnings.
/// This analysis can retain more entries than the desktop's u8 partition IDs;
/// `project` refuses to wrap those IDs.
#[derive(Debug, Clone)]
pub struct GptTable {
    pub sector_size: usize,
    pub headers: Vec<GptHeader>,
    pub selected_header: usize,
    pub entries: Vec<GptEntry>,
    pub warnings: Vec<String>,
}

impl GptTable {
    pub fn read(source: &dyn SectorSource, sector_size: usize) -> anyhow::Result<Option<Self>> {
        Self::read_cancellable(source, sector_size, &CancellationToken::new())
    }

    /// Direct token polling between reads and entries; cancellation never
    /// returns a partially parsed table.
    pub fn read_cancellable(
        source: &dyn SectorSource,
        sector_size: usize,
        cancel: &CancellationToken,
    ) -> anyhow::Result<Option<Self>> {
        ensure!(
            matches!(sector_size, 512 | 1024 | 2048 | 4096),
            "unsupported GPT sector size: {sector_size}"
        );
        ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
        let sectors = source.size() / sector_size as u64;
        if sectors < 2 {
            return Ok(None);
        }
        let mut locations = GPT_FALLBACK_LBAS.to_vec();
        locations.push(sectors - 1);
        let mut headers = Vec::new();
        let mut arrays = Vec::new();
        let mut warnings = Vec::new();
        let mut visited = Vec::new();
        // Only the four leading probes may enqueue a backup pointer. This
        // avoids following corrupt pointer chains and bounds total work to 9 arrays.
        let mut location_index = 0;
        while location_index < locations.len() {
            ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
            let lba = locations[location_index];
            location_index += 1;
            if lba >= sectors || visited.contains(&lba) {
                continue;
            }
            visited.push(lba);
            let offset = lba * sector_size as u64;
            let bytes = source
                .read(offset, GPT_HEADER_BYTES)
                .with_context(|| format!("reading GPT header at LBA {lba}"))?;
            if !super::metadata::has_gpt_signature(&bytes) {
                continue;
            }
            ensure!(
                bytes.len() == GPT_HEADER_BYTES,
                "short GPT header read at LBA {lba}"
            );
            let mut header = GptHeader::decode(&bytes, lba).context("decoding GPT header")?;
            if GPT_FALLBACK_LBAS.contains(&lba)
                && header.backup_lba < sectors
                && header.backup_lba != lba
            {
                locations.push(header.backup_lba);
            }
            let array = read_array(
                source,
                sector_size,
                sectors,
                &mut header,
                &mut warnings,
                cancel,
            )?;
            headers.push(header);
            arrays.push(array);
        }
        ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
        let selected = headers
            .iter()
            .position(|header| header.verified)
            .or_else(|| {
                arrays.iter().position(|entries| {
                    entries
                        .as_ref()
                        .is_some_and(|entries| entries.iter().any(|entry| entry.valid))
                })
            });
        let Some(selected_header) = selected else {
            return Ok(None);
        };
        let selected = &headers[selected_header];
        if !selected.verified {
            warnings.push(format!(
                "GPT recovered from unverified header/array at LBA {}",
                selected.read_lba
            ));
        }
        if selected.read_lba != 1 {
            warnings.push(format!(
                "GPT selected alternate header at LBA {} instead of primary LBA 1",
                selected.read_lba
            ));
        }
        let counterpart = headers.iter().find(|header| {
            header.read_lba == selected.backup_lba && header.read_lba != selected.read_lba
        });
        match counterpart {
            Some(other) => {
                if selected.current_lba != other.backup_lba
                    || selected.backup_lba != other.current_lba
                    || selected.disk_guid != other.disk_guid
                    || selected.revision != other.revision
                    || selected.first_usable_lba != other.first_usable_lba
                    || selected.last_usable_lba != other.last_usable_lba
                    || selected.entry_count != other.entry_count
                    || selected.entry_size != other.entry_size
                    || selected.entries_crc32 != other.entries_crc32
                {
                    warnings.push("GPT primary/backup metadata disagree; selected copy preserved without merging".into());
                }
            }
            None => warnings.push("GPT counterpart header missing or undecodable".into()),
        }
        let entries = arrays
            .swap_remove(selected_header)
            .context("selected GPT array missing")?;
        Ok(Some(Self {
            sector_size,
            headers,
            selected_header,
            entries,
            warnings,
        }))
    }

    pub fn header(&self) -> &GptHeader {
        &self.headers[self.selected_header]
    }

    pub fn disk_id(&self) -> Option<String> {
        let guid = self.header().disk_guid;
        (!guid.is_nil()).then(|| guid.to_string().to_uppercase())
    }

    pub fn partition_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.valid).count()
    }

    /// Dense index in the accepted partition list, distinct from the raw slot.
    pub fn partition_by_index(&self, index: usize) -> Option<&GptEntry> {
        self.entries.iter().filter(|entry| entry.valid).nth(index)
    }

    /// libvsgpt's "identifier" lookup refers to the u32 on-disk entry index,
    /// not the unique GUID. GUID lookup is a separate convenience operation.
    pub fn partition_by_entry_index(&self, entry_index: u32) -> Option<&GptEntry> {
        self.entries
            .iter()
            .find(|entry| entry.valid && entry.entry_index == entry_index)
    }

    /// Source slot order, independent of sparse/high raw entry indexes.
    pub fn project(&self) -> anyhow::Result<Vec<Partition>> {
        self.entries
            .iter()
            .filter(|entry| entry.valid)
            .enumerate()
            .map(|(index, entry)| {
                let id = u8::try_from(index).context(
                    "GPT has more than 256 usable partitions; partition IDs cannot wrap",
                )?;
                entry.project(id)
            })
            .collect()
    }

    /// Nil GUIDs do not identify a partition. Duplicate unique GUIDs are
    /// ambiguous and produce an error instead of silently choosing a copy.
    pub fn partition_by_guid(&self, guid: Uuid) -> anyhow::Result<Option<&GptEntry>> {
        if guid.is_nil() {
            return Ok(None);
        }
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.valid && entry.unique_guid == guid);
        let entry = matches.next();
        ensure!(
            matches.next().is_none(),
            "ambiguous duplicate GPT partition GUID: {guid}"
        );
        Ok(entry)
    }
}

/// Only callers without authoritative container/device geometry may infer it.
/// Valid header geometry and both CRCs are required to change the preferred size.
pub fn infer_sector_size(source: &dyn SectorSource, preferred: usize) -> anyhow::Result<usize> {
    ensure!(
        matches!(preferred, 512 | 1024 | 2048 | 4096),
        "unsupported GPT sector size: {preferred}"
    );
    for sector_size in std::iter::once(preferred).chain(
        [512, 1024, 2048, 4096]
            .into_iter()
            .filter(|size| *size != preferred),
    ) {
        if GptTable::read(source, sector_size)?.is_some_and(|table| table.header().verified) {
            return Ok(sector_size);
        }
    }
    Ok(preferred)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut checksum = flate2::Crc::new();
    checksum.update(bytes);
    checksum.sum()
}

fn read_array(
    source: &dyn SectorSource,
    sector_size: usize,
    sectors: u64,
    header: &mut GptHeader,
    warnings: &mut Vec<String>,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<Vec<GptEntry>>> {
    ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
    let lba = header.read_lba;
    // Zero header size survives only as explicitly unverified legacy recovery.
    if header.header_size != 0
        && !(GPT_HEADER_BYTES as u32..=sector_size as u32).contains(&header.header_size)
    {
        warnings.push(format!(
            "GPT header LBA {lba}: invalid header size {}",
            header.header_size
        ));
        return Ok(None);
    }
    let header_length = (header.header_size as usize).max(GPT_HEADER_BYTES);
    let mut header_bytes = source
        .read(lba * sector_size as u64, header_length)
        .with_context(|| format!("reading GPT checksum header at LBA {lba}"))?;
    ensure!(
        header_bytes.len() == header_length,
        "short GPT checksum header at LBA {lba}"
    );
    header_bytes[16..20].fill(0);
    header.header_crc_valid =
        header.header_size != 0 && crc32(&header_bytes) == header.header_crc32;
    if !header.header_crc_valid {
        warnings.push(format!(
            "GPT header LBA {lba}: header CRC32 mismatch or missing"
        ));
    }
    if header.entry_size < 128 || !header.entry_size.is_multiple_of(128) {
        warnings.push(format!(
            "GPT header LBA {lba}: unsupported entry size {}",
            header.entry_size
        ));
        return Ok(None);
    }
    let array_length = u64::from(header.entry_count) * u64::from(header.entry_size);
    if array_length > GPT_ENTRY_ARRAY_CAP as u64 {
        warnings.push(format!(
            "GPT header LBA {lba}: entry array exceeds 1 MiB analysis limit"
        ));
        return Ok(None);
    }
    let array_offset = header.entries_lba.checked_mul(sector_size as u64);
    let array_end = array_offset.and_then(|offset| offset.checked_add(array_length));
    if header.entries_lba == 0 || array_end.is_none_or(|end| end > source.size()) {
        warnings.push(format!(
            "GPT header LBA {lba}: entry array outside evidence"
        ));
        return Ok(None);
    }
    let array_offset = array_offset.context("GPT entry offset overflow")?;
    let array_end = array_end.context("GPT entry end overflow")?;
    let array_last_lba = if array_length == 0 {
        header.entries_lba
    } else {
        (array_end - 1) / sector_size as u64
    };
    if array_length != 0
        && ((header.entries_lba..=array_last_lba).contains(&lba)
            || (header.backup_lba != 0
                && (header.entries_lba..=array_last_lba).contains(&header.backup_lba)))
    {
        warnings.push(format!(
            "GPT header LBA {lba}: entry array overlaps a header"
        ));
        return Ok(None);
    }
    let usable_known = header.first_usable_lba != 0 || header.last_usable_lba != 0;
    if usable_known
        && (header.first_usable_lba > header.last_usable_lba || header.last_usable_lba >= sectors)
    {
        warnings.push(format!("GPT header LBA {lba}: invalid usable LBA range"));
        return Ok(None);
    }
    let geometry_valid = header.current_lba == lba
        && (lba == 1 || header.backup_lba == 1)
        && header.backup_lba != 0
        && header.backup_lba < sectors
        && header.backup_lba != lba
        && header.first_usable_lba > 1
        && usable_known
        && !(header.first_usable_lba..=header.last_usable_lba).contains(&lba)
        && !(header.first_usable_lba..=header.last_usable_lba).contains(&header.backup_lba)
        && (array_length == 0
            || array_last_lba < header.first_usable_lba
            || header.entries_lba > header.last_usable_lba);
    if !geometry_valid || header.revision != 0x0001_0000 || header.reserved != 0 {
        warnings.push(format!(
            "GPT header LBA {lba}: unverified geometry, revision or reserved field"
        ));
    }
    ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
    let bytes = source
        .read(array_offset, array_length as usize)
        .with_context(|| format!("reading GPT entry array for header LBA {lba}"))?;
    ensure!(
        bytes.len() == array_length as usize,
        "short GPT entry array for header LBA {lba}"
    );
    header.entries_crc_valid = crc32(&bytes) == header.entries_crc32;
    if !header.entries_crc_valid {
        warnings.push(format!("GPT header LBA {lba}: entry-array CRC32 mismatch"));
    }
    header.verified = header.header_crc_valid
        && header.entries_crc_valid
        && geometry_valid
        && header.revision == 0x0001_0000
        && header.reserved == 0;
    let mut entries: Vec<GptEntry> = Vec::new();
    let mut identifiers = std::collections::HashSet::new();
    for (index, bytes) in bytes.chunks_exact(header.entry_size as usize).enumerate() {
        ensure!(!cancel.is_cancelled(), "GPT analysis cancelled");
        let entry_offset = array_offset + index as u64 * u64::from(header.entry_size);
        let mut entry =
            GptEntry::decode(bytes, index as u32, entry_offset).context("decoding GPT entry")?;
        if entry.type_guid.is_nil() {
            continue;
        }
        entry.valid = entry.first_lba != 0
            && entry.last_lba >= entry.first_lba
            && entry.last_lba < sectors
            && (!usable_known
                || (entry.first_lba >= header.first_usable_lba
                    && entry.last_lba <= header.last_usable_lba))
            && !(entry.first_lba..=entry.last_lba).contains(&lba)
            && (header.backup_lba == 0
                || !(entry.first_lba..=entry.last_lba).contains(&header.backup_lba))
            && (array_length == 0
                || entry.last_lba < header.entries_lba
                || entry.first_lba > array_last_lba);
        if !entry.valid {
            warnings.push(format!(
                "GPT header LBA {lba} slot {index}: invalid complete partition range"
            ));
        }
        if entry.name().is_err() {
            warnings.push(format!(
                "GPT header LBA {lba} slot {index}: invalid UTF-16 name preserved"
            ));
        }
        if entry.valid && !entry.unique_guid.is_nil() && !identifiers.insert(entry.unique_guid) {
            warnings.push(format!(
                "GPT header LBA {lba} slot {index}: duplicate unique partition GUID"
            ));
        }
        if entry.valid
            && entries.iter().any(|other| {
                other.valid
                    && entry.first_lba <= other.last_lba
                    && other.first_lba <= entry.last_lba
            })
        {
            warnings.push(format!(
                "GPT header LBA {lba} slot {index}: overlapping partition ranges preserved"
            ));
        }
        entries.push(entry);
    }
    Ok(Some(entries))
}

#[cfg(test)]
pub(super) mod tests;
