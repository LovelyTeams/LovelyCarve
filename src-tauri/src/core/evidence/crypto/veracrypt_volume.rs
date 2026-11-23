//! VeraCrypt and TrueCrypt through the shared unlock: the header, PRF, PIM
//! and keyfile options, the password the header key derives from, opening
//! the volume, and the facts an unlocked volume reports.

use std::path::Path;

use anyhow::{anyhow, ensure, Context};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::encrypted_image::{
    partition_range, wrong_credential, EncryptionScheme, EvidenceReader, OpenedVolume, Plaintext,
    PlaintextLayout, UnlockCredential, UnlockOptions,
};
use crate::core::common::error::Failure;
use crate::core::evidence::image::DiskImage;

/// Keyfiles one unlock reads at most (a keyfile directory expands to its
/// files).
pub const MAX_KEYFILES: usize = 1024;

/// Which VeraCrypt headers an unlock reads.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VeracryptHeader {
    /// The normal and the hidden header, as VeraCrypt tries them.
    #[default]
    Auto,
    /// Only the normal (outer) header.
    Normal,
    /// Only hidden-volume headers.
    Hidden,
    /// The embedded backup headers.
    Backup,
    /// The system encryption header in sector 62 of the drive.
    System,
}

/// The key derivation an unlock may be limited to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VeracryptPrf {
    Sha512,
    Sha256,
    Blake2s,
    Whirlpool,
    Streebog,
    Ripemd160,
    Argon2id,
}

impl VeracryptPrf {
    pub(crate) fn prf(self) -> veracrypt::Prf {
        match self {
            Self::Sha512 => veracrypt::Prf::Sha512,
            Self::Sha256 => veracrypt::Prf::Sha256,
            Self::Blake2s => veracrypt::Prf::Blake2s,
            Self::Whirlpool => veracrypt::Prf::Whirlpool,
            Self::Streebog => veracrypt::Prf::Streebog,
            Self::Ripemd160 => veracrypt::Prf::Ripemd160,
            Self::Argon2id => veracrypt::Prf::Argon2id,
        }
    }
}

/// What an unlocked VeraCrypt or TrueCrypt volume reports.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VeracryptDetails {
    /// `VeraCrypt` or `TrueCrypt`.
    pub flavor: &'static str,
    /// The header that opened: `normal`, `hidden`, `backupNormal`,
    /// `backupHidden`, `legacyHidden` or `system`.
    pub header: &'static str,
    /// The key derivation (`sha512`, `argon2id` …).
    pub prf: &'static str,
    /// PBKDF2 iterations or Argon2id passes.
    pub iterations: u32,
    /// Argon2id memory in KiB; zero for PBKDF2.
    pub memory_kib: u32,
    /// The encryption algorithm, as VeraCrypt names it (`AES-Twofish`).
    pub cipher: String,
    pub header_version: u16,
    pub required_program_version: u16,
    pub sector_size: u32,
    pub volume_size: u64,
    pub hidden_volume_size: u64,
    pub encrypted_area_start: u64,
    pub encrypted_area_length: u64,
    /// A system (pre-boot) encryption header.
    pub system: bool,
    /// Non-system in-place encryption made the volume.
    pub in_place: bool,
    /// False when system encryption was paused: the unencrypted tail reads
    /// as stored.
    pub encryption_complete: bool,
    /// The primary and secondary master keys are equal.
    pub weak_master_key: bool,
    /// The first sector is a plaintext NTFS boot sector passed through as
    /// stored.
    pub plaintext_boot_sector: bool,
    /// Keyfiles mixed into the password.
    pub keyfiles: usize,
}

impl VeracryptDetails {
    pub(crate) fn of(info: &veracrypt::VolumeInfo, keyfiles: usize) -> Self {
        Self {
            flavor: info.flavor.name(),
            header: info.header.id(),
            prf: info.kdf.prf.id(),
            iterations: info.kdf.iterations,
            memory_kib: info.kdf.memory_kib,
            cipher: info.chain.name(),
            header_version: info.header_version,
            required_program_version: info.required_program_version,
            sector_size: info.sector_size,
            volume_size: info.volume_size,
            hidden_volume_size: info.hidden_volume_size,
            encrypted_area_start: info.encrypted_area_start,
            encrypted_area_length: info.encrypted_area_length,
            system: info.system(),
            in_place: info.in_place(),
            encryption_complete: info.encryption_complete,
            weak_master_key: info.weak_master_key,
            plaintext_boot_sector: info.plaintext_boot_sector,
            keyfiles,
        }
    }

    /// The task-center and toast label: `VeraCrypt AES-Twofish / SHA-512`.
    pub(crate) fn label(info: &veracrypt::VolumeInfo) -> String {
        format!(
            "{} {} / {}",
            info.flavor.name(),
            info.chain.name(),
            info.kdf.prf.name()
        )
    }
}

/// Open the VeraCrypt or TrueCrypt volume of `reader`. VeraCrypt volumes
/// carry no signature; a region that is not a whole number of the evidence's
/// sectors cannot be one. A system encryption header is read from sector 62
/// of the drive `reader` covers; the plaintext is the chosen partition, or the
/// whole drive with its unencrypted parts as stored.
pub(super) fn open(
    source: &DiskImage,
    reader: EvidenceReader,
    options: &UnlockOptions,
    cancellation: &CancellationToken,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> anyhow::Result<OpenedVolume> {
    let (_, length) = reader.bounds();
    let sector = source.sector_size as u64;
    ensure!(
        sector > 0 && length.is_multiple_of(sector),
        Failure::invalid_input(format!(
            "a VeraCrypt volume is a whole number of {sector}-byte sectors; the region is {length} bytes"
        ))
    );
    let password = match &options.credential {
        UnlockCredential::Password { password } => password,
        credential @ (UnlockCredential::RecoveryPassword { .. }
        | UnlockCredential::StartupKey { .. }
        | UnlockCredential::MasterKey { .. }
        | UnlockCredential::Vmk { .. }
        | UnlockCredential::ClearKey) => {
            return Err(wrong_credential(credential, EncryptionScheme::Veracrypt))
        }
    };
    let (password, keyfiles) = effective_password(password, &options.keyfiles)?;
    let mut source_range = None;
    let headers = match options.header {
        VeracryptHeader::Auto => veracrypt::HeaderSelection::Auto,
        VeracryptHeader::Normal => veracrypt::HeaderSelection::Normal,
        VeracryptHeader::Hidden => veracrypt::HeaderSelection::Hidden,
        VeracryptHeader::Backup => veracrypt::HeaderSelection::Backup,
        VeracryptHeader::System => veracrypt::HeaderSelection::System(match options.partition_id {
            Some(id) => {
                let (offset, length) = partition_range(source, id)?;
                source_range = Some((offset, length));
                veracrypt::SystemRange::Partition { offset, length }
            }
            None => veracrypt::SystemRange::Drive,
        }),
    };
    let request = veracrypt::UnlockRequest {
        password: &password,
        pim: options.pim,
        prf: options.prf.map(VeracryptPrf::prf),
        headers,
    };
    let report = |step: veracrypt::Progress| progress(step.done, step.total);
    let control = veracrypt::Control {
        cancelled: &|| cancellation.is_cancelled(),
        progress: Some(&report),
        threads: None,
    };
    let volume = veracrypt::unlock(reader, &request, &control).map_err(map_error)?;
    let info = volume.info().clone();
    let length = volume.size();
    let mut opened = OpenedVolume::new(
        Plaintext::Veracrypt(Box::new(volume)),
        length,
        VeracryptDetails::label(&info),
        info.sector_size as usize,
    );
    opened.hidden = info.header.is_hidden();
    opened.veracrypt = Some(VeracryptDetails::of(&info, keyfiles));
    opened.source_range = source_range;
    // A system partition holds one filesystem; containers and drives may
    // hold a partition table.
    if options.header == VeracryptHeader::System && options.partition_id.is_some() {
        opened.layout = PlaintextLayout::Volume;
    }
    Ok(opened)
}

/// The password the header key derives from: the typed password, mixed with
/// the keyfiles when there are any, and how many keyfiles went in.
pub(crate) fn effective_password(
    password: &str,
    keyfiles: &[String],
) -> anyhow::Result<(Zeroizing<Vec<u8>>, usize)> {
    ensure!(
        password.len() <= veracrypt::keyfile::MAX_PASSWORD_LEN,
        Failure::invalid_input(format!(
            "VeraCrypt passwords are at most {} bytes",
            veracrypt::keyfile::MAX_PASSWORD_LEN
        ))
    );
    if keyfiles.is_empty() {
        ensure!(
            !password.is_empty(),
            Failure::invalid_input("the password is empty")
        );
        return Ok((Zeroizing::new(password.as_bytes().to_vec()), 0));
    }
    let mut pool = veracrypt::KeyfilePool::new(password.len());
    for path in keyfiles {
        let files = veracrypt::keyfile::expand(Path::new(path))
            .with_context(|| Failure::invalid_input(format!("VeraCrypt keyfile {path}")))?;
        for file in files {
            ensure!(
                pool.count() < MAX_KEYFILES,
                Failure::resource_limit(format!("more than {MAX_KEYFILES} VeraCrypt keyfiles"))
            );
            let contents = veracrypt::keyfile::read(&file).with_context(|| {
                Failure::invalid_input(format!("VeraCrypt keyfile {}", file.display()))
            })?;
            pool.add(&contents).map_err(|error| {
                anyhow!(Failure::invalid_input(format!(
                    "VeraCrypt keyfile {}: {error}",
                    file.display()
                )))
            })?;
        }
    }
    let mixed = pool.apply(password.as_bytes()).map_err(map_error)?;
    Ok((mixed, pool.count()))
}

/// The application error for a reader failure.
pub(crate) fn map_error(error: veracrypt::VeraError) -> anyhow::Error {
    use veracrypt::VeraError;
    match error {
        VeraError::AuthenticationFailed => anyhow!(Failure::invalid_input(
            "no VeraCrypt or TrueCrypt header opened: check the password, PIM, keyfiles, key derivation and header"
        )),
        VeraError::Cancelled => anyhow!(Failure::cancelled("encrypted volume operation cancelled")),
        VeraError::TooSmall { .. } | VeraError::Corrupt(_) => {
            anyhow!(Failure::corrupt(error.to_string()))
        }
        VeraError::Unsupported(_) => anyhow!(Failure::unsupported(error.to_string())),
        VeraError::InvalidRequest(_) => anyhow!(Failure::invalid_input(error.to_string())),
        VeraError::Io(source) => anyhow::Error::new(source).context("VeraCrypt source read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::error::{code_of, ErrorCode};

    #[test]
    fn options_parse_from_their_wire_names() {
        let header: VeracryptHeader = serde_json::from_str(r#""backup""#).unwrap();
        assert_eq!(header, VeracryptHeader::Backup);
        let prf: VeracryptPrf = serde_json::from_str(r#""argon2id""#).unwrap();
        assert_eq!(prf.prf(), veracrypt::Prf::Argon2id);
        assert!(serde_json::from_str::<VeracryptPrf>(r#""md5""#).is_err());
    }

    #[test]
    fn keyfiles_mix_into_the_password_and_empty_ones_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("a.bin");
        let empty = directory.path().join("empty.bin");
        std::fs::write(&first, b"keyfile contents").unwrap();
        std::fs::write(&empty, b"").unwrap();
        let (plain, count) = effective_password("secret", &[]).unwrap();
        assert_eq!((plain.as_slice(), count), (&b"secret"[..], 0));
        let paths = [first.to_string_lossy().into_owned()];
        let (mixed, count) = effective_password("secret", &paths).unwrap();
        assert_eq!((mixed.len(), count), (64, 1));
        let (keyfile_only, _) = effective_password("", &paths).unwrap();
        assert_ne!(keyfile_only.as_slice(), mixed.as_slice());
        let error =
            effective_password("secret", &[empty.to_string_lossy().into_owned()]).unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
        let error = effective_password("", &[]).unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
        let error = effective_password(&"a".repeat(129), &[]).unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
        // A directory contributes each file directly inside it.
        let folder = directory.path().join("keys");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("one"), b"1").unwrap();
        std::fs::write(folder.join("two"), b"2").unwrap();
        std::fs::write(folder.join("nested").join("three"), b"3").unwrap();
        let (_, count) =
            effective_password("secret", &[folder.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(count, 2);
        // A directory with an empty file in it fails like the file itself.
        let error =
            effective_password("secret", &[directory.path().to_string_lossy().into_owned()])
                .unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
    }

    #[test]
    fn reader_errors_keep_their_kind() {
        use veracrypt::VeraError;
        let cases = [
            (VeraError::AuthenticationFailed, ErrorCode::InvalidInput),
            (VeraError::Cancelled, ErrorCode::Cancelled),
            (VeraError::Corrupt("x".into()), ErrorCode::CorruptData),
            (
                VeraError::Unsupported("x".into()),
                ErrorCode::UnsupportedFormat,
            ),
            (
                VeraError::InvalidRequest("x".into()),
                ErrorCode::InvalidInput,
            ),
            (
                VeraError::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
                ErrorCode::CorruptData,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(code_of(&map_error(error)), Some(code));
        }
    }
}
