//! LovelyCarve additions for geometry and acquisition metadata compatibility.
use crate::{EwfError, EwfMetadata, Result};

pub(crate) fn utf16_text(raw: &[u8]) -> String {
    String::from_utf16_lossy(
        &raw.chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .trim_start_matches('\u{feff}')
    .to_owned()
}

pub(crate) fn device_metadata(raw: &[u8], metadata: &mut EwfMetadata) -> Result<()> {
    let text = utf16_text(raw);
    let lines: Vec<_> = text.lines().collect();
    if let (Some(names), Some(values)) = (lines.get(2), lines.get(3)) {
        for (name, value) in names.split('\t').zip(values.split('\t')) {
            if value.is_empty() {
                continue;
            }
            match name {
                "b" => {
                    metadata.bytes_per_sector = Some(
                        value
                            .parse()
                            .map_err(|_| EwfError::Parse("invalid EWF2 sector size".into()))?,
                    )
                }
                "md" => metadata.model = Some(value.into()),
                "sn" => metadata.serial_number = Some(value.into()),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod date_tests {
    use super::*;

    #[test]
    fn ewf2_dates_follow_libewf_at_acquisition_tt_system_mapping() {
        let text = "\u{feff}1\nmain\nat\ttt\n1142163845\t1142163800\n";
        let raw: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut metadata = EwfMetadata::default();
        crate::reader::parse_ewf2_case_data(&raw, &mut metadata);
        assert_eq!(metadata.acquiry_date.as_deref(), Some("1142163845"));
        assert_eq!(metadata.system_date.as_deref(), Some("1142163800"));
    }

    #[test]
    fn header_and_header2_dates_are_exposed_without_rewriting_raw_values() {
        for (acquired, system) in [
            ("2026 9 21 9 13 5", "2026 9 21 9 13 0"),
            ("1142163845", "1142163800"),
            ("invalid", "0"),
        ] {
            let text = format!("1\nmain\nm\tu\n{acquired}\t{system}\n");
            let raw: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            for header in [text, utf16_text(&raw)] {
                let mut metadata = EwfMetadata::default();
                crate::parse::parse_header_text(&header, &mut metadata);
                assert_eq!(metadata.acquiry_date.as_deref(), Some(acquired));
                assert_eq!(metadata.system_date.as_deref(), Some(system));
            }
        }
    }
}
