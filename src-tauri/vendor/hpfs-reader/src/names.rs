use std::collections::HashSet;

use lamfold::{BlockSource, FoldError, Result};

use crate::{Hpfs, SECTOR_SIZE, le16, le32, require};

#[path = "oem_tables.rs"]
mod oem_tables;

impl<S: BlockSource> Hpfs<S> {
    pub(crate) fn read_code_pages(&mut self) -> Result<()> {
        let total = self.header.code_pages;
        require(total <= 256, "HPFS filename code page count")?;
        if total == 0 {
            return Ok(());
        }
        let mut sector = self.header.code_page_directory;
        let mut seen = HashSet::new();
        while sector != 0 {
            require(
                seen.insert(sector),
                "HPFS cyclic filename code page directory",
            )?;
            let directory = self.sector::<SECTOR_SIZE>(sector)?;
            let count = le32(&directory, 4);
            let start = le32(&directory, 8);
            require(
                le32(&directory, 0) == 0x494521f7
                    && (1..=31).contains(&count)
                    && start.checked_add(count).is_some_and(|end| end <= total),
                "HPFS filename code page directory fields",
            )?;
            for index in 0..count {
                let entry = &directory[16 + index as usize * 16..];
                let code_page = le16(entry, 2);
                require(
                    code_page != 0
                        && self
                            .code_pages
                            .insert((start + index) as u16, code_page)
                            .is_none(),
                    "HPFS duplicate or invalid filename code page",
                )?;
            }
            sector = le32(&directory, 12);
        }
        require(
            self.code_pages.len() as u32 == total,
            "HPFS incomplete filename code page directory",
        )
    }

    pub(crate) fn decode_name(&self, raw: &[u8], index: u8) -> Result<String> {
        let name = if raw.is_ascii() {
            String::from_utf8(raw.to_vec()).unwrap()
        } else {
            let code_page = self
                .code_pages
                .get(&u16::from(index))
                .copied()
                .or_else(|| self.code_pages.is_empty().then_some(437))
                .ok_or(FoldError::Corrupt(
                    "HPFS directory filename code page index",
                ))?;
            let encoding = match code_page {
                932 | 943 => Some(encoding_rs::SHIFT_JIS),
                936 | 1381 | 1386 => Some(encoding_rs::GBK),
                949 => Some(encoding_rs::EUC_KR),
                950 => Some(encoding_rs::BIG5),
                _ => None,
            };
            if let Some(encoding) = encoding {
                encoding
                    .decode_without_bom_handling_and_without_replacement(raw)
                    .ok_or(FoldError::Corrupt(
                        "HPFS invalid multibyte directory filename",
                    ))?
                    .into_owned()
            } else {
                let table = oem_tables::get(code_page)
                    .ok_or(FoldError::Unsupported("HPFS filename OEM code page"))?;
                raw.iter()
                    .map(|&byte| match byte {
                        0..=127 => Ok(char::from(byte)),
                        _ => {
                            let value = table[usize::from(byte - 128)];
                            if value == '\0' {
                                Err(FoldError::Corrupt(
                                    "HPFS undefined OEM directory filename byte",
                                ))
                            } else {
                                Ok(value)
                            }
                        }
                    })
                    .collect::<Result<String>>()?
            }
        };
        require(
            !name
                .chars()
                .any(|value| value == '/' || value == '\\' || value.is_control())
                && !matches!(name.as_str(), "." | ".."),
            "HPFS invalid directory filename",
        )?;
        Ok(name)
    }
}
