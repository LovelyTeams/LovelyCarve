//! Session-only plaintext views over bounded, read-only evidence sources.
//!
//! BitLocker, LUKS, VeraCrypt and FileVault share this one unlock path: the
//! scheme opens its volume from a region of the evidence, and the plaintext
//! becomes a derived image that the caller attaches to the source partition
//! or keeps as evidence of its own.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, bail, ensure, Context};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::common::error::Failure;
use crate::core::evidence::crypto::bitlocker;
pub use crate::core::evidence::crypto::credential::{EncryptionScheme, UnlockCredential};
use crate::core::evidence::crypto::veracrypt_volume;
pub use crate::core::evidence::crypto::veracrypt_volume::{
    VeracryptDetails, VeracryptHeader, VeracryptPrf,
};
use crate::core::evidence::image::{DerivedImage, DiskImage, ImageInfo};
use crate::core::evidence::partitions::volume_probe::probe_volume;
use crate::core::evidence::partitions::{
    detect_filesystem, detect_partitions_with_meta, Partition, PartitionKind, SectorSource,
};

/// Sector size of every plaintext image this module builds.
const PLAINTEXT_SECTOR: usize = 512;

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UnlockTarget {
    #[default]
    NewImage,
    SourcePartition,
}

// Intentionally no Debug or Serialize: credentials must not enter diagnostics.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnlockOptions {
    #[serde(default)]
    pub target: UnlockTarget,
    pub scheme: EncryptionScheme,
    pub credential: UnlockCredential,
    pub partition_id: Option<u8>,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    /// VeraCrypt: which headers to read.
    #[serde(default)]
    pub header: VeracryptHeader,
    /// VeraCrypt: the PIM, 0 for the default.
    #[serde(default)]
    pub pim: u32,
    /// VeraCrypt: only this key derivation.
    #[serde(default)]
    pub prf: Option<VeracryptPrf>,
    /// VeraCrypt: host paths of keyfiles or keyfile directories.
    #[serde(default)]
    pub keyfiles: Vec<String>,
    pub logical_volume_index: Option<usize>,
    pub encrypted_root_plist_path: Option<String>,
}

#[derive(Serialize)]
pub struct UnlockResult {
    #[serde(skip)]
    pub(crate) read_cancellation: CancellationToken,
    pub image: ImageInfo,
    pub scheme: EncryptionScheme,
    pub cipher: String,
    pub encryption_sector_size: usize,
    pub source_offset: u64,
    pub source_length: u64,
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_volume_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_volume_index: Option<usize>,
    /// The BitLocker recovery password a VMK credential yielded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_password: Option<String>,
    /// What the VeraCrypt or TrueCrypt header that opened says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub veracrypt: Option<VeracryptDetails>,
}

pub(crate) struct EvidenceReader {
    source: Weak<DiskImage>,
    offset: u64,
    length: u64,
    position: u64,
    cancellation: Arc<Mutex<CancellationToken>>,
}

impl EvidenceReader {
    pub(crate) fn bounds(&self) -> (u64, u64) {
        (self.offset, self.length)
    }
    pub(crate) fn new(
        source: &Arc<DiskImage>,
        partition_id: Option<u8>,
        offset: Option<u64>,
        length: Option<u64>,
        cancellation: Arc<Mutex<CancellationToken>>,
    ) -> anyhow::Result<Self> {
        let (offset, length) = if let Some(id) = partition_id {
            ensure!(
                offset.is_none() && length.is_none(),
                "partitionId cannot be combined with offset/length"
            );
            partition_range(source, id)?
        } else {
            let offset = offset.unwrap_or(0);
            (
                offset,
                length.unwrap_or_else(|| source.len().saturating_sub(offset)),
            )
        };
        ensure!(
            length >= 512
                && offset
                    .checked_add(length)
                    .is_some_and(|end| end <= source.len()),
            "encrypted region is outside the image"
        );
        Ok(Self {
            source: Arc::downgrade(source),
            offset,
            length,
            position: 0,
            cancellation,
        })
    }

    /// The source, unless the operation was cancelled or the evidence closed.
    fn open_source(&self) -> std::io::Result<Arc<DiskImage>> {
        if self
            .cancellation
            .lock()
            .map_err(|_| std::io::Error::other("encrypted cancellation lock poisoned"))?
            .is_cancelled()
        {
            return Err(std::io::Error::other(
                "encrypted volume operation cancelled",
            ));
        }
        self.source
            .upgrade()
            .ok_or_else(|| std::io::Error::other("encrypted evidence closed"))
    }

    /// Fill `buffer` from region offset `offset`; short reads and
    /// synthesized (bad-sector) bytes fail rather than become ciphertext.
    fn fill(&self, source: &DiskImage, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
        let absolute = self.offset + offset;
        let filled = source
            .read_into(absolute, buffer)
            .map_err(std::io::Error::other)?;
        if filled != buffer.len() || source.has_bad_region_in_range(absolute, buffer.len() as u64) {
            return Err(std::io::Error::other(
                "encrypted evidence contains unreadable ciphertext",
            ));
        }
        Ok(())
    }
}

/// The byte range of partition `id` in `source`.
pub(super) fn partition_range(source: &DiskImage, id: u8) -> anyhow::Result<(u64, u64)> {
    let partition = source
        .partitions
        .iter()
        .find(|partition| partition.id == id)
        .context(Failure::not_found("partition not found"))?;
    Ok((
        partition
            .start_lba
            .checked_mul(source.sector_size as u64)
            .context(Failure::corrupt("partition offset overflow"))?,
        partition
            .length_lba
            .checked_mul(source.sector_size as u64)
            .context(Failure::corrupt("partition size overflow"))?,
    ))
}

impl Read for EvidenceReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let source = self.open_source()?;
        let count = buffer
            .len()
            .min(self.length.saturating_sub(self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        self.fill(&source, self.position, &mut buffer[..count])?;
        self.position += count as u64;
        Ok(count)
    }
}

/// VeraCrypt reads ciphertext at explicit offsets, from several threads.
impl veracrypt::ReadAt for EvidenceReader {
    fn size(&self) -> u64 {
        self.length
    }

    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
        let source = self.open_source()?;
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past the encrypted region",
            ));
        }
        self.fill(&source, offset, buffer)
    }
}

impl Seek for EvidenceReader {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        if self
            .cancellation
            .lock()
            .map_err(|_| std::io::Error::other("encrypted cancellation lock poisoned"))?
            .is_cancelled()
        {
            return Err(std::io::Error::other(
                "encrypted volume operation cancelled",
            ));
        }
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        self.position = u64::try_from(position)
            .map_err(|_| std::io::Error::other("invalid encrypted evidence seek"))?;
        Ok(self.position)
    }
}

/// A plaintext reader that moves a cursor, so reads take turns.
pub(super) enum PlaintextReader {
    Bitlocker {
        volume: Box<crate::core::evidence::crypto::bde_image::BdeVolume>,
        _ciphertext: Arc<bitlocker::Ciphertext>,
    },
    Luks(Box<luks::DecryptedPayload<EvidenceReader>>),
    Filevault(Box<filevault::FileVaultVolume<EvidenceReader>>),
}

pub(super) enum Plaintext {
    Locked(Mutex<PlaintextReader>),
    /// VeraCrypt reads at explicit offsets and decrypts on the caller's
    /// thread, so concurrent reads do not wait for each other.
    Veracrypt(Box<veracrypt::Volume<EvidenceReader>>),
}

impl Plaintext {
    fn locked(reader: PlaintextReader) -> Self {
        Self::Locked(Mutex::new(reader))
    }
}

struct PlaintextSource {
    plaintext: Plaintext,
    length: u64,
}

impl SectorSource for PlaintextSource {
    fn size(&self) -> u64 {
        self.length
    }

    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let length = (length as u64).min(self.length.saturating_sub(offset)) as usize;
        let mut bytes = vec![0; length];
        let offset = offset.min(self.length);
        let reader = match &self.plaintext {
            Plaintext::Veracrypt(volume) => {
                volume
                    .read_at(offset, &mut bytes)
                    .map_err(veracrypt_volume::map_error)?;
                return Ok(bytes);
            }
            Plaintext::Locked(reader) => reader,
        };
        let mut reader = reader
            .lock()
            .map_err(|_| anyhow!("decrypted reader lock poisoned"))?;
        match &mut *reader {
            PlaintextReader::Bitlocker { volume, .. } => {
                let count = volume.read_at(offset, &mut bytes)?;
                ensure!(count == length, "short BitLocker plaintext read");
            }
            PlaintextReader::Luks(reader) => reader.read_at(offset, &mut bytes)?,
            PlaintextReader::Filevault(reader) => {
                let count = reader.read_at(offset, &mut bytes)?;
                ensure!(count == length, "short FileVault plaintext read");
            }
        }
        Ok(bytes)
    }
}

/// How an opened plaintext is laid out.
#[derive(Clone, Copy)]
pub(super) enum PlaintextLayout {
    /// A disk: its partition table, or a filesystem filling it, is detected.
    Disk,
    /// Exactly one filesystem volume, whatever its contents look like.
    Volume,
}

/// The FileVault logical volume a container unlock opened.
struct LogicalVolume {
    index: usize,
    identifier: Option<String>,
    name: Option<String>,
}

/// One scheme's opened volume, before it becomes an image.
pub(super) struct OpenedVolume {
    plaintext: Plaintext,
    length: u64,
    cipher: String,
    encryption_sector_size: usize,
    pub(super) layout: PlaintextLayout,
    logical_volume: Option<LogicalVolume>,
    recovery_password: Option<String>,
    pub(super) hidden: bool,
    pub(super) veracrypt: Option<VeracryptDetails>,
    /// The source bytes the plaintext stands for, when they are not the
    /// region the reader covers (a system partition read through its drive).
    pub(super) source_range: Option<(u64, u64)>,
}

impl OpenedVolume {
    pub(super) fn new(
        plaintext: Plaintext,
        length: u64,
        cipher: String,
        encryption_sector_size: usize,
    ) -> Self {
        Self {
            plaintext,
            length,
            cipher,
            encryption_sector_size,
            layout: PlaintextLayout::Disk,
            logical_volume: None,
            recovery_password: None,
            hidden: false,
            veracrypt: None,
            source_range: None,
        }
    }
}

pub fn unlock(
    source: &Arc<DiskImage>,
    options: UnlockOptions,
    cancellation: CancellationToken,
) -> anyhow::Result<(DiskImage, UnlockResult)> {
    unlock_with_progress(source, options, cancellation, &|_, _| {})
}

/// [`unlock`], reporting `(done, total)` key derivation jobs while a
/// VeraCrypt header search runs.
pub fn unlock_with_progress(
    source: &Arc<DiskImage>,
    options: UnlockOptions,
    cancellation: CancellationToken,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> anyhow::Result<(DiskImage, UnlockResult)> {
    ensure!(
        !cancellation.is_cancelled(),
        Failure::cancelled("encrypted volume operation cancelled")
    );
    check_options(&options)?;
    let read_cancellation = Arc::new(Mutex::new(cancellation.clone()));
    // A system encryption header sits in sector 62 of the drive, outside the
    // partition it protects: read the whole drive.
    let system =
        options.scheme == EncryptionScheme::Veracrypt && options.header == VeracryptHeader::System;
    let reader = if system {
        EvidenceReader::new(
            source,
            None,
            Some(0),
            Some(source.len()),
            Arc::clone(&read_cancellation),
        )?
    } else {
        EvidenceReader::new(
            source,
            options.partition_id,
            options.offset,
            options.length,
            Arc::clone(&read_cancellation),
        )?
    };
    let (offset, source_length) = reader.bounds();
    let opened = match options.scheme {
        EncryptionScheme::Bitlocker => {
            let unlocked = bitlocker::unlock(
                source,
                offset,
                source_length,
                &options.credential,
                Arc::clone(&read_cancellation),
            )?;
            let length = unlocked.volume.size();
            let mut opened = OpenedVolume::new(
                Plaintext::locked(PlaintextReader::Bitlocker {
                    volume: Box::new(unlocked.volume),
                    _ciphertext: unlocked.ciphertext,
                }),
                length,
                unlocked.cipher,
                unlocked.sector_size,
            );
            opened.layout = PlaintextLayout::Volume;
            opened.recovery_password = unlocked.recovery_password;
            opened
        }
        EncryptionScheme::Luks => open_luks(reader, &options)?,
        EncryptionScheme::Veracrypt => {
            veracrypt_volume::open(source, reader, &options, &cancellation, progress)?
        }
        EncryptionScheme::Filevault => open_filevault(reader, &options, &cancellation)?,
    };
    if cancellation.is_cancelled() {
        bail!(Failure::cancelled("encrypted volume operation cancelled"));
    }
    let OpenedVolume {
        plaintext,
        length,
        cipher,
        encryption_sector_size,
        layout,
        logical_volume,
        recovery_password,
        hidden,
        veracrypt,
        source_range,
    } = opened;
    let (offset, source_length) = source_range.unwrap_or((offset, source_length));
    let plaintext = Arc::new(PlaintextSource { plaintext, length });
    let (partitions, table_scheme, disk_id) = match layout {
        PlaintextLayout::Disk => {
            let detected = detect_partitions_with_meta(plaintext.as_ref(), PLAINTEXT_SECTOR)?;
            (detected.partitions, detected.scheme, detected.disk_id)
        }
        PlaintextLayout::Volume => (vec![single_volume(plaintext.as_ref())?], None, None),
    };
    if options.scheme == EncryptionScheme::Filevault {
        ensure!(
            partitions.iter().any(|partition| matches!(
                partition.kind,
                PartitionKind::HfsPlus | PartitionKind::HfsX
            )),
            "FileVault plaintext does not contain a recognized HFS+ volume"
        );
    }
    let mut image =
        DiskImage::from_assembled("decrypted".into(), plaintext, partitions, PLAINTEXT_SECTOR)?;
    image.path = format!("decrypted://{}", image.id).into();
    if let (Some(name), Some(partition)) = (
        logical_volume
            .as_ref()
            .and_then(|volume| volume.name.clone()),
        image.partitions.first_mut(),
    ) {
        partition.label = Some(name);
    }
    image.derived = Some(DerivedImage::Decrypted {
        parent: source.id.clone(),
    });
    image.table_scheme = table_scheme;
    image.disk_id = disk_id;
    ensure!(
        !cancellation.is_cancelled(),
        Failure::cancelled("encrypted volume operation cancelled")
    );
    // Completed views have their own lifetime; a late unlock cancel must not relock them.
    let lifetime = CancellationToken::new();
    *read_cancellation
        .lock()
        .map_err(|_| anyhow!("encrypted cancellation lock poisoned"))? = lifetime.clone();
    let (logical_volume_index, logical_volume_identifier) = match logical_volume {
        Some(volume) => (Some(volume.index), volume.identifier),
        None => (None, None),
    };
    let result = UnlockResult {
        read_cancellation: lifetime,
        image: image.info(),
        scheme: options.scheme,
        cipher,
        encryption_sector_size,
        source_offset: offset,
        source_length,
        hidden,
        volume_key: None,
        logical_volume_identifier,
        logical_volume_index,
        recovery_password,
        veracrypt,
    };
    Ok((image, result))
}

/// Options that cannot apply to the chosen scheme fail before any read.
fn check_options(options: &UnlockOptions) -> anyhow::Result<()> {
    let scheme = options.scheme;
    ensure!(
        options.credential.opens(scheme),
        Failure::invalid_input(format!(
            "a {} does not open {} volumes",
            options.credential.label(),
            scheme.label()
        ))
    );
    if let UnlockCredential::Password { password } = &options.credential {
        // A VeraCrypt volume may open with keyfiles alone.
        ensure!(
            !password.is_empty()
                || (scheme == EncryptionScheme::Veracrypt && !options.keyfiles.is_empty()),
            Failure::invalid_input("the password is empty")
        );
        ensure!(
            password.len() <= 4096,
            Failure::invalid_input("password exceeds 4096 bytes")
        );
    }
    ensure!(
        scheme == EncryptionScheme::Veracrypt
            || (options.header == VeracryptHeader::Auto
                && options.pim == 0
                && options.prf.is_none()
                && options.keyfiles.is_empty()),
        Failure::invalid_input(
            "headers, PIM, key derivations and keyfiles apply only to VeraCrypt"
        )
    );
    if options.header == VeracryptHeader::System {
        ensure!(
            options.offset.is_none() && options.length.is_none(),
            Failure::invalid_input("a system encryption header is read from the whole drive; choose a partition or the whole image")
        );
        ensure!(
            options.partition_id.is_some() || options.target == UnlockTarget::NewImage,
            Failure::invalid_input("a system-encrypted drive opens as new evidence")
        );
        ensure!(
            options.pim <= veracrypt::MAX_BOOT_PIM,
            Failure::invalid_input(format!(
                "system encryption PIM is at most {}",
                veracrypt::MAX_BOOT_PIM
            ))
        );
    }
    ensure!(
        options.pim <= veracrypt::MAX_PIM,
        Failure::invalid_input(format!("PIM is at most {}", veracrypt::MAX_PIM))
    );
    ensure!(
        options.keyfiles.len() <= veracrypt_volume::MAX_KEYFILES,
        Failure::resource_limit(format!(
            "more than {} VeraCrypt keyfiles",
            veracrypt_volume::MAX_KEYFILES
        ))
    );
    ensure!(
        scheme == EncryptionScheme::Filevault
            || (options.logical_volume_index.is_none()
                && options.encrypted_root_plist_path.is_none()),
        Failure::invalid_input(
            "logical volume / encryption context options apply only to FileVault"
        )
    );
    Ok(())
}

/// The error for a credential the scheme's opener cannot use; `check_options`
/// rejects these first.
pub(super) fn wrong_credential(
    credential: &UnlockCredential,
    scheme: EncryptionScheme,
) -> anyhow::Error {
    anyhow!(Failure::invalid_input(format!(
        "a {} does not open {} volumes",
        credential.label(),
        scheme.label()
    )))
}

fn open_luks(mut reader: EvidenceReader, options: &UnlockOptions) -> anyhow::Result<OpenedVolume> {
    let (_, length) = reader.bounds();
    crate::core::evidence::crypto::luks_metadata::validate(&mut reader, length)?;
    reader.rewind()?;
    let volume = match &options.credential {
        UnlockCredential::Password { password } => {
            luks::LuksVolume::unlock_with_passphrase(reader, password.as_bytes())?
        }
        UnlockCredential::MasterKey { key_hex, .. } => {
            ensure!(
                key_hex.len() <= 512
                    && key_hex.len().is_multiple_of(2)
                    && !key_hex.is_empty()
                    && key_hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
                Failure::invalid_input("invalid LUKS master key hex")
            );
            let key = zeroize::Zeroizing::new(
                hex::decode(key_hex)
                    .context(Failure::invalid_input("invalid LUKS master key hex"))?,
            );
            luks::LuksVolume::unlock_with_master_key(reader, &key)?
        }
        credential @ (UnlockCredential::RecoveryPassword { .. }
        | UnlockCredential::StartupKey { .. }
        | UnlockCredential::Vmk { .. }
        | UnlockCredential::ClearKey) => {
            return Err(wrong_credential(credential, EncryptionScheme::Luks))
        }
    };
    let size = volume.payload_size();
    let info = volume.info();
    ensure!(
        size > 0 && size % info.sector_size as u64 == 0,
        Failure::corrupt("invalid LUKS payload geometry")
    );
    let cipher = format!(
        "LUKS{} {}-{}",
        info.version, info.cipher_name, info.cipher_mode
    );
    let sector = info.sector_size;
    Ok(OpenedVolume::new(
        Plaintext::locked(PlaintextReader::Luks(Box::new(volume))),
        size,
        cipher,
        sector,
    ))
}

fn open_filevault(
    reader: EvidenceReader,
    options: &UnlockOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<OpenedVolume> {
    let external =
        super::filevault::read_external_context(options.encrypted_root_plist_path.as_deref())?;
    let container = filevault::storage::Container::open(reader, external.as_deref())?;
    ensure!(
        container.volumes().count() == 1 || options.target == UnlockTarget::NewImage,
        "multiple FileVault logical volumes require independent newImage sessions"
    );
    let index = match options.logical_volume_index {
        Some(index) => index,
        None if container.volumes().count() == 1 => 0,
        None => bail!("FileVault contains multiple logical volumes; choose logicalVolumeIndex"),
    };
    let mut volume = match &options.credential {
        UnlockCredential::Password { password } => {
            container
                .unlock_with_password_cancellable(index, password, || cancellation.is_cancelled())?
        }
        UnlockCredential::MasterKey { key_hex, .. } => {
            ensure!(
                matches!(key_hex.len(), 32 | 64)
                    && key_hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
                Failure::invalid_input(
                    "FileVault recovered key must contain 16 or 32 bytes in hex"
                )
            );
            let key = zeroize::Zeroizing::new(hex::decode(key_hex)?);
            container.unlock_with_master_key(index, &key)?
        }
        credential @ (UnlockCredential::RecoveryPassword { .. }
        | UnlockCredential::StartupKey { .. }
        | UnlockCredential::Vmk { .. }
        | UnlockCredential::ClearKey) => {
            return Err(wrong_credential(credential, EncryptionScheme::Filevault))
        }
    };
    hfsplus::VolumeHeader::parse(&mut volume)
        .context("FileVault plaintext HFS+ header validation")?;
    let logical_volume = LogicalVolume {
        index,
        identifier: volume.info().lv_identifier.clone(),
        name: volume.info().lv_name.clone(),
    };
    let length = volume.size();
    let mut opened = OpenedVolume::new(
        Plaintext::locked(PlaintextReader::Filevault(Box::new(volume))),
        length,
        "FileVault 2 AES-XTS-128".to_string(),
        512,
    );
    opened.logical_volume = Some(logical_volume);
    Ok(opened)
}

/// The one partition of a plaintext that is a single volume: all of it, typed
/// the way a partition of that filesystem is (`Unknown` when none matches).
fn single_volume(plaintext: &dyn SectorSource) -> anyhow::Result<Partition> {
    let kind = match probe_volume(plaintext, PLAINTEXT_SECTOR)? {
        Some(hit) if hit.relative_offset == 0 => hit.kind,
        Some(_) | None => detect_filesystem(&plaintext.read(0, 4096)?, PartitionKind::Unknown),
    };
    Ok(Partition {
        id: 0,
        kind,
        start_lba: 0,
        length_lba: plaintext.size() / PLAINTEXT_SECTOR as u64,
        label: None,
        unique_guid: None,
        amiga_geometry: None,
    })
}
