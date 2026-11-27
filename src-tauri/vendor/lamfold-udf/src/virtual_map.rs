//! Current Virtual Allocation Table for sequentially recorded cooked UDF images.
use crate::{descriptors::*, udf::Udf};
use alloc::vec::Vec;
use lamfold::{BlockSource, FoldError, Result};

pub(crate) struct VirtualMap {
    entries: Vec<u32>,
}

impl VirtualMap {
    pub(crate) fn span(&self, position: u64, block_size: u32) -> Result<(u64, u64)> {
        let block_size = u64::from(block_size);
        let block = usize::try_from(position / block_size)
            .map_err(|_| FoldError::Corrupt("udf: VAT block outside address space"))?;
        let translated = self
            .entries
            .get(block)
            .copied()
            .filter(|&address| address != u32::MAX)
            .ok_or(FoldError::Corrupt("udf: missing VAT block mapping"))?;
        let within = position % block_size;
        Ok((
            u64::from(translated) * block_size + within,
            block_size - within,
        ))
    }
}

impl<S: BlockSource> Udf<S> {
    pub(crate) fn read_virtual_map(&mut self, revision: u16) -> Result<VirtualMap> {
        // The newest transaction ends with the VAT entry. Look only near the
        // recorded physical partition end, without following historical VATs.
        for distance in 0..4 {
            let Some(block) = self.partition_length.checked_sub(1 + distance) else {
                break;
            };
            if let Some(bytes) = self.vat_file_bytes(block, revision)? {
                return self.parse_virtual_table(&bytes, revision, block);
            }
        }
        Err(FoldError::Unsupported(
            "udf: VAT ICB is not near the recorded partition end",
        ))
    }

    fn parse_virtual_table(
        &self,
        bytes: &[u8],
        revision: u16,
        vat_block: u32,
    ) -> Result<VirtualMap> {
        let table = if revision == 0x0150 {
            let end = bytes
                .len()
                .checked_sub(36)
                .ok_or(FoldError::Corrupt("udf: short UDF 1.50 VAT"))?;
            require(
                bytes.get(end..end + 24) == Some(b"\0*UDF Virtual Alloc Tbl\0"),
                "udf: invalid UDF 1.50 VAT identifier",
            )?;
            &bytes[..end]
        } else {
            let header = usize::from(u16_at(bytes, 0)?);
            let implementation = usize::from(u16_at(bytes, 2)?);
            require(
                header == 152 + implementation
                    && u16_at(bytes, 144)? <= 0x0260
                    && u16_at(bytes, 150)? == 0,
                "udf: invalid UDF 2.x VAT header",
            )?;
            bytes
                .get(header..)
                .ok_or(FoldError::Corrupt("udf: truncated VAT header"))?
        };
        require(
            !table.is_empty() && table.len().is_multiple_of(4),
            "udf: invalid VAT entry array length",
        )?;
        let mut entries = Vec::with_capacity(table.len() / 4);
        for entry in table.as_chunks::<4>().0 {
            let address = u32_at(entry, 0)?;
            require(
                address == u32::MAX || address < vat_block,
                "udf: VAT entry outside preceding physical recording",
            )?;
            entries.push(address);
        }
        Ok(VirtualMap { entries })
    }
}
