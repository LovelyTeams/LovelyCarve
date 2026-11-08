//! `Manifest.plist`, `Status.plist` and `Info.plist` — what the backup says
//! about itself and the device it came from.

use std::path::Path;
use std::time::SystemTime;

use crate::error::Error;

/// Device and backup metadata, gathered from all three property lists.
///
/// Every field is optional: these plists vary by iOS version, and an absent
/// field is reported absent rather than filled with a plausible default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackupMetadata {
    /// `Manifest.plist` `Version` — the backup format version, e.g. `"10.0"`.
    pub version: Option<String>,
    /// Whether `Manifest.plist` declares the backup encrypted.
    pub is_encrypted: bool,
    /// Whether a device passcode was set when the backup was taken.
    pub was_passcode_set: bool,
    /// When the backup was written.
    pub date: Option<SystemTime>,
    /// iOS version, e.g. `"26.2"`.
    pub product_version: Option<String>,
    /// Hardware model, e.g. `"iPhone13,3"`.
    pub product_type: Option<String>,
    /// iOS build, e.g. `"23C5000"`.
    pub build_version: Option<String>,
    /// The device's name as the user set it.
    pub device_name: Option<String>,
    /// Hardware serial number.
    pub serial_number: Option<String>,
    /// Device UDID — also the backup directory's name.
    pub unique_identifier: Option<String>,
    /// IMEI, for a cellular device.
    pub imei: Option<String>,
    /// `Status.plist` `SnapshotState`: `"finished"` for a completed backup.
    ///
    /// A backup interrupted mid-write leaves this at another value, which is a
    /// reason to expect missing blobs.
    pub snapshot_state: Option<String>,
    /// Whether `Status.plist` records this as a full backup.
    pub is_full_backup: Option<bool>,
}

/// Read a property list from `root/name`, or `None` when the file is absent.
///
/// A *missing* file is `None`; a file that is present but unreadable is an
/// error. Collapsing the two would hide a corrupt plist as an absent one.
fn optional_plist(root: &Path, name: &str) -> Result<Option<plist::Dictionary>, Error> {
    let path = root.join(name);
    if !path.exists() {
        return Ok(None);
    }
    let value = plist::Value::from_file(&path).map_err(|source| Error::BadPlist {
        file: name.to_owned(),
        detail: source.to_string(),
    })?;
    value
        .into_dictionary()
        .map(Some)
        .ok_or_else(|| Error::BadPlist {
            file: name.to_owned(),
            detail: "not a dictionary".into(),
        })
}

/// A required property list.
fn required_plist(root: &Path, name: &str) -> Result<plist::Dictionary, Error> {
    optional_plist(root, name)?.ok_or_else(|| Error::NotABackup {
        missing: name.to_owned(),
        path: root.display().to_string(),
    })
}

fn string(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key)?.as_string().map(ToOwned::to_owned)
}

fn boolean(dict: &plist::Dictionary, key: &str) -> Option<bool> {
    dict.get(key)?.as_boolean()
}

/// The parsed `Manifest.plist` plus the metadata gathered from its siblings.
pub struct Manifest {
    /// Everything an examiner reads off the backup.
    pub metadata: BackupMetadata,
    /// `BackupKeyBag`, present when the backup is encrypted.
    pub keybag: Option<Vec<u8>>,
    /// `ManifestKey`: a 4-byte little-endian protection class followed by the
    /// wrapped key for `Manifest.db`.
    pub manifest_key: Option<Vec<u8>>,
}

impl Manifest {
    /// Read `Manifest.plist` and, when present, `Status.plist` and `Info.plist`.
    ///
    /// # Errors
    /// [`Error::NotABackup`] when `Manifest.plist` is absent — the cheapest and
    /// clearest signal that this directory is not a backup — and
    /// [`Error::BadPlist`] when a plist is present but will not parse.
    pub fn read(root: &Path) -> Result<Self, Error> {
        let manifest = required_plist(root, "Manifest.plist")?;
        let status = optional_plist(root, "Status.plist")?;
        let info = optional_plist(root, "Info.plist")?;

        // Lockdown is the authoritative device record inside Manifest.plist;
        // Info.plist carries the same facts under human-readable keys and is
        // the fallback when Lockdown is absent or partial.
        let lockdown = manifest
            .get("Lockdown")
            .and_then(plist::Value::as_dictionary);

        let from_lockdown = |key: &str| lockdown.and_then(|d| string(d, key));
        let from_info = |key: &str| info.as_ref().and_then(|d| string(d, key));

        let metadata = BackupMetadata {
            version: string(&manifest, "Version"),
            is_encrypted: boolean(&manifest, "IsEncrypted").unwrap_or(false),
            was_passcode_set: boolean(&manifest, "WasPasscodeSet").unwrap_or(false),
            date: manifest
                .get("Date")
                .and_then(plist::Value::as_date)
                .map(SystemTime::from),
            product_version: from_lockdown("ProductVersion")
                .or_else(|| from_info("Product Version")),
            product_type: from_lockdown("ProductType").or_else(|| from_info("Product Type")),
            build_version: from_lockdown("BuildVersion").or_else(|| from_info("Build Version")),
            device_name: from_lockdown("DeviceName").or_else(|| from_info("Device Name")),
            serial_number: from_lockdown("SerialNumber").or_else(|| from_info("Serial Number")),
            unique_identifier: from_lockdown("UniqueDeviceID")
                .or_else(|| from_info("Unique Identifier")),
            imei: from_lockdown("InternationalMobileEquipmentIdentity")
                .or_else(|| from_info("IMEI")),
            snapshot_state: status.as_ref().and_then(|d| string(d, "SnapshotState")),
            is_full_backup: status.as_ref().and_then(|d| boolean(d, "IsFullBackup")),
        };

        Ok(Self {
            metadata,
            keybag: manifest
                .get("BackupKeyBag")
                .and_then(plist::Value::as_data)
                .map(<[u8]>::to_vec),
            manifest_key: manifest
                .get("ManifestKey")
                .and_then(plist::Value::as_data)
                .map(<[u8]>::to_vec),
        })
    }
}
