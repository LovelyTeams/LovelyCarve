//! Checked header/copy selection and transaction-bounded metadata objects.
use crate::{
    read::{le_u16, le_u32, le_u64},
    FileVaultError, VolumeHeader,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
};
const MAX_REGION: u64 = 256 * 1024 * 1024;
pub(crate) fn invalid(what: &'static str) -> FileVaultError {
    FileVaultError::OutOfRange { what }
}
pub(crate) fn checksum(bytes: &[u8]) -> u32 {
    crc::Crc::<u32>::new(&crc::CRC_32_ISCSI).checksum(bytes) ^ u32::MAX
}
pub(crate) fn verify(bytes: &[u8]) -> Result<(), FileVaultError> {
    if bytes.len() < 64
        || le_u32(bytes, 4) != u32::MAX
        || le_u16(bytes, 8) != 1
        || le_u32(bytes, 0) != checksum(&bytes[8..])
    {
        return Err(invalid("metadata version, seed or CRC-32C"));
    }
    Ok(())
}
pub(crate) fn read_region<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    size: u64,
    source_size: u64,
) -> Result<Vec<u8>, FileVaultError> {
    if size == 0
        || size > MAX_REGION
        || offset.checked_add(size).is_none_or(|end| end > source_size)
    {
        return Err(invalid("metadata region outside evidence or memory budget"));
    }
    let mut bytes = vec![0; usize::try_from(size).map_err(|_| invalid("region length"))?];
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
pub(crate) fn read_header<R: Read + Seek>(
    reader: &mut R,
) -> Result<(VolumeHeader, u64), FileVaultError> {
    let source_size = reader.seek(SeekFrom::End(0))?;
    let mut first_error = None;
    for offset in [0, source_size.saturating_sub(512)] {
        let result = (|| {
            let bytes = read_region(reader, offset, 512, source_size)?;
            let header = VolumeHeader::parse(&bytes)?;
            verify(&bytes)?;
            if le_u16(&bytes, 10) != 0x10
                || le_u32(&bytes, 90) != 1
                || le_u32(&bytes, 168) != 16
                || !(1..=4).contains(&le_u16(&bytes, 94))
                || header.bytes_per_sector != 512
                || !header.block_size.is_power_of_two()
                || !(512..=1024 * 1024).contains(&header.block_size)
                || header.physical_volume_size > source_size
                || header.physical_volume_size < 1024
                || !header.physical_volume_size.is_multiple_of(512)
            {
                return Err(invalid("unsupported physical volume header geometry"));
            }
            Ok((header, offset))
        })();
        match result {
            Ok(header) => return Ok(header),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    Err(first_error.unwrap_or_else(|| invalid("physical volume header missing")))
}
#[derive(Clone)]
pub(crate) struct Object {
    pub kind: u16,
    pub identifier: u64,
    pub transaction: u64,
    pub bytes: Vec<u8>,
}
pub(crate) type Objects = BTreeMap<(u16, u64), Vec<Object>>;
pub(crate) fn object(
    objects: &Objects,
    kind: u16,
    identifier: u64,
    transaction: u64,
) -> Result<&Object, FileVaultError> {
    objects
        .get(&(kind, identifier))
        .and_then(|versions| {
            versions
                .iter()
                .filter(|object| object.transaction <= transaction)
                .max_by_key(|object| object.transaction)
        })
        .ok_or_else(|| invalid("referenced metadata object missing at transaction"))
}
fn decrypt_objects(header: &VolumeHeader, ciphertext: &[u8]) -> Result<Objects, FileVaultError> {
    if !ciphertext.len().is_multiple_of(8192) {
        return Err(invalid("encrypted metadata unit alignment"));
    }
    let mut objects = Objects::new();
    for (index, encrypted) in ciphertext.as_chunks::<8192>().0.iter().enumerate() {
        // Unused ciphertext units are zeroed and do not represent an encrypted object.
        if encrypted.iter().all(|byte| *byte == 0) {
            continue;
        }
        let mut bytes = encrypted.to_vec();
        crate::xts::decrypt_units(
            &mut bytes,
            &header.key_data,
            &header.physical_volume_identifier,
            8192,
            index as u128,
        );
        if bytes.starts_with(b"LVFwiped") {
            continue;
        }
        verify(&bytes)?;
        if le_u32(&bytes, 48) != 8192 {
            return Err(invalid("unsupported metadata object size"));
        }
        let object = Object {
            kind: le_u16(&bytes, 10),
            identifier: le_u64(&bytes, 24),
            transaction: le_u64(&bytes, 16),
            bytes,
        };
        let versions = objects.entry((object.kind, object.identifier)).or_default();
        if versions.iter().any(|existing| {
            existing.transaction == object.transaction && existing.bytes != object.bytes
        }) {
            return Err(invalid("conflicting metadata object versions"));
        }
        versions.push(object);
    }
    if objects.is_empty() {
        return Err(invalid("no checksum-validated metadata objects"));
    }
    Ok(objects)
}
pub(crate) fn read_objects<R: Read + Seek>(
    reader: &mut R,
    header: &VolumeHeader,
) -> Result<(Objects, u64, u64, Option<String>), FileVaultError> {
    let block_size = u64::from(header.block_size);
    let mut candidates = Vec::new();
    for block in header.metadata_block_numbers {
        if block == 0 {
            continue;
        }
        let Some(offset) = block.checked_mul(block_size) else {
            continue;
        };
        let result = (|| {
            let first = read_region(reader, offset, 8192, header.physical_volume_size)?;
            verify(&first)?;
            if le_u16(&first, 10) != 0x11 || le_u32(&first, 48) != 8192 {
                return Err(invalid("plaintext metadata pointer"));
            }
            let size = u64::from(le_u32(&first, 64));
            let region = read_region(reader, offset, size, header.physical_volume_size)?;
            let descriptor =
                usize::try_from(le_u32(&first, 220)).map_err(|_| invalid("descriptor offset"))?;
            let descriptor = region
                .get(
                    descriptor
                        ..descriptor
                            .checked_add(48)
                            .ok_or_else(|| invalid("descriptor overflow"))?,
                )
                .ok_or_else(|| invalid("volume group descriptor truncated"))?;
            let size = le_u64(descriptor, 8)
                .checked_mul(block_size)
                .ok_or_else(|| invalid("metadata size overflow"))?;
            if size == 0 || size > MAX_REGION || !size.is_multiple_of(8192) {
                return Err(invalid("metadata region alignment or budget"));
            }
            let mut copies = Vec::new();
            for at in [32, 40] {
                let address = le_u64(descriptor, at);
                if address == 0 {
                    continue;
                }
                if address >> 48 != 0 {
                    return Err(invalid(
                        "metadata on another physical volume is unsupported",
                    ));
                }
                copies.push(
                    address
                        .checked_mul(block_size)
                        .ok_or_else(|| invalid("metadata address overflow"))?,
                );
            }
            let group_xml_size = le_u32(&first, 228) as usize;
            let group_name = if group_xml_size == 0 {
                None
            } else {
                let group_xml_offset = le_u32(&first, 224) as usize;
                let xml = region
                    .get(
                        group_xml_offset
                            ..group_xml_offset
                                .checked_add(group_xml_size)
                                .ok_or_else(|| invalid("volume group plist overflow"))?,
                    )
                    .ok_or_else(|| invalid("volume group plist outside region"))?;
                let group = crate::storage_plist::parse(xml)?;
                if let Some(identifier) = group
                    .get("com.apple.corestorage.lvg.uuid")
                    .and_then(crate::storage_plist::Value::string)
                {
                    if uuid::Uuid::parse_str(identifier)
                        .map_err(|_| invalid("volume group UUID"))?
                        .as_bytes()
                        != &header.logical_volume_group_identifier
                    {
                        return Err(invalid(
                            "metadata group UUID does not match physical header",
                        ));
                    }
                }
                if let Some(members) = group.get("com.apple.corestorage.lvg.physicalVolumes") {
                    if members.array().len() != 1 {
                        return Err(invalid("multiple physical volume members unsupported"));
                    }
                    let identifier = members.array()[0]
                        .string()
                        .ok_or_else(|| invalid("physical volume member UUID"))?;
                    if uuid::Uuid::parse_str(identifier)
                        .map_err(|_| invalid("physical volume member UUID"))?
                        .as_bytes()
                        != &header.physical_volume_identifier
                    {
                        return Err(invalid(
                            "metadata physical member does not match source header",
                        ));
                    }
                }
                group
                    .get("com.apple.corestorage.lvg.name")
                    .and_then(crate::storage_plist::Value::string)
                    .map(str::to_owned)
            };
            Ok((le_u64(&first, 16), offset, size, copies, group_name))
        })();
        if let Ok(candidate) = result {
            candidates.push(candidate);
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    for (_, pointer, size, copies, group_name) in candidates {
        for encrypted_offset in copies {
            let result = read_region(reader, encrypted_offset, size, header.physical_volume_size)
                .and_then(|bytes| decrypt_objects(header, &bytes));
            if let Ok(objects) = result {
                return Ok((objects, pointer, encrypted_offset, group_name));
            }
        }
    }
    Err(invalid(
        "no valid metadata copy (CRC, bounds or unsupported physical members)",
    ))
}
pub(crate) fn plist(
    objects: &Objects,
    object: &Object,
) -> Result<crate::storage_plist::Value, FileVaultError> {
    let (compressed, uncompressed, offset, size, mut continuation) = match object.kind {
        0x19 => (
            le_u32(&object.bytes, 104),
            le_u32(&object.bytes, 108),
            le_u32(&object.bytes, 112),
            le_u32(&object.bytes, 116),
            le_u64(&object.bytes, 96),
        ),
        0x1a => (
            le_u32(&object.bytes, 120),
            le_u32(&object.bytes, 124),
            le_u32(&object.bytes, 128),
            le_u32(&object.bytes, 132),
            0,
        ),
        _ => return Err(invalid("object does not contain a plist")),
    };
    if compressed == 0
        || compressed > 16 * 1024 * 1024
        || uncompressed == 0
        || uncompressed > 16 * 1024 * 1024
    {
        return Err(invalid("plist size budget"));
    }
    let mut data = object
        .bytes
        .get(
            offset as usize
                ..(offset as usize)
                    .checked_add(size as usize)
                    .ok_or_else(|| invalid("plist offset overflow"))?,
        )
        .ok_or_else(|| invalid("plist outside object"))?
        .to_vec();
    let mut visited = std::collections::HashSet::new();
    while data.len() < compressed as usize {
        if continuation == 0 || !visited.insert(continuation) || visited.len() > 4096 {
            return Err(invalid("plist continuation missing or cyclic"));
        }
        let next = self::object(objects, 0x24, continuation, object.transaction)?;
        continuation = le_u64(&next.bytes, 64);
        let length = le_u32(&next.bytes, 72) as usize;
        let bytes = next
            .bytes
            .get(
                80..80usize
                    .checked_add(length)
                    .ok_or_else(|| invalid("continuation length"))?,
            )
            .ok_or_else(|| invalid("plist continuation truncated"))?;
        if data.len() + bytes.len() > compressed as usize {
            return Err(invalid("plist continuation exceeds declared size"));
        }
        data.extend_from_slice(bytes);
    }
    if data.len() != compressed as usize {
        return Err(invalid("plist compressed length mismatch"));
    }
    if compressed != uncompressed {
        let mut decoder = flate2::read::ZlibDecoder::new(data.as_slice());
        let mut output = Vec::new();
        (&mut decoder)
            .take(u64::from(uncompressed) + 1)
            .read_to_end(&mut output)?;
        if output.len() != uncompressed as usize || decoder.total_in() != u64::from(compressed) {
            return Err(invalid("plist deflate length mismatch"));
        }
        data = output;
    }
    crate::storage_plist::parse(&data)
}
