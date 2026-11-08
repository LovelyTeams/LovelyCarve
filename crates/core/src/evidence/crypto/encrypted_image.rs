//! Session-only plaintext views over bounded, read-only evidence sources.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, bail, ensure, Context};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

use crate::evidence::image::{DerivedImage, DiskImage, ImageInfo};
use crate::evidence::partitions::{detect_partitions_with_meta, SectorSource};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionScheme {
    Luks,
    Veracrypt,
    Filevault,
}

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
    #[serde(default)]
    pub password: String,
    pub master_key_hex: Option<String>,
    pub partition_id: Option<u8>,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub pim: u32,
    pub logical_volume_index: Option<usize>,
    pub encrypted_root_plist_path: Option<String>,
}

impl Drop for UnlockOptions {
    fn drop(&mut self) {
        self.password.zeroize();
        if let Some(key) = &mut self.master_key_hex {
            key.zeroize();
        }
    }
}

#[derive(Serialize)]
pub struct UnlockResult {
    #[serde(skip)]
    pub read_cancellation: CancellationToken,
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
            let partition = source
                .partitions
                .iter()
                .find(|partition| partition.id == id)
                .context("partition not found")?;
            (
                partition
                    .start_lba
                    .checked_mul(source.sector_size as u64)
                    .context("partition offset overflow")?,
                partition
                    .length_lba
                    .checked_mul(source.sector_size as u64)
                    .context("partition size overflow")?,
            )
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
}

impl Read for EvidenceReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
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
        let source = self
            .source
            .upgrade()
            .ok_or_else(|| std::io::Error::other("encrypted evidence closed"))?;
        let count = buffer
            .len()
            .min(self.length.saturating_sub(self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        let offset = self.offset + self.position;
        let bytes = source
            .read_range(offset, count)
            .map_err(std::io::Error::other)?;
        if bytes.len() != count
            || source.bad_regions().iter().any(|region| {
                region.offset < offset + count as u64
                    && region.offset.saturating_add(region.len) > offset
            })
        {
            return Err(std::io::Error::other(
                "encrypted evidence contains unreadable ciphertext",
            ));
        }
        buffer[..count].copy_from_slice(&bytes);
        self.position += count as u64;
        Ok(count)
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

enum PlaintextReader {
    Luks(Box<luks::DecryptedPayload<EvidenceReader>>),
    Veracrypt(veracrypt::DecryptedVolume<EvidenceReader>),
    Filevault(filevault::FileVaultVolume<EvidenceReader>),
}

struct PlaintextSource {
    reader: Mutex<PlaintextReader>,
    length: u64,
}

impl SectorSource for PlaintextSource {
    fn size(&self) -> u64 {
        self.length
    }

    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let length = (length as u64).min(self.length.saturating_sub(offset)) as usize;
        let mut bytes = vec![0; length];
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| anyhow!("decrypted reader lock poisoned"))?;
        match &mut *reader {
            PlaintextReader::Luks(reader) => reader.read_at(offset.min(self.length), &mut bytes)?,
            PlaintextReader::Veracrypt(reader) => {
                reader.read_at(offset.min(self.length), &mut bytes)?
            }
            PlaintextReader::Filevault(reader) => {
                let count = reader.read_at(offset.min(self.length), &mut bytes)?;
                ensure!(count == length, "short FileVault plaintext read");
            }
        }
        Ok(bytes)
    }
}

pub fn unlock(
    source: &Arc<DiskImage>,
    options: UnlockOptions,
    cancellation: CancellationToken,
) -> anyhow::Result<(DiskImage, UnlockResult)> {
    ensure!(
        !cancellation.is_cancelled(),
        "encrypted volume operation cancelled"
    );
    ensure!(
        options.master_key_hex.is_some() != !options.password.is_empty(),
        "provide exactly one password or master key"
    );
    ensure!(
        options.master_key_hex.is_none()
            || matches!(
                options.scheme,
                EncryptionScheme::Luks | EncryptionScheme::Filevault
            ),
        "master keys apply only to LUKS or FileVault"
    );
    ensure!(
        options.password.len() <= 4096,
        "password exceeds 4096 bytes"
    );
    ensure!(
        options.pim <= 1000,
        "PIM above 1000 exceeds the supported derivation budget"
    );
    ensure!(
        options.scheme == EncryptionScheme::Filevault
            || (options.logical_volume_index.is_none()
                && options.encrypted_root_plist_path.is_none()),
        "logical volume / encryption context options apply only to FileVault"
    );
    let read_cancellation = Arc::new(Mutex::new(cancellation.clone()));
    let mut reader = EvidenceReader::new(
        source,
        options.partition_id,
        options.offset,
        options.length,
        Arc::clone(&read_cancellation),
    )?;
    let offset = reader.offset;
    let length = reader.length;
    let source_length = length;
    let mut logical_volume_identifier = None;
    let mut logical_volume_index = None;
    let mut logical_volume_name = None;
    let (reader, length, cipher, encryption_sector_size) = match options.scheme {
        EncryptionScheme::Luks => {
            ensure!(
                !options.hidden && options.pim == 0,
                "hidden/PIM options apply only to VeraCrypt"
            );
            crate::evidence::crypto::luks_metadata::validate(&mut reader, length)?;
            reader.rewind()?;
            let volume = if let Some(key) = &options.master_key_hex {
                ensure!(
                    key.len() <= 512
                        && key.len().is_multiple_of(2)
                        && !key.is_empty()
                        && key.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "invalid LUKS master key hex"
                );
                let key = zeroize::Zeroizing::new(
                    hex::decode(key).context("invalid LUKS master key hex")?,
                );
                luks::LuksVolume::unlock_with_master_key(reader, &key)?
            } else {
                luks::LuksVolume::unlock_with_passphrase(reader, options.password.as_bytes())?
            };
            let size = volume.payload_size();
            let info = volume.info();
            ensure!(
                size > 0 && size % info.sector_size as u64 == 0,
                "invalid LUKS payload geometry"
            );
            let cipher = format!(
                "LUKS{} {}-{}",
                info.version, info.cipher_name, info.cipher_mode
            );
            let sector = info.sector_size;
            (
                PlaintextReader::Luks(Box::new(volume)),
                size,
                cipher,
                sector,
            )
        }
        EncryptionScheme::Filevault => {
            ensure!(
                !options.hidden && options.pim == 0,
                "hidden/PIM apply only to VeraCrypt"
            );
            let external = super::filevault::read_external_context(
                options.encrypted_root_plist_path.as_deref(),
            )?;
            let container = filevault::storage::Container::open(reader, external.as_deref())?;
            ensure!(
                container.volumes().count() == 1 || options.target == UnlockTarget::NewImage,
                "multiple FileVault logical volumes require independent newImage sessions"
            );
            let index = match options.logical_volume_index {
                Some(index) => index,
                None if container.volumes().count() == 1 => 0,
                None => {
                    bail!("FileVault contains multiple logical volumes; choose logicalVolumeIndex")
                }
            };
            let mut volume = if let Some(key) = &options.master_key_hex {
                ensure!(
                    matches!(key.len(), 32 | 64)
                        && key.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "FileVault recovered key must contain 16 or 32 bytes in hex"
                );
                let key = zeroize::Zeroizing::new(hex::decode(key)?);
                container.unlock_with_master_key(index, &key)?
            } else {
                container.unlock_with_password_cancellable(index, &options.password, || {
                    cancellation.is_cancelled()
                })?
            };
            hfsplus::VolumeHeader::parse(&mut volume)
                .context("FileVault plaintext HFS+ header validation")?;
            logical_volume_identifier = volume.info().lv_identifier.clone();
            logical_volume_index = Some(index);
            logical_volume_name = volume.info().lv_name.clone();
            let size = volume.size();
            (
                PlaintextReader::Filevault(volume),
                size,
                "FileVault 2 AES-XTS-128".to_string(),
                512,
            )
        }
        EncryptionScheme::Veracrypt => {
            let volume = if options.hidden {
                veracrypt::VeraVolume::unlock_hidden_with_pim(
                    reader,
                    options.password.as_bytes(),
                    options.pim,
                )?
            } else {
                veracrypt::VeraVolume::unlock_with_pim(
                    reader,
                    options.password.as_bytes(),
                    options.pim,
                )?
            };
            let size = volume.data_size();
            let cipher = format!(
                "{} / {}",
                volume.info().cipher_display(),
                volume.info().prf.name()
            );
            (PlaintextReader::Veracrypt(volume), size, cipher, 512)
        }
    };
    if cancellation.is_cancelled() {
        bail!("encrypted volume operation cancelled");
    }
    let plaintext = Arc::new(PlaintextSource {
        reader: Mutex::new(reader),
        length,
    });
    let detected = detect_partitions_with_meta(plaintext.as_ref(), 512)?;
    if options.scheme == EncryptionScheme::Filevault {
        ensure!(
            detected.partitions.iter().any(|partition| matches!(
                partition.kind,
                crate::evidence::partitions::PartitionKind::HfsPlus
                    | crate::evidence::partitions::PartitionKind::HfsX
            )),
            "FileVault plaintext does not contain a recognized HFS+ volume"
        );
    }
    let mut image =
        DiskImage::from_assembled("decrypted".into(), plaintext, detected.partitions, 512)?;
    image.path = format!("decrypted://{}", image.id).into();
    if let (Some(name), Some(partition)) = (logical_volume_name, image.partitions.first_mut()) {
        partition.label = Some(name);
    }
    image.derived = Some(DerivedImage::Decrypted {
        parent: source.id.clone(),
    });
    image.table_scheme = detected.scheme;
    image.disk_id = detected.disk_id;
    ensure!(
        !cancellation.is_cancelled(),
        "encrypted volume operation cancelled"
    );
    // Completed views have their own lifetime; a late unlock cancel must not relock them.
    let lifetime = CancellationToken::new();
    *read_cancellation
        .lock()
        .map_err(|_| anyhow!("encrypted cancellation lock poisoned"))? = lifetime.clone();
    let result = UnlockResult {
        read_cancellation: lifetime,
        image: image.info(),
        scheme: options.scheme,
        cipher,
        encryption_sector_size,
        source_offset: offset,
        source_length,
        hidden: options.hidden,
        volume_key: None,
        logical_volume_identifier,
        logical_volume_index,
    };
    Ok((image, result))
}
