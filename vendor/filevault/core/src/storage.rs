//! Read-only CoreStorage container: checksum-validated metadata, multiple logical volumes,
//! password/recovery protectors and recovered keys. APFS encryption is a separate format.
use crate::{
    read::{le_u32, le_u64},
    storage_metadata::{self as metadata, invalid},
    storage_plist::Value,
    volume::{DecryptedVolume, Segment},
    EncryptionContext, FileVaultError, FileVaultInfo, FileVaultVolume, LogicalVolumeInfo,
    Protector, ProtectorKind, VolumeHeader,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek},
};

/// Public logical-volume inventory contains no unwrapped keys or credentials.
#[derive(Debug, Clone)]
pub struct LogicalVolume {
    pub index: usize,
    pub info: FileVaultInfo,
    pub segments: Vec<Segment>,
    pub object_identifier: u64,
    pub transaction: u64,
}
#[derive(Clone, Copy)]
enum KeyValidation {
    WrappedIntegrity,
    RecoveredHeader,
}
type ContextInventory = (Vec<EncryptionContext>, Vec<Protector>, Option<String>);

struct Configuration {
    volume: LogicalVolume,
    contexts: Vec<EncryptionContext>,
    family: [u8; 16],
}

/// A validated bounded CoreStorage partition. Metadata-copy provenance is retained.
pub struct Container<R: Read + Seek> {
    reader: R,
    header: VolumeHeader,
    configurations: Vec<Configuration>,
    pub volume_group_name: Option<String>,
    pub header_offset: u64,
    pub metadata_pointer_offset: u64,
    pub encrypted_metadata_offset: u64,
}
fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::string).map(str::to_owned)
}
fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::integer)
}
fn bytes<const N: usize>(value: &[u8], offset: usize) -> Result<[u8; N], FileVaultError> {
    value
        .get(
            offset
                ..offset
                    .checked_add(N)
                    .ok_or_else(|| invalid("wrapped key offset overflow"))?,
        )
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid("wrapped key structure truncated"))
}
fn contexts(root: &Value) -> Result<ContextInventory, FileVaultError> {
    let value = root
        .get("com.apple.corestorage.lvf.encryption.context")
        .unwrap_or(root);
    let users = value.get("CryptoUsers").map_or(&[][..], Value::array);
    if users.len() > 64 {
        return Err(invalid("crypto user count budget"));
    }
    let status = value
        .get("ConversionInfo")
        .and_then(|info| text(info, "ConversionStatus"))
        .or_else(|| text(value, "ConversionStatus"));
    let mut protectors = Vec::new();
    let mut result = Vec::new();
    let active_context = number(root, "DefaultEncryptionContext");
    let keys = value.get("WrappedVolumeKeys").map_or(&[][..], Value::array);
    if keys.len() > 64 {
        return Err(invalid("wrapped volume key count budget"));
    }
    for user in users {
        let user_type = u32::try_from(number(user, "UserType").unwrap_or(0))
            .map_err(|_| invalid("crypto user type"))?;
        protectors.push(Protector {
            user_type,
            kind: ProtectorKind::from_user_type(user_type),
        });
        let Some(passphrase) = user.get("PassphraseWrappedKEKStruct").and_then(Value::data) else {
            continue;
        };
        let identifier = text(user, "KeyEncryptingKeyIdent");
        let candidates: Vec<&Value> = if keys.is_empty() {
            vec![user]
        } else {
            keys.iter().collect()
        };
        for key in candidates {
            if active_context.is_some() && number(key, "VolumeKeyIndex") != active_context {
                continue;
            }
            if key.get("BlockAlgorithm").and_then(Value::string) != Some("AES-XTS") {
                continue;
            }
            if identifier.is_some() && text(key, "KeyEncryptingKeyIdent") != identifier {
                continue;
            }
            let wrapped = key
                .get("KEKWrappedVolumeKeyStruct")
                .and_then(Value::data)
                .ok_or_else(|| invalid("AES-XTS wrapped volume key missing"))?;
            result.push(EncryptionContext {
                salt: bytes(passphrase, 8)?,
                iterations: u32::from_le_bytes(bytes(passphrase, 168)?),
                wrapped_kek: bytes(passphrase, 32)?,
                wrapped_vmk: bytes(wrapped, 8)?,
                protectors: Vec::new(),
                conversion_status: status.clone(),
            });
        }
    }
    if result.len() > 64 {
        return Err(invalid("key derivation candidate count budget"));
    }
    Ok((result, protectors, status))
}
fn segments(
    objects: &metadata::Objects,
    object: &metadata::Object,
    header: &VolumeHeader,
) -> Result<Vec<Segment>, FileVaultError> {
    let map_identifier = le_u64(&object.bytes, 72);
    let map = metadata::object(objects, 0x305, map_identifier, object.transaction)?;
    let count = le_u32(&map.bytes, 64) as usize;
    if count == 0 || count > (map.bytes.len() - 72) / 40 {
        return Err(invalid("logical extent count or record length"));
    }
    let base_identifier = le_u64(&object.bytes, 112);
    let base = if base_identifier == 0 {
        0
    } else {
        let base = metadata::object(objects, 0x505, base_identifier, object.transaction)?;
        if le_u32(&base.bytes, 64) != 1 {
            return Err(invalid("multi-entry physical base descriptor unsupported"));
        }
        let address = le_u64(&base.bytes, 80);
        if address >> 48 != 0 {
            return Err(invalid("logical volume requires another physical member"));
        }
        address
    };
    let block_size = u64::from(header.block_size);
    let mut result = Vec::new();
    for index in 0..count {
        let entry = &map.bytes[72 + index * 40..72 + (index + 1) * 40];
        let physical = le_u64(entry, 32);
        if physical >> 48 != 0 {
            return Err(invalid("logical extent requires another physical member"));
        }
        if !matches!(le_u32(entry, 20), 0 | 0x0040_0000) {
            return Err(invalid("unsupported logical extent flags"));
        }
        result.push(Segment {
            logical_offset: le_u64(entry, 8)
                .checked_mul(block_size)
                .ok_or_else(|| invalid("logical extent offset overflow"))?,
            physical_offset: physical
                .checked_add(base)
                .and_then(|block| block.checked_mul(block_size))
                .ok_or_else(|| invalid("physical extent offset overflow"))?,
            length: u64::from(le_u32(entry, 16))
                .checked_mul(block_size)
                .ok_or_else(|| invalid("extent length overflow"))?,
        });
    }
    Ok(result)
}
impl<R: Read + Seek> Container<R> {
    /// Read checksum-validated metadata and inventory all current logical-volume objects.
    /// An optional external context may be plaintext XML or EncryptedRoot.plist.wipekey.
    pub fn open(mut reader: R, external_context: Option<&[u8]>) -> Result<Self, FileVaultError> {
        let (header, header_offset) = metadata::read_header(&mut reader)?;
        let (objects, metadata_pointer_offset, encrypted_metadata_offset, volume_group_name) =
            metadata::read_objects(&mut reader, &header)?;
        let external = if let Some(input) = external_context {
            if input.len() < 16 || input.len() > 16 * 1024 * 1024 {
                return Err(invalid("external encryption context length"));
            }
            let data = if input.starts_with(b"<") || input.starts_with(b"\xef\xbb\xbf") {
                input.to_vec()
            } else {
                let mut data = input.to_vec();
                crate::xts::decrypt_units(
                    &mut data,
                    &header.physical_volume_identifier,
                    &[0; 16],
                    input.len(),
                    0,
                );
                data
            };
            Some(crate::storage_plist::parse(&data)?)
        } else {
            None
        };
        let transaction_bound = objects
            .get(&(0x13, 0))
            .and_then(|versions| versions.iter().map(|object| object.transaction).max())
            .unwrap_or(u64::MAX);
        let mut configurations = BTreeMap::new();
        for ((kind, identifier), versions) in &objects {
            if *kind != 0x1a {
                continue;
            }
            let Some(object) = versions
                .iter()
                .filter(|object| object.transaction <= transaction_bound)
                .max_by_key(|object| object.transaction)
            else {
                continue;
            };
            let plist = metadata::plist(&objects, object)?;
            let family_uuid = text(&plist, "com.apple.corestorage.lv.familyUUID")
                .ok_or_else(|| invalid("logical volume family UUID missing"))?;
            let family = uuid::Uuid::parse_str(&family_uuid)
                .map_err(|_| invalid("invalid logical family UUID"))?;
            let lv = LogicalVolumeInfo {
                size: number(&plist, "com.apple.corestorage.lv.size")
                    .ok_or_else(|| invalid("logical volume size missing"))?,
                family_uuid,
                lv_identifier: text(&plist, "com.apple.corestorage.lv.uuid"),
                name: text(&plist, "com.apple.corestorage.lv.name"),
            };
            if let Some(group) = text(&plist, "com.apple.corestorage.lv.groupUUID") {
                if self::uuid_bytes(&group)? != header.logical_volume_group_identifier {
                    return Err(invalid(
                        "logical volume belongs to another physical header group",
                    ));
                }
            }
            let family_identifier = le_u64(&object.bytes, 64);
            let context = if let Some(external) = &external {
                external.clone()
            } else {
                metadata::plist(
                    &objects,
                    metadata::object(&objects, 0x19, family_identifier, object.transaction)?,
                )?
            };
            if let Some(context_family) = text(&context, "com.apple.corestorage.lvf.uuid") {
                if uuid::Uuid::parse_str(&context_family).ok() != Some(family) {
                    return Err(invalid("encryption context belongs to another family"));
                }
            }
            let (contexts, protectors, status) = self::contexts(&context)?;
            let fallback_context = contexts
                .iter()
                .min_by_key(|context| context.iterations)
                .cloned()
                .unwrap_or(EncryptionContext {
                    salt: [0; 16],
                    iterations: 0,
                    wrapped_kek: [0; 24],
                    wrapped_vmk: [0; 24],
                    protectors: Vec::new(),
                    conversion_status: None,
                });
            let mut info = crate::build_info(&header, &fallback_context, &lv, lv.size);
            info.protectors = protectors;
            info.conversion_status = status;
            let segments = self::segments(&objects, object, &header)?;
            // Validate the allocation plan without reading/decrypting any plaintext bytes.
            DecryptedVolume::with_segments(
                std::io::Cursor::new(Vec::<u8>::new()),
                crate::unlock::keys_from_master([0; 16], family.as_bytes()),
                segments.clone(),
                lv.size,
                header.physical_volume_size,
            )?;
            let identity = if let Some(identifier) = &lv.lv_identifier {
                uuid::Uuid::parse_str(identifier)
                    .map_err(|_| invalid("invalid logical volume UUID"))?
                    .to_string()
            } else {
                format!("object:{identifier}")
            };
            let configuration = Configuration {
                family: *family.as_bytes(),
                contexts,
                volume: LogicalVolume {
                    index: 0,
                    info,
                    segments,
                    object_identifier: *identifier,
                    transaction: object.transaction,
                },
            };
            if configurations.insert(identity, configuration).is_some() {
                return Err(invalid("duplicate logical volume UUID"));
            }
        }
        if configurations.is_empty() {
            return Err(invalid("no logical volumes in metadata"));
        }
        let mut configurations: Vec<_> = configurations.into_values().collect();
        for (index, configuration) in configurations.iter_mut().enumerate() {
            configuration.volume.index = index;
        }
        Ok(Self {
            reader,
            header,
            configurations,
            volume_group_name,
            header_offset,
            metadata_pointer_offset,
            encrypted_metadata_offset,
        })
    }
    /// Physical and volume-group identity come from the selected header copy.
    pub fn header(&self) -> &VolumeHeader {
        &self.header
    }
    /// Enumerate volume identities and physical mappings without a password.
    pub fn volumes(&self) -> impl Iterator<Item = &LogicalVolume> {
        self.configurations
            .iter()
            .map(|configuration| &configuration.volume)
    }
    fn configuration(&self, index: usize) -> Result<&Configuration, FileVaultError> {
        self.configurations
            .get(index)
            .ok_or_else(|| invalid("logical volume index outside inventory"))
    }
    /// Try each password/recovery protector associated with the selected family.
    pub fn unlock_with_password(
        self,
        index: usize,
        password: &str,
    ) -> Result<FileVaultVolume<R>, FileVaultError> {
        self.unlock_with_password_cancellable(index, password, || false)
    }
    /// Poll cancellation between bounded PBKDF2 attempts; one derivation is atomic.
    pub fn unlock_with_password_cancellable(
        self,
        index: usize,
        password: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<FileVaultVolume<R>, FileVaultError> {
        let configuration = self.configuration(index)?;
        if configuration.volume.info.conversion_status.as_deref() != Some("Complete") {
            return Err(invalid(
                "partial, converting or unknown encryption state unsupported",
            ));
        }
        let mut recovered = None;
        let mut rounds = 0u32;
        for context in &configuration.contexts {
            if cancelled() {
                return Err(invalid("FileVault unlock cancelled"));
            }
            rounds = rounds
                .checked_add(context.iterations)
                .filter(|total| *total <= 16_000_000)
                .ok_or_else(|| invalid("total PBKDF2 attempt budget"))?;
            match crate::unlock::derive_volume_keys(password, context, &configuration.family) {
                Ok(keys) => {
                    recovered = Some(keys);
                    break;
                }
                Err(FileVaultError::KeyUnwrap { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let keys = recovered.ok_or(FileVaultError::KeyUnwrap {
            what: "all password/recovery protectors",
        })?;
        self.unlock_keys(index, keys, KeyValidation::WrappedIntegrity)
    }
    /// Accept a recovered 16-byte VMK or a 32-byte XTS pair; verify an HFS+ header before publishing.
    pub fn unlock_with_master_key(
        self,
        index: usize,
        key: &[u8],
    ) -> Result<FileVaultVolume<R>, FileVaultError> {
        let configuration = self.configuration(index)?;
        let mut keys = crate::unlock::keys_from_master(bytes(key, 0)?, &configuration.family);
        match key.len() {
            16 => {}
            32 => keys.tweak_key = bytes(key, 16)?,
            _ => return Err(invalid("FileVault key must contain 16 or 32 bytes")),
        }
        self.unlock_keys(index, keys, KeyValidation::RecoveredHeader)
    }
    fn unlock_keys(
        self,
        index: usize,
        keys: crate::unlock::VolumeKeys,
        validation: KeyValidation,
    ) -> Result<FileVaultVolume<R>, FileVaultError> {
        let configuration = self.configuration(index)?;
        if configuration.volume.info.conversion_status.as_deref() != Some("Complete") {
            return Err(invalid("unsupported encryption conversion state"));
        }
        let info = configuration.volume.info.clone();
        let segments = configuration.volume.segments.clone();
        let mut decrypted = DecryptedVolume::with_segments(
            self.reader,
            keys,
            segments,
            info.lv_size,
            self.header.physical_volume_size,
        )?;
        if matches!(validation, KeyValidation::RecoveredHeader) {
            let mut header = [0; 512];
            if decrypted.read_at(1024, &mut header)? != header.len()
                || !(header[..4] == [b'H', b'+', 0, 4] || header[..4] == [b'H', b'X', 0, 5])
            {
                return Err(invalid("recovered key did not decrypt a valid HFS+ header"));
            }
            let block_size = u32::from_be_bytes([header[40], header[41], header[42], header[43]]);
            let total_blocks = u32::from_be_bytes([header[44], header[45], header[46], header[47]]);
            if !block_size.is_power_of_two()
                || block_size < 512
                || total_blocks == 0
                || u64::from(block_size) * u64::from(total_blocks) > info.lv_size
            {
                return Err(invalid("recovered key did not decrypt a valid HFS+ header"));
            }
        }
        Ok(FileVaultVolume { decrypted, info })
    }
}

/// Check the fixed physical-header signature, version and CRC before recognizing a container.
#[must_use]
pub fn check_volume_signature(bytes: &[u8]) -> bool {
    bytes.get(..512).is_some_and(|header| {
        VolumeHeader::parse(header).is_ok()
            && metadata::verify(header).is_ok()
            && crate::read::le_u16(header, 10) == 0x10
            && crate::read::le_u32(header, 90) == 1
    })
}

#[cfg(test)]
#[path = "storage/tests.rs"]
mod tests;

fn uuid_bytes(text: &str) -> Result<[u8; 16], FileVaultError> {
    Ok(*uuid::Uuid::parse_str(text)
        .map_err(|_| invalid("invalid volume group UUID"))?
        .as_bytes())
}
