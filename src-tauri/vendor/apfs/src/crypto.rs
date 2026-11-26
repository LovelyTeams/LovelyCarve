//! Software-encrypted APFS volumes and their on-disk key lockers.
use aes::{Aes128, cipher::KeyInit};
use aes_kw::KekAes256;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::{Read, Seek, SeekFrom};
use xts_mode::Xts128;
use zeroize::Zeroizing;

use crate::error::{ApfsError, Result};
use crate::object::ObjectHeader;
use crate::superblock::{ApfsSuperblock, NxSuperblock};

pub(crate) const CRYPTO_SECTOR: usize = 512;
pub(crate) const FS_UNENCRYPTED: u64 = 1;
const FS_ONEKEY: u64 = 8;
const CONTAINER_KEYBAG: u32 = 0x6b65_7973;
const VOLUME_KEYBAG: u32 = 0x7265_6373;
const VOLUME_KEY: u16 = 2;
const UNLOCK_RECORD: u16 = 3;
const MAX_BAG_BLOCKS: u64 = 64;
const MAX_PASSWORD_ROUNDS: u32 = 20_000_000;

pub(crate) struct VolumeCipher(Xts128<Aes128>);

impl VolumeCipher {
    fn new(first: &[u8; 16], second: &[u8; 16]) -> Self {
        Self(Xts128::new(
            Aes128::new(first.into()),
            Aes128::new(second.into()),
        ))
    }

    pub(crate) fn decrypt(&self, bytes: &mut [u8], first_sector: u64) {
        let (sectors, remainder) = bytes.as_chunks_mut::<CRYPTO_SECTOR>();
        debug_assert!(remainder.is_empty());
        for (index, sector) in sectors.iter_mut().enumerate() {
            let tweak = (u128::from(first_sector) + index as u128).to_le_bytes();
            self.0.decrypt_sector(sector, tweak);
        }
    }
}

pub(crate) fn validate_volume(volume: &ApfsSuperblock) -> Result<()> {
    if volume.encryption_state_oid != 0 {
        return Err(ApfsError::Unsupported("APFS encryption transition".into()));
    }
    if volume.fs_flags & (FS_UNENCRYPTED | FS_ONEKEY) == 0 {
        return Err(ApfsError::Unsupported(
            "APFS volume uses per-file encryption keys".into(),
        ));
    }
    Ok(())
}

pub(crate) fn unlock<R: Read + Seek>(
    reader: &mut R,
    nx: &NxSuperblock,
    volume: &ApfsSuperblock,
    credential: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Result<VolumeCipher> {
    if cancelled() {
        return Err(ApfsError::Cancelled);
    }
    validate_volume(volume)?;
    if nx.keylocker_block == 0 && nx.keylocker_count == 0 {
        return Err(ApfsError::Unsupported(
            "APFS container has no software key locker".into(),
        ));
    }
    let bag = read_keybag(
        reader,
        nx,
        (nx.keylocker_block, nx.keylocker_count),
        &nx.uuid,
        CONTAINER_KEYBAG,
    )?;
    let (mut wrapped_volume, mut volume_bag) = (None, None);
    for entry in records(&bag)? {
        if entry.uuid != volume.uuid {
            continue;
        }
        let slot = match entry.tag {
            VOLUME_KEY => &mut wrapped_volume,
            UNLOCK_RECORD => &mut volume_bag,
            _ => continue,
        };
        if slot.replace(entry.data).is_some() {
            return Err(bad_bag("duplicate volume record"));
        }
    }
    let (Some(wrapped_volume), Some(volume_bag)) = (wrapped_volume, volume_bag) else {
        return Err(ApfsError::Unsupported(
            "APFS volume has no software unlock records".into(),
        ));
    };
    let wrapped_volume = WrappedKey::parse(wrapped_volume)?;
    if !wrapped_volume.software() {
        return Err(ApfsError::Unsupported(
            "APFS volume key wrapping method".into(),
        ));
    }
    let range: &[u8; 16] = volume_bag
        .try_into()
        .map_err(|_| bad_bag("volume key locker range"))?;
    let range = (
        u64::from_le_bytes(range[..8].try_into().unwrap()),
        u64::from_le_bytes(range[8..].try_into().unwrap()),
    );
    let bag = read_keybag(reader, nx, range, &volume.uuid, VOLUME_KEYBAG)?;
    let mut attempted = false;
    let mut rounds_left = MAX_PASSWORD_ROUNDS;
    let mut record_error = None;
    for entry in records(&bag)?
        .into_iter()
        .filter(|entry| entry.tag == UNLOCK_RECORD)
    {
        let record = match WrappedKey::parse(entry.data) {
            Ok(record) => record,
            Err(error) => {
                record_error.get_or_insert(error);
                continue;
            }
        };
        if !record.software() {
            continue;
        }
        let (rounds, salt) = match record.password_parameters() {
            Ok(parameters) => parameters,
            Err(error) => {
                record_error.get_or_insert(error);
                continue;
            }
        };
        let Some(remaining) = rounds_left.checked_sub(rounds) else {
            record_error.get_or_insert_with(|| {
                ApfsError::Unsupported("APFS password derivation cost limit".into())
            });
            continue;
        };
        rounds_left = remaining;
        let derived = derive_password_key(credential, salt, rounds, cancelled)?;
        attempted = true;
        let mut kek = Zeroizing::new([0u8; 32]);
        if KekAes256::from(*derived)
            .unwrap(record.key, kek.as_mut())
            .is_err()
        {
            continue;
        }
        let mut vek = Zeroizing::new([0u8; 32]);
        // A successful KEK followed by a wrong VEK can occur in a bag with stale records.
        if KekAes256::from(*kek)
            .unwrap(wrapped_volume.key, vek.as_mut())
            .is_ok()
        {
            return Ok(VolumeCipher::new(
                vek[..16].try_into().unwrap(),
                vek[16..].try_into().unwrap(),
            ));
        }
    }
    if let Some(error) = record_error {
        Err(error)
    } else if attempted {
        Err(ApfsError::InvalidCredential)
    } else {
        Err(ApfsError::Unsupported(
            "APFS key bag has no supported password or personal recovery record".into(),
        ))
    }
}

/// PBKDF2-HMAC-SHA256 for a single 32-byte derived key, checking cancellation
/// periodically between PRF evaluations (RFC 8018 §5.2).
fn derive_password_key(
    password: &[u8],
    salt: &[u8],
    rounds: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<Zeroizing<[u8; 32]>> {
    if cancelled() {
        return Err(ApfsError::Cancelled);
    }
    let hmac =
        <Hmac<Sha256> as Mac>::new_from_slice(password).expect("HMAC accepts any key length");
    let mut first = hmac.clone();
    first.update(salt);
    first.update(&1u32.to_be_bytes());
    let mut previous: Zeroizing<[u8; 32]> = Zeroizing::new(first.finalize().into_bytes().into());
    let mut result = Zeroizing::new(*previous);
    for round in 1..rounds {
        if round.is_multiple_of(2048) && cancelled() {
            return Err(ApfsError::Cancelled);
        }
        let mut prf = hmac.clone();
        prf.update(previous.as_ref());
        previous.copy_from_slice(&prf.finalize().into_bytes());
        for (out, value) in result.iter_mut().zip(previous.iter()) {
            *out ^= *value;
        }
    }
    if cancelled() {
        return Err(ApfsError::Cancelled);
    }
    Ok(result)
}

#[cfg(test)]
#[test]
fn password_derivation_agrees_with_rustcrypto_and_polls_cancellation() {
    let password = b"password";
    let salt = b"apfs-salt";
    let rounds = 8129;
    let mut expected = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, rounds, &mut expected);
    assert_eq!(
        *derive_password_key(password, salt, rounds, &|| false).unwrap(),
        expected
    );
    let checks = std::cell::Cell::new(0);
    let result = derive_password_key(password, salt, rounds, &|| {
        checks.set(checks.get() + 1);
        checks.get() == 3
    });
    assert!(matches!(result, Err(ApfsError::Cancelled)));
}

fn bad_bag(reason: &str) -> ApfsError {
    ApfsError::CorruptedData(format!("APFS key bag: {reason}"))
}

fn read_keybag<R: Read + Seek>(
    reader: &mut R,
    nx: &NxSuperblock,
    (block, count): (u64, u64),
    uuid: &[u8; 16],
    kind: u32,
) -> Result<Vec<u8>> {
    if block == 0 || !(1..=MAX_BAG_BLOCKS).contains(&count) || *uuid == [0; 16] {
        return Err(bad_bag("invalid block range or UUID"));
    }
    let block_size = u64::from(nx.block_size);
    let offset = block
        .checked_mul(block_size)
        .ok_or_else(|| bad_bag("offset overflow"))?;
    let length = count * block_size;
    offset
        .checked_add(length)
        .ok_or_else(|| bad_bag("end overflow"))?;
    reader.seek(SeekFrom::Start(offset))?;
    let mut bag = vec![0; length as usize];
    reader.read_exact(&mut bag)?;
    let valid = |bytes: &[u8]| {
        ObjectHeader::parse(bytes).is_ok_and(|header| {
            header.type_and_flags == kind && header.subtype == 0 && header.xid <= nx.header.xid
        }) && crate::fletcher::verify_object(bytes)
    };
    if !valid(&bag) {
        VolumeCipher::new(uuid, uuid).decrypt(&mut bag, offset / CRYPTO_SECTOR as u64);
        if !valid(&bag) {
            return Err(bad_bag(
                "object type, transaction or checksum does not match",
            ));
        }
    }
    Ok(bag)
}

struct BagRecord<'a> {
    uuid: [u8; 16],
    tag: u16,
    data: &'a [u8],
}

fn records(bytes: &[u8]) -> Result<Vec<BagRecord<'_>>> {
    let bytes = bytes
        .get(ObjectHeader::SIZE..)
        .ok_or_else(|| bad_bag("short locker object"))?;
    let (header, _) = bytes
        .split_first_chunk::<16>()
        .ok_or_else(|| bad_bag("short locker header"))?;
    let version = u16::from_le_bytes(header[..2].try_into().unwrap());
    let count = u16::from_le_bytes(header[2..4].try_into().unwrap()) as usize;
    let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if version != 2 {
        return Err(ApfsError::Unsupported(format!(
            "APFS key bag version {version}"
        )));
    }
    let locker = bytes
        .get(..length)
        .ok_or_else(|| bad_bag("locker length"))?;
    let mut entries = locker.get(16..).ok_or_else(|| bad_bag("locker length"))?;
    // Even an empty, aligned record occupies 32 bytes.
    if count > entries.len() / 32 {
        return Err(bad_bag("record count exceeds locker"));
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let (header, payload) = entries
            .split_first_chunk::<24>()
            .ok_or_else(|| bad_bag("short record header"))?;
        let size = u16::from_le_bytes(header[18..20].try_into().unwrap()) as usize;
        let data = payload
            .get(..size)
            .ok_or_else(|| bad_bag("record length"))?;
        records.push(BagRecord {
            uuid: header[..16].try_into().unwrap(),
            tag: u16::from_le_bytes(header[16..18].try_into().unwrap()),
            data,
        });
        entries = entries
            .get((24 + size).next_multiple_of(16)..)
            .ok_or_else(|| bad_bag("record padding exceeds locker"))?;
    }
    if entries.iter().any(|&byte| byte != 0) {
        return Err(bad_bag("unaccounted record data"));
    }
    Ok(records)
}

fn packed_fields(mut bytes: &[u8]) -> Result<[Option<&[u8]>; 256]> {
    let mut fields = [None; 256];
    while !bytes.is_empty() {
        let (&[tag, length], rest) = bytes
            .split_first_chunk::<2>()
            .ok_or_else(|| bad_bag("short packed header"))?;
        bytes = rest;
        if tag == 0 && length == 0 && bytes.iter().all(|&byte| byte == 0) {
            break;
        }
        let size = if length < 128 {
            usize::from(length)
        } else {
            let width = usize::from(length & 127);
            if !(1..=4).contains(&width) {
                return Err(bad_bag("unsupported packed length"));
            }
            let (length_bytes, rest) = bytes
                .split_at_checked(width)
                .ok_or_else(|| bad_bag("short packed length"))?;
            bytes = rest;
            length_bytes
                .iter()
                .fold(0usize, |total, &byte| (total << 8) | usize::from(byte))
        };
        let (value, rest) = bytes
            .split_at_checked(size)
            .ok_or_else(|| bad_bag("packed value exceeds record"))?;
        if fields[tag as usize].replace(value).is_some() {
            return Err(bad_bag("duplicate packed field"));
        }
        bytes = rest;
    }
    Ok(fields)
}

struct WrappedKey<'a> {
    flags: u32,
    key: &'a [u8],
    rounds: Option<&'a [u8]>,
    salt: Option<&'a [u8]>,
}

impl<'a> WrappedKey<'a> {
    fn parse(blob: &'a [u8]) -> Result<Self> {
        let outer = packed_fields(blob)?[0x30].ok_or_else(|| bad_bag("wrapped key envelope"))?;
        let fields = packed_fields(outer)?[0xa3].ok_or_else(|| bad_bag("wrapped key payload"))?;
        let fields = packed_fields(fields)?;
        let metadata = fields[0x82]
            .and_then(|metadata| metadata.first_chunk::<4>())
            .ok_or_else(|| bad_bag("wrapped key flags"))?;
        let key = fields[0x83].ok_or_else(|| bad_bag("wrapped key bytes"))?;
        Ok(Self {
            flags: u32::from_le_bytes(*metadata),
            key,
            rounds: fields[0x84],
            salt: fields[0x85],
        })
    }

    fn software(&self) -> bool {
        self.flags & !0x10 == 0 && self.key.len() == 40
    }

    fn password_parameters(&self) -> Result<(u32, &[u8])> {
        let bytes = self
            .rounds
            .filter(|bytes| (1..=4).contains(&bytes.len()))
            .ok_or_else(|| bad_bag("invalid password iteration count"))?;
        let count = bytes
            .iter()
            .fold(0u32, |total, &byte| (total << 8) | u32::from(byte));
        if count == 0 {
            return Err(bad_bag("zero password iteration count"));
        }
        let salt = self
            .salt
            .filter(|salt| salt.len() == 16)
            .ok_or_else(|| bad_bag("password salt"))?;
        Ok((count, salt))
    }
}

#[cfg(test)]
#[test]
fn seeks_through_relocated_ciphertext_and_a_sparse_sector() {
    use crate::catalog::{FileExtentRecord, FileExtentVal};
    use crate::extents::ApfsForkReader;
    use std::io::Cursor;

    let cipher = VolumeCipher::new(&[0x11; 16], &[0x29; 16]);
    let block_size = 4096;
    let (old_block, new_block) = (7, 3);
    let plaintext: Vec<u8> = (0..block_size)
        .map(|index| ((index * 137 + 13) % 251) as u8)
        .collect();
    let mut raw = vec![0u8; (new_block + 1) * block_size];
    let stored = &mut raw[new_block * block_size..];
    stored.copy_from_slice(&plaintext);
    for (sector, bytes) in stored
        .as_chunks_mut::<CRYPTO_SECTOR>()
        .0
        .iter_mut()
        .enumerate()
    {
        let tweak = (old_block * block_size / CRYPTO_SECTOR + sector) as u128;
        cipher.0.encrypt_sector(bytes, tweak.to_le_bytes());
    }
    let extents = vec![
        FileExtentRecord {
            logical_addr: 0,
            value: FileExtentVal {
                flags_and_length: block_size as u64,
                phys_block_num: 0,
                crypto_id: 0,
            },
        },
        FileExtentRecord {
            logical_addr: block_size as u64,
            value: FileExtentVal {
                flags_and_length: (1 << 56) | block_size as u64,
                phys_block_num: new_block as u64,
                crypto_id: old_block as u64,
            },
        },
    ];
    let mut reader = crate::backing::ContainerReader::new(Cursor::new(&raw), raw.len() as u64);
    let mut fork = ApfsForkReader::from_container(
        &mut reader,
        block_size as u32,
        extents,
        2 * block_size as u64,
        Some(&cipher),
    )
    .unwrap();
    fork.seek(SeekFrom::Start((block_size - 1) as u64)).unwrap();
    let mut range = [0; 1004];
    fork.read_exact(&mut range).unwrap();
    assert_eq!(range[0], 0);
    assert_eq!(&range[1..], &plaintext[..1003]);
    fork.seek(SeekFrom::Start((block_size + 511) as u64))
        .unwrap();
    fork.read_exact(&mut range).unwrap();
    assert_eq!(range, plaintext[511..1515]);
}
