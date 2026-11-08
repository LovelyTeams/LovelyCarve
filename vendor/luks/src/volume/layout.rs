//! Header loading and validated plaintext payload construction.

use super::{split_encryption, DecryptedPayload, VolumeInfo};
use crate::header::{Luks1Header, LUKS1_PHDR_LEN, SECTOR};
use crate::header2::{Luks2Header, LUKS2_BIN_HDR_LEN};
use crate::sector_cipher::SectorCipher;
use crate::Result;
use std::io::{Read, Seek, SeekFrom};
use zeroize::Zeroizing;

pub(crate) fn read_header2<R: Read + Seek>(reader: &mut R) -> Result<(Luks2Header, String)> {
    let mut binary = [0; LUKS2_BIN_HDR_LEN];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut binary)?;
    let declared = crate::bytes::be_u64(&binary, 8);
    let total = reader.seek(SeekFrom::End(0))?;
    if !(8192..=4 * 1024 * 1024).contains(&declared)
        || !declared.is_power_of_two()
        || declared > total
    {
        return Err(std::io::Error::other("invalid LUKS2 header size").into());
    }
    let mut full = vec![0; declared as usize];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut full)?;
    Ok((
        Luks2Header::parse(&full)?,
        crate::bytes::cstr(&binary, 168, 40),
    ))
}

pub(crate) fn payload2<R: Read + Seek>(
    mut reader: R,
    segment: &crate::Luks2Segment,
    uuid: String,
    master_key: Vec<u8>,
) -> Result<DecryptedPayload<R>> {
    let total_size = reader.seek(SeekFrom::End(0))?;
    if !matches!(segment.sector_size, 512 | 4096)
        || segment.offset >= total_size
        || segment.offset % segment.sector_size as u64 != 0
        || (total_size - segment.offset) % segment.sector_size as u64 != 0
    {
        return Err(std::io::Error::other("invalid LUKS2 payload geometry").into());
    }
    let (cipher_name, cipher_mode) = split_encryption(&segment.encryption);
    let key_bytes = master_key.len() as u32;
    Ok(DecryptedPayload {
        cipher: SectorCipher::new(&cipher_name, &cipher_mode, &master_key)?,
        reader,
        master_key: Zeroizing::new(master_key),
        payload_offset: segment.offset,
        sector_size: segment.sector_size,
        iv_tweak: segment.iv_tweak,
        total_size,
        position: 0,
        info: VolumeInfo {
            uuid,
            version: 2,
            cipher_name,
            cipher_mode,
            key_bytes,
            sector_size: segment.sector_size,
        },
    })
}

pub(crate) fn payload1<R: Read + Seek>(
    mut reader: R,
    header: &Luks1Header,
    master_key: Vec<u8>,
) -> Result<DecryptedPayload<R>> {
    let total_size = reader.seek(SeekFrom::End(0))?;
    let offset = header.payload_byte_offset();
    if offset < LUKS1_PHDR_LEN as u64 || offset >= total_size || (total_size - offset) % SECTOR != 0
    {
        return Err(std::io::Error::other("invalid LUKS1 payload geometry").into());
    }
    Ok(DecryptedPayload {
        cipher: SectorCipher::new(&header.cipher_name, &header.cipher_mode, &master_key)?,
        reader,
        master_key: Zeroizing::new(master_key),
        payload_offset: offset,
        sector_size: SECTOR as usize,
        iv_tweak: 0,
        total_size,
        position: 0,
        info: VolumeInfo {
            uuid: header.uuid.clone(),
            version: 1,
            cipher_name: header.cipher_name.clone(),
            cipher_mode: header.cipher_mode.clone(),
            key_bytes: header.key_bytes,
            sector_size: SECTOR as usize,
        },
    })
}
