use crate::types::{AcquisitionError, EwfMetadata};
use safe_read::le_u32;

/// Parse the tab-delimited text from an EWF `header` section into metadata fields.
///
/// Format (version 1, ASCII):
/// ```text
/// 1\r\n
/// main\r\n
/// c\tn\ta\te\tt\tav\tov\tm\tu\tp\r\n
/// val\tval\tval\t...\r\n
/// ```
///
/// Field codes: `c=case_number`, `n=evidence_number`, a=description,
/// e=examiner, t=notes, `av=acquiry_software`, `ov=os_version`,
/// `m=acquiry_date`, `u=system_date`, p=password (ignored).
pub(crate) fn parse_header_text(text: &str, meta: &mut EwfMetadata) {
    // Normalize line endings and split into lines
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();

    // Need at least 4 lines: version, "main", field names, field values
    if lines.len() < 4 {
        return;
    }

    let names: Vec<&str> = lines[2].split('\t').collect();
    let values: Vec<&str> = lines[3].split('\t').collect();

    for (i, &name) in names.iter().enumerate() {
        let val = values.get(i).copied().unwrap_or("");
        if val.is_empty() {
            continue;
        }
        let field = match name {
            "c" => &mut meta.case_number,
            "n" => &mut meta.evidence_number,
            "a" => &mut meta.description,
            "e" => &mut meta.examiner,
            "t" => &mut meta.notes,
            "av" => &mut meta.acquiry_software,
            "ov" => &mut meta.os_version,
            "m" => &mut meta.acquiry_date,
            "u" => &mut meta.system_date,
            "md" => &mut meta.model,
            "sn" => &mut meta.serial_number,
            _ => continue,
        };
        *field = Some(val.to_string());
    }
}

/// Parse EWF `error2` section data into acquisition error entries.
///
/// Layout (little-endian, after the 76-byte section descriptor):
/// - `u32` `number_of_entries`
/// - 4 bytes padding
/// - For each entry: `u32` `first_sector` + `u32` `number_of_sectors`
/// - 4 bytes Adler-32 checksum
#[must_use]
pub fn parse_error2_data(data: &[u8]) -> Vec<AcquisitionError> {
    if data.len() < 8 {
        return Vec::new();
    }
    let entry_count = le_u32(data, 0) as usize;
    if entry_count == 0 {
        return Vec::new();
    }
    // Each entry is 8 bytes (u32 first_sector + u32 sector_count), starting at offset 8
    let entries_start = 8;
    // Cap capacity by what the data can actually contain — prevents OOM when entry_count
    // is a large value written by a crafted file but the data buffer is small.
    let max_possible = data.len().saturating_sub(entries_start) / 8;
    let mut errors = Vec::with_capacity(entry_count.min(max_possible));
    for i in 0..entry_count {
        let off = entries_start + i * 8;
        if off + 8 > data.len() {
            break;
        }
        let first_sector = le_u32(data, off);
        let sector_count = le_u32(data, off + 4);
        errors.push(AcquisitionError {
            first_sector,
            sector_count,
        });
    }
    errors
}
