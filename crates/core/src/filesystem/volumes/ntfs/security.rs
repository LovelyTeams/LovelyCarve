//! Resolve legacy security attributes or modern $Secure:$SII / $SDS descriptors.
use super::*;
use anyhow::{ensure, Context};
use ntfs::{
    indexes::{NtfsIndexEntryData, NtfsIndexEntryHasData, NtfsIndexEntryKey, NtfsIndexEntryType},
    types::NtfsPosition,
    NtfsIndex,
};

#[derive(Clone, Debug)]
struct SecurityIndex;
#[derive(Debug)]
struct SecurityId(u32);
#[derive(Debug, PartialEq, Eq)]
struct SecurityLocation {
    hash: u32,
    id: u32,
    offset: u64,
    size: u32,
}

impl NtfsIndexEntryType for SecurityIndex {
    type KeyType = SecurityId;
}
impl NtfsIndexEntryHasData for SecurityIndex {
    type DataType = SecurityLocation;
}
impl NtfsIndexEntryKey for SecurityId {
    fn key_from_slice(bytes: &[u8], _position: NtfsPosition) -> ntfs::Result<Self> {
        let Some(bytes) = bytes.first_chunk::<4>() else {
            return Err(ntfs::NtfsError::BufferTooSmall {
                expected: 4,
                actual: bytes.len(),
            });
        };
        Ok(Self(u32::from_le_bytes(*bytes)))
    }
}
impl NtfsIndexEntryData for SecurityLocation {
    fn data_from_slice(bytes: &[u8], _position: NtfsPosition) -> ntfs::Result<Self> {
        let Some(bytes) = bytes.first_chunk::<20>() else {
            return Err(ntfs::NtfsError::BufferTooSmall {
                expected: 20,
                actual: bytes.len(),
            });
        };
        Ok(Self {
            hash: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            id: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            offset: u64::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ]),
            size: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        })
    }
}

impl NtfsVolume<'_> {
    /// Return a self-relative Windows security descriptor; this does not enforce host permissions.
    pub fn security_descriptor(&self, record: u64, max_bytes: usize) -> Result<Option<Vec<u8>>> {
        let mut fs = self.reader();
        let file = self.ntfs.file(&mut fs, record)?;
        let mut attributes = file.attributes();
        while let Some(item) = attributes.next(&mut fs) {
            let item = item?;
            let attribute = item.to_attribute()?;
            if attribute.raw_type() != NtfsAttributeType::SecurityDescriptor as u32 {
                continue;
            }
            ensure!(
                attribute.value_length() <= max_bytes as u64,
                "NTFS security descriptor exceeds caller budget"
            );
            let mut bytes = vec![0; attribute.value_length() as usize];
            attribute.value(&mut fs)?.read_exact(&mut fs, &mut bytes)?;
            validate_descriptor(&bytes)?;
            return Ok(Some(bytes));
        }
        let info = match file.info() {
            Ok(info) => info,
            Err(ntfs::NtfsError::AttributeNotFound { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(id) = info.security_id().filter(|id| *id != 0) else {
            return Ok(None);
        };
        let secure = self.ntfs.file(&mut fs, 9)?;
        let mut root = None;
        let mut allocation = None;
        let mut attributes = secure.attributes();
        while let Some(item) = attributes.next(&mut fs) {
            let item = item?;
            let attribute = item.to_attribute()?;
            if attribute.name()? != "$SII" {
                continue;
            }
            match attribute.ty()? {
                NtfsAttributeType::IndexRoot => root = Some(item),
                NtfsAttributeType::IndexAllocation => allocation = Some(item),
                _ => {}
            }
        }
        let index = NtfsIndex::<SecurityIndex>::new(
            root.context("$Secure is missing $SII index")?,
            allocation,
        )?;
        let mut finder = index.finder();
        let entry = finder
            .find(&mut fs, |key| id.cmp(&key.0))
            .context("NTFS security ID is absent from $SII")??;
        let location = entry
            .data()
            .context("NTFS $SII entry has no descriptor location")??;
        ensure!(
            location.id == id
                && location.size >= 40
                && u64::from(location.size - 20) <= max_bytes as u64,
            "invalid NTFS security descriptor location or size"
        );
        let item = secure
            .data(&mut fs, "$SDS")
            .context("$Secure is missing $SDS stream")??;
        let attribute = item.to_attribute()?;
        ensure!(
            location
                .offset
                .checked_add(u64::from(location.size))
                .is_some_and(|end| end <= attribute.value_length()),
            "security descriptor outside $SDS"
        );
        let mut value =
            super::data_stream::DataStream::new(&attribute, &mut fs, self.ntfs.cluster_size(), 9)?;
        value.seek(&mut fs, SeekFrom::Start(location.offset))?;
        let mut bytes = vec![0; location.size as usize];
        let mut filled = 0;
        while filled < bytes.len() {
            let read = value.read(&mut fs, &mut bytes[filled..])?;
            ensure!(read > 0, "truncated NTFS security descriptor");
            filled += read;
        }
        ensure!(
            SecurityLocation::data_from_slice(&bytes, attribute.position())? == location,
            "$SII and $SDS descriptor headers differ"
        );
        let descriptor = bytes.split_off(20);
        validate_descriptor(&descriptor)?;
        Ok(Some(descriptor))
    }
}

fn validate_descriptor(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() >= 20 && bytes[0] == 1 && bytes[3] & 0x80 != 0,
        "invalid self-relative NTFS security descriptor"
    );
    for offset in [4, 8, 12, 16] {
        let relative = u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) as usize;
        if relative == 0 {
            continue;
        }
        ensure!(
            relative >= 20 && relative < bytes.len(),
            "NTFS security descriptor component outside buffer"
        );
        let component = &bytes[relative..];
        let length = if offset < 12 {
            ensure!(
                component.len() >= 8 && component[0] == 1 && component[1] <= 15,
                "invalid NTFS security SID"
            );
            8 + usize::from(component[1]) * 4
        } else {
            ensure!(component.len() >= 8, "truncated NTFS ACL");
            let size = usize::from(u16::from_le_bytes([component[2], component[3]]));
            ensure!(size >= 8, "invalid NTFS ACL size");
            size
        };
        ensure!(
            length <= component.len(),
            "truncated NTFS security component"
        );
    }
    Ok(())
}
