//! What opens an encrypted volume, for every scheme the unlock path supports.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Encryption formats one unlock path opens.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionScheme {
    Bitlocker,
    Luks,
    Veracrypt,
    Filevault,
}

impl EncryptionScheme {
    /// The IPC and task-label word of the scheme.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bitlocker => "bitlocker",
            Self::Luks => "luks",
            Self::Veracrypt => "veracrypt",
            Self::Filevault => "filevault",
        }
    }

    /// The product name used in messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bitlocker => "BitLocker",
            Self::Luks => "LUKS",
            Self::Veracrypt => "VeraCrypt",
            Self::Filevault => "FileVault",
        }
    }
}

/// What opens a volume. Intentionally no Debug or Serialize: credentials must
/// not enter diagnostics, and their text is wiped when the credential drops.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum UnlockCredential {
    /// A password or passphrase: the BitLocker user password, the LUKS
    /// passphrase, the VeraCrypt password, or the FileVault password or
    /// personal recovery key.
    Password { password: String },
    /// The BitLocker 48-digit recovery password.
    #[serde(rename_all = "camelCase")]
    RecoveryPassword { recovery_password: String },
    /// A BitLocker startup key (`.BEK`) file on the host.
    StartupKey { path: String },
    /// A recovered volume key in hex: the LUKS master key, a FileVault VMK or
    /// XTS key pair, or the BitLocker FVEK with its optional diffuser TWEAK.
    #[serde(rename_all = "camelCase")]
    MasterKey {
        key_hex: String,
        #[serde(default)]
        tweak_hex: Option<String>,
    },
    /// A BitLocker volume master key in hex, such as one found in memory.
    #[serde(rename_all = "camelCase")]
    Vmk { key_hex: String },
    /// The clear-key protector a BitLocker volume keeps while protection is
    /// suspended.
    ClearKey,
}

impl UnlockCredential {
    /// Whether this kind of credential can open a volume of `scheme`.
    pub fn opens(&self, scheme: EncryptionScheme) -> bool {
        match self {
            Self::Password { .. } => true,
            Self::MasterKey { tweak_hex, .. } => match scheme {
                EncryptionScheme::Bitlocker => true,
                EncryptionScheme::Luks | EncryptionScheme::Filevault => tweak_hex.is_none(),
                EncryptionScheme::Veracrypt => false,
            },
            Self::RecoveryPassword { .. }
            | Self::StartupKey { .. }
            | Self::Vmk { .. }
            | Self::ClearKey => scheme == EncryptionScheme::Bitlocker,
        }
    }

    /// The credential kind named in messages; never its value.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Password { .. } => "password",
            Self::RecoveryPassword { .. } => "recovery password",
            Self::StartupKey { .. } => "startup key",
            Self::MasterKey {
                tweak_hex: Some(_), ..
            } => "master key with TWEAK",
            Self::MasterKey {
                tweak_hex: None, ..
            } => "master key",
            Self::Vmk { .. } => "VMK",
            Self::ClearKey => "clear key",
        }
    }
}

impl Drop for UnlockCredential {
    fn drop(&mut self) {
        match self {
            Self::Password { password } => password.zeroize(),
            Self::RecoveryPassword { recovery_password } => recovery_password.zeroize(),
            Self::MasterKey { key_hex, tweak_hex } => {
                key_hex.zeroize();
                if let Some(tweak) = tweak_hex {
                    tweak.zeroize();
                }
            }
            Self::Vmk { key_hex } => key_hex.zeroize(),
            Self::StartupKey { .. } | Self::ClearKey => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<UnlockCredential, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn credentials_parse_by_kind_and_reject_unknown_fields() {
        let labels: Vec<&str> = [
            r#"{"kind":"password","password":"secret"}"#,
            r#"{"kind":"recoveryPassword","recoveryPassword":"1"}"#,
            r#"{"kind":"startupKey","path":"key.bek"}"#,
            r#"{"kind":"masterKey","keyHex":"00"}"#,
            r#"{"kind":"masterKey","keyHex":"00","tweakHex":"11"}"#,
            r#"{"kind":"vmk","keyHex":"00"}"#,
            r#"{"kind":"clearKey"}"#,
        ]
        .into_iter()
        .map(|json| parse(json).map(|credential| credential.label()).unwrap())
        .collect();
        assert_eq!(
            labels,
            vec![
                "password",
                "recovery password",
                "startup key",
                "master key",
                "master key with TWEAK",
                "VMK",
                "clear key",
            ]
        );
        assert!(parse(r#"{"kind":"password","password":"x","pim":1}"#).is_err());
        assert!(parse(r#"{"kind":"fvek","keyHex":"00"}"#).is_err());
    }

    #[test]
    fn only_bitlocker_takes_bitlocker_credentials() {
        let opens = |json: &str| {
            let credential = parse(json).unwrap();
            [
                EncryptionScheme::Bitlocker,
                EncryptionScheme::Luks,
                EncryptionScheme::Veracrypt,
                EncryptionScheme::Filevault,
            ]
            .map(|scheme| credential.opens(scheme))
        };
        assert_eq!(
            opens(r#"{"kind":"password","password":"x"}"#),
            [true, true, true, true]
        );
        assert_eq!(
            opens(r#"{"kind":"masterKey","keyHex":"00"}"#),
            [true, true, false, true]
        );
        assert_eq!(
            opens(r#"{"kind":"masterKey","keyHex":"00","tweakHex":"11"}"#),
            [true, false, false, false]
        );
        for json in [
            r#"{"kind":"recoveryPassword","recoveryPassword":"1"}"#,
            r#"{"kind":"startupKey","path":"key.bek"}"#,
            r#"{"kind":"vmk","keyHex":"00"}"#,
            r#"{"kind":"clearKey"}"#,
        ] {
            assert_eq!(opens(json), [true, false, false, false], "{json}");
        }
    }
}
