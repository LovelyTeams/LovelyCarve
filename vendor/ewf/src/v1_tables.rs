//! Locate the data span belonging to each v1 table. Early EnCase and SMART
//! place chunks in the table itself; later writers use a preceding sectors span.
use crate::{EwfError, Result, SectionDescriptor, SECTION_DESCRIPTOR_SIZE};

pub(crate) fn data_end(
    table: &SectionDescriptor,
    descriptors: &[SectionDescriptor],
    file_len: u64,
) -> Result<Option<u64>> {
    if table.section_type != "table" && table.section_type != "table2" {
        return Ok(None);
    }
    let table_end = table
        .offset
        .checked_add(table.section_size)
        .ok_or_else(|| EwfError::Parse("EWF table section offset overflow".into()))?;
    if let Some(sectors) = descriptors
        .iter()
        .rev()
        .find(|descriptor| descriptor.section_type == "sectors" && descriptor.offset < table.offset)
    {
        let end = sectors
            .offset
            .checked_add(sectors.section_size)
            .ok_or_else(|| EwfError::Parse("EWF sectors section offset overflow".into()))?;
        if end > table.offset || end > file_len {
            return Err(EwfError::Parse(
                "EWF sectors span crosses its table or file end".into(),
            ));
        }
        Ok(Some(end))
    } else {
        if table.section_size < SECTION_DESCRIPTOR_SIZE as u64 || table_end > file_len {
            return Err(EwfError::Parse(
                "EWF embedded table span exceeds file".into(),
            ));
        }
        Ok(Some(table_end))
    }
}
