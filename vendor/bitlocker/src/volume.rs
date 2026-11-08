//! Public API: parse a BitLocker volume's metadata and unlock it from a password.
//!
//! [`BitLockerVolume::unlock_with_password`] runs the full chain — parse the
//! volume header, locate and parse the FVE metadata, derive the VMK from the
//! password (stretch → AES-CCM), unwrap the FVEK/TWEAK, and return a
//! [`DecryptedVolume`] exposing a plaintext `Read + Seek` view that honours
//! BitLocker's volume-header relocation and metadata-region blanking.

use std::io::{Read, Seek, SeekFrom};

use crate::crypto::{aes_ccm_unwrap, password_hash, recovery_key_hash, stretch_key, SectorCipher};
use crate::error::{BdeError, Result};
use crate::header::VolumeHeader;
use crate::metadata::{
    FveMetadata, MetadataEntry, PROTECTION_CLEAR, PROTECTION_PASSWORD, PROTECTION_RECOVERY,
    PROTECTION_STARTUP_KEY, VALUE_TYPE_AES_CCM, VALUE_TYPE_EXTERNAL_KEY, VALUE_TYPE_KEY,
    VALUE_TYPE_STRETCH,
};
use crate::method::{EncryptionMethod, SectorCipherKind};

/// Encryption method sentinel: the volume is not encrypted.
const METHOD_NONE: u16 = 0x0000;
/// Bytes needed to parse the volume boot header (data-unit size is variable).
const SECTOR_SIZE: usize = 512;
/// How many bytes to read at a candidate metadata offset (reserved FVE metadata
/// blocks are well under this).
const METADATA_READ_LEN: usize = 64 * 1024;

/// Namespace for opening a BitLocker volume. All state lives in the returned
/// [`DecryptedVolume`]; this type carries no data itself.
pub struct BitLockerVolume;

impl BitLockerVolume {
    /// Parse the volume header and the first valid FVE metadata block from
    /// `reader`, without unlocking. Used by the forensic analyzer to audit the
    /// key protectors even when no password is known.
    ///
    /// # Errors
    /// [`BdeError::NotBitLocker`] if the header signature is absent, or
    /// [`BdeError::NoValidMetadata`] if no `-FVE-FS-` block is found.
    pub fn read_metadata<R: Read + Seek>(reader: &mut R) -> Result<FveMetadata> {
        let mut header = [0u8; SECTOR_SIZE];
        reader.seek(SeekFrom::Start(0))?;
        read_fill(reader, &mut header)?;
        let volume_header = VolumeHeader::parse(&header)?;

        let offsets = volume_header.fve_metadata_offsets;
        let total = reader.seek(SeekFrom::End(0))?;
        let mut candidates = offsets.to_vec();
        let mut index = 0;
        while index < candidates.len() && index < 6 {
            let offset = candidates[index];
            index += 1;
            if offset == 0 || offset.checked_add(112).is_none_or(|end| end > total) {
                continue;
            }
            let mut block = vec![0u8; (total - offset).min(METADATA_READ_LEN as u64) as usize];
            if reader.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let count = match read_available(reader, &mut block) {
                Ok(count) => count,
                Err(_) => continue,
            };
            block.truncate(count);
            // Vista stores backup offsets only in the metadata block header.
            if block.get(..8) == Some(b"-FVE-FS-") && block.len() >= 64 {
                for field in [32, 40, 48] {
                    let backup = crate::bytes::le_u64(&block, field);
                    if backup != 0 && !candidates.contains(&backup) && candidates.len() < 6 {
                        candidates.push(backup);
                    }
                }
            }
            if let Some(meta) = FveMetadata::parse(&block, volume_header.bytes_per_sector) {
                return Ok(meta);
            }
        }
        Err(BdeError::NoValidMetadata { offsets })
    }

    /// Unlock the volume with `password`, returning a plaintext view.
    ///
    /// # Errors
    /// Fails loud on a non-BitLocker image, an unsupported cipher, a missing
    /// password protector, absent key material, or a wrong password (the AES-CCM
    /// tag fails to verify).
    pub fn unlock_with_password<R: Read + Seek>(
        reader: R,
        password: &str,
    ) -> Result<DecryptedVolume<R>> {
        Self::unlock(
            reader,
            PROTECTION_PASSWORD,
            "password",
            &VmkUnwrap::Stretch(password_hash(password)),
        )
    }

    /// Unlock the volume with a 48-digit `recovery` password, returning a
    /// plaintext view. Uses the recovery protector (`0x0800`) and the recovery
    /// key-derivation (`recovery_key_hash`).
    ///
    /// # Errors
    /// [`BdeError::InvalidRecoveryPassword`] if the recovery password is
    /// malformed; otherwise the same failures as [`Self::unlock_with_password`]
    /// (non-BitLocker image, unsupported/unvalidated cipher, no recovery
    /// protector, absent key material, or a wrong key).
    pub fn unlock_with_recovery_password<R: Read + Seek>(
        reader: R,
        recovery: &str,
    ) -> Result<DecryptedVolume<R>> {
        let key_hash = recovery_key_hash(recovery)
            .map_err(|reason| BdeError::InvalidRecoveryPassword { reason })?;
        Self::unlock(
            reader,
            PROTECTION_RECOVERY,
            "recovery password",
            &VmkUnwrap::Stretch(key_hash),
        )
    }

    /// Unlock the volume with **no credential** via its clear-key protector
    /// (`0x0000`), returning a plaintext view. A clear-key protector stores the
    /// VMK unprotected — BitLocker adds it when protection is *suspended* — so the
    /// clear key held in the VMK's KEY property unwraps the VMK directly, with no
    /// stretch and no external key. A clear-key volume is effectively unencrypted.
    ///
    /// # Errors
    /// [`BdeError::NoUnlockProtector`] if the volume carries no clear-key protector;
    /// otherwise the same failures as [`Self::unlock_with_password`] (non-BitLocker
    /// image, unsupported/unvalidated cipher, absent key material, or a wrong key).
    pub fn unlock_clear_key<R: Read + Seek>(reader: R) -> Result<DecryptedVolume<R>> {
        Self::unlock(reader, PROTECTION_CLEAR, "clear key", &VmkUnwrap::Clear)
    }

    /// Unlock the volume with a startup-key (`.BEK`) external key, returning a
    /// plaintext view. `startup_key` is the raw bytes of the `.BEK` file. Uses the
    /// startup-key protector (`0x0200`): the 256-bit external key held in the
    /// `.BEK` AES-CCM-unwraps the VMK directly — no stretch, no password.
    ///
    /// # Errors
    /// [`BdeError::MissingKeyMaterial`] if the `.BEK` carries no external key,
    /// [`BdeError::NoUnlockProtector`] if the volume has no startup-key protector;
    /// otherwise the same failures as [`Self::unlock_with_password`] (non-BitLocker
    /// image, unsupported/unvalidated cipher, absent key material, or a wrong key).
    pub fn unlock_with_startup_key<R: Read + Seek>(
        reader: R,
        startup_key: &[u8],
    ) -> Result<DecryptedVolume<R>> {
        let (guid, external_key) = parse_startup_key(startup_key)?;
        Self::unlock(
            reader,
            PROTECTION_STARTUP_KEY,
            "startup key",
            &VmkUnwrap::External(guid, external_key),
        )
    }

    /// Authenticate a raw 256-bit VMK against the encrypted FVEK.
    /// No recovery-password protector is required.
    pub fn unlock_with_vmk<R: Read + Seek>(
        mut reader: R,
        vmk: &[u8],
    ) -> Result<DecryptedVolume<R>> {
        if vmk.len() != 32 {
            return Err(BdeError::InvalidData("VMK must contain exactly 32 bytes"));
        }
        let metadata = Self::read_metadata(&mut reader)?;
        let cipher = cipher_from_vmk(
            &metadata,
            cipher_kind(metadata.encryption_method)?,
            &take_key32(vmk, 0, "VMK")?,
        )?;
        DecryptedVolume::open(reader, metadata, cipher)
    }

    /// Open with raw FVEK/TWEAK bytes. These keys have no authentication tag;
    /// callers must independently verify the resulting plaintext.
    pub fn unlock_with_keys<R: Read + Seek>(
        mut reader: R,
        fvek: &[u8],
        tweak: &[u8],
    ) -> Result<DecryptedVolume<R>> {
        let metadata = Self::read_metadata(&mut reader)?;
        let kind = cipher_kind(metadata.encryption_method)?;
        let key_size = match kind {
            SectorCipherKind::Cbc128Diffuser | SectorCipherKind::Cbc128 => 16,
            SectorCipherKind::Xts256 => 64,
            _ => 32,
        };
        let tweak_size = match kind {
            SectorCipherKind::Cbc128Diffuser => 16,
            SectorCipherKind::Cbc256Diffuser => 32,
            _ => 0,
        };
        if fvek.len() != key_size || tweak.len() != tweak_size {
            return Err(BdeError::InvalidData(
                "incorrect FVEK/TWEAK length for cipher",
            ));
        }
        let mut container = vec![0; 12 + key_size.max(32) + tweak_size];
        container[12..12 + key_size].copy_from_slice(fvek);
        if tweak_size > 0 {
            container[44..44 + tweak_size].copy_from_slice(tweak);
        }
        let cipher = cipher_from_container(kind, &container)?;
        DecryptedVolume::open(reader, metadata, cipher)
    }

    /// Shared unlock path: parse metadata, gate the cipher method, derive the
    /// sector cipher from the given protector, and return the plaintext view.
    fn unlock<R: Read + Seek>(
        mut reader: R,
        protector_type: u16,
        protector_name: &'static str,
        unwrap: &VmkUnwrap,
    ) -> Result<DecryptedVolume<R>> {
        let metadata = Self::read_metadata(&mut reader)?;

        // Unencrypted volumes do not require a protector. Other methods must
        // have an implemented transform and authenticated key material.
        if metadata.encryption_method == METHOD_NONE {
            return DecryptedVolume::open(reader, metadata, SectorCipher::unencrypted());
        }
        let kind = cipher_kind(metadata.encryption_method)?;
        let cipher = derive_cipher(&metadata, kind, protector_type, protector_name, unwrap)?;
        DecryptedVolume::open(reader, metadata, cipher)
    }
}

/// How the AES-CCM key that unwraps the VMK is obtained.
enum VmkUnwrap {
    /// Stretch this credential hash with the VMK's stretch-key salt (password /
    /// recovery-password protector).
    Stretch([u8; 32]),
    /// Read the clear key stored in the VMK's KEY property — no credential, no
    /// stretch (clear-key protector `0x0000`).
    Clear,
    /// Use this 256-bit external key (parsed from a startup-key `.BEK`) directly
    /// as the AES-CCM key that unwraps the VMK — no credential, no stretch, no
    /// stored key (startup-key protector `0x0200`).
    External([u8; 16], [u8; 32]),
}

/// Extract the 256-bit external key from a startup-key `.BEK` file.
///
/// The `.BEK` is a 48-byte FVE metadata header followed by an external-key entry
/// (value type `0x0009`) whose payload is a 16-byte key GUID, an 8-byte FILETIME,
/// then nested entries — one of which is a KEY property (`0x0001`: a 4-byte
/// key-type then the raw 32-byte external key). Bounds-checked throughout: a
/// truncated or malformed `.BEK` yields a loud error, never a panic.
fn parse_startup_key(bek: &[u8]) -> Result<([u8; 16], [u8; 32])> {
    let body = bek.get(48..).unwrap_or_default();
    let external = MetadataEntry::parse_sequence(body)
        .into_iter()
        .find(|e| e.value_type == VALUE_TYPE_EXTERNAL_KEY)
        .ok_or(BdeError::MissingKeyMaterial {
            what: "external key",
        })?;
    // The external-key payload is a 16-byte key GUID + 8-byte FILETIME, then the
    // nested entries hold the KEY property with the raw 32-byte key.
    let key = external
        .nested(24)
        .into_iter()
        .find(|e| e.value_type == VALUE_TYPE_KEY)
        .ok_or(BdeError::MissingKeyMaterial {
            what: "external key property",
        })?;
    let guid = take_key16(&external.data, 0, "external key identifier")?;
    Ok((guid, take_key32(&key.data, 4, "external key")?))
}

/// Derive the sector cipher from the VMK protected by `protector_type`, obtaining
/// the VMK's AES-CCM unwrap key per `unwrap` (stretch a credential, or read the
/// stored clear key), and build the transform for the already-validated `kind`.
/// `protector_name` names the protector in the not-found error.
fn derive_cipher(
    metadata: &FveMetadata,
    kind: SectorCipherKind,
    protector_type: u16,
    protector_name: &'static str,
    unwrap: &VmkUnwrap,
) -> Result<SectorCipher> {
    let mut error = BdeError::NoUnlockProtector {
        protector: protector_name,
        found: metadata.protector_types(),
    };
    for vmk in metadata
        .vmk_entries()
        .filter(|entry| entry.protection_type() == Some(protector_type))
    {
        match derive_cipher_from_protector(metadata, kind, vmk, unwrap) {
            Ok(cipher) => return Ok(cipher),
            Err(reason) => error = reason,
        }
    }
    Err(error)
}

fn derive_cipher_from_protector(
    metadata: &FveMetadata,
    kind: SectorCipherKind,
    vmk: &MetadataEntry,
    unwrap: &VmkUnwrap,
) -> Result<SectorCipher> {
    // VMK properties are nested entries starting at value-data offset 28.
    let props = vmk.nested(28);
    let vmk_ccm = props
        .iter()
        .find(|e| e.value_type == VALUE_TYPE_AES_CCM)
        .ok_or(BdeError::MissingKeyMaterial {
            what: "VMK AES-CCM key",
        })?;

    // 2. Obtain the 32-byte key that AES-CCM-unwraps the VMK: stretch the
    //    credential with the VMK's stretch-key salt, or read the clear key stored
    //    in the VMK's KEY property (clear-key protector — no credential).
    let unwrap_key = match unwrap {
        VmkUnwrap::Stretch(key_hash) => {
            let stretch = props
                .iter()
                .find(|e| e.value_type == VALUE_TYPE_STRETCH)
                .ok_or(BdeError::MissingKeyMaterial {
                    what: "stretch key",
                })?;
            // Salt is at stretch value-data offset 4 (after the 4-byte method).
            let mut salt = [0u8; 16];
            let salt_src = stretch
                .data
                .get(4..20)
                .ok_or(BdeError::MissingKeyMaterial {
                    what: "stretch salt",
                })?;
            salt.copy_from_slice(salt_src);
            stretch_key(key_hash, &salt)
        }
        VmkUnwrap::Clear => {
            // FVE KEY property: a 4-byte method, then the 32-byte clear key.
            let key = props
                .iter()
                .find(|e| e.value_type == VALUE_TYPE_KEY)
                .ok_or(BdeError::MissingKeyMaterial { what: "clear key" })?;
            take_key32(&key.data, 4, "clear")?
        }
        // The external key from the .BEK is the AES-CCM unwrap key directly.
        VmkUnwrap::External(guid, key) => {
            if vmk.data.get(..16) != Some(guid.as_slice()) {
                return Err(BdeError::InvalidData(
                    "BEK identifier does not match protector",
                ));
            }
            *key
        }
    };

    let vmk_container =
        aes_ccm_unwrap(&unwrap_key, &vmk_ccm.data).ok_or(BdeError::AuthenticationFailed {
            what: "volume master key",
        })?;
    let vmk_key = take_key32(&vmk_container, 12, "volume master key")?;

    cipher_from_vmk(metadata, kind, &vmk_key)
}

fn cipher_kind(raw: u16) -> Result<SectorCipherKind> {
    EncryptionMethod::decode(raw)
        .ok_or(BdeError::UnsupportedEncryptionMethod { method: raw })?
        .validated_kind()
        .ok_or(BdeError::UnvalidatedEncryptionMethod { method: raw })
}

fn cipher_from_vmk(
    metadata: &FveMetadata,
    kind: SectorCipherKind,
    vmk: &[u8; 32],
) -> Result<SectorCipher> {
    let entry = metadata
        .fvek_entry()
        .ok_or(BdeError::MissingKeyMaterial { what: "FVEK entry" })?;
    let container =
        aes_ccm_unwrap(vmk, &entry.data).ok_or(BdeError::AuthenticationFailed { what: "FVEK" })?;
    cipher_from_container(kind, &container)
}

fn cipher_from_container(kind: SectorCipherKind, fvek_container: &[u8]) -> Result<SectorCipher> {
    match kind {
        SectorCipherKind::Cbc256Diffuser => Ok(SectorCipher::new_cbc256_diffuser(
            take_key32(fvek_container, 12, "FVEK")?,
            take_key32(fvek_container, 44, "TWEAK")?,
        )),
        SectorCipherKind::Cbc128Diffuser => {
            let fvek = take_key16(fvek_container, 12, "FVEK")?;
            let tweak = take_key16(fvek_container, 44, "FVEK")?;
            Ok(SectorCipher::new(fvek, tweak))
        }
        SectorCipherKind::Cbc128 => {
            let fvek = take_key16(fvek_container, 12, "FVEK")?;
            Ok(SectorCipher::new_cbc(fvek))
        }
        SectorCipherKind::Cbc256 => {
            let fvek = take_key32(fvek_container, 12, "FVEK")?;
            Ok(SectorCipher::new_cbc256(fvek))
        }
        SectorCipherKind::Xts128 => {
            // XTS-128 carries a 32-byte key (two AES-128 keys) at offset 12.
            let fvek = take_key32(fvek_container, 12, "FVEK")?;
            Ok(SectorCipher::new_xts128(fvek))
        }
        SectorCipherKind::Xts256 => {
            // XTS-256 carries a 64-byte key (two AES-256 keys) at offset 12.
            let fvek = take_key64(fvek_container, 12, "FVEK")?;
            Ok(SectorCipher::new_xts256(fvek))
        }
    }
}

fn take_key64(container: &[u8], off: usize, what: &'static str) -> Result<[u8; 64]> {
    let s = container
        .get(off..off + 64)
        .ok_or(BdeError::MalformedKeyContainer {
            what,
            got: container.len(),
            need: off + 64,
        })?;
    let mut k = [0u8; 64];
    k.copy_from_slice(s);
    Ok(k)
}

fn take_key32(container: &[u8], off: usize, what: &'static str) -> Result<[u8; 32]> {
    let s = container
        .get(off..off + 32)
        .ok_or(BdeError::MalformedKeyContainer {
            what,
            got: container.len(),
            need: off + 32,
        })?;
    let mut k = [0u8; 32];
    k.copy_from_slice(s);
    Ok(k)
}

fn take_key16(container: &[u8], off: usize, what: &'static str) -> Result<[u8; 16]> {
    let s = container
        .get(off..off + 16)
        .ok_or(BdeError::MalformedKeyContainer {
            what,
            got: container.len(),
            need: off + 16,
        })?;
    let mut k = [0u8; 16];
    k.copy_from_slice(s);
    Ok(k)
}

/// A plaintext view of an unlocked BitLocker volume.
pub struct DecryptedVolume<R> {
    reader: R,
    cipher: SectorCipher,
    metadata: FveMetadata,
    total_size: u64,
    position: u64,
    sector_size: usize,
    eow: crate::eow::EowMap,
}

#[path = "volume_read.rs"]
mod read;

/// Read exactly `buf.len()` bytes, erroring on premature EOF.
fn read_fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    reader.read_exact(buf)?;
    Ok(())
}

/// Read up to `buf.len()` bytes, zero-filling the remainder on EOF. Returns the
/// number of real bytes read.
fn read_available<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    for b in &mut buf[filled..] {
        *b = 0;
    }
    Ok(filled)
}

#[cfg(test)]
#[path = "volume_tests.rs"]
mod tests;
