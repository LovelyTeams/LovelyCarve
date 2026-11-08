//! Domains an examiner would expect to find, and what each one holds.
//!
//! This list drives an *absence* observation, so what belongs in it is narrow:
//! a domain earns a place only if a reader would want to know it was missing.
//! Padding it with domains that are routinely absent turns every backup into a
//! wall of findings and trains the reader to skip them.
//!
//! Matching is by **prefix**, because the app-group domains carry a bundle
//! identifier suffix (`AppDomainGroup-group.net.whatsapp.WhatsApp.shared`) and
//! an exact match would report every one of them absent.

/// A domain worth noticing the absence of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedDomain {
    /// The domain, or the prefix an app-group domain starts with.
    pub domain: &'static str,
    /// What it holds, in the terms an examiner would ask for it.
    pub what: &'static str,
}

/// The domains whose absence is reported.
///
/// The system domains are here because essentially every iPhone backup has
/// them, so their absence says something about the *backup* — that it is
/// partial, filtered, or not a full-device backup at all. The messenger domains
/// are here because their presence or absence is the question most often asked
/// of a phone backup.
pub const EXPECTED_DOMAINS: &[ExpectedDomain] = &[
    ExpectedDomain {
        domain: "HomeDomain",
        what: "user preferences, Messages, call history, notes",
    },
    ExpectedDomain {
        domain: "CameraRollDomain",
        what: "the camera roll and its Photos database",
    },
    ExpectedDomain {
        domain: "WirelessDomain",
        what: "cellular and carrier state",
    },
    ExpectedDomain {
        domain: "KeychainDomain",
        what: "the keychain (encrypted backups only)",
    },
    ExpectedDomain {
        domain: "AppDomainGroup-group.net.whatsapp",
        what: "WhatsApp messages and media",
    },
    ExpectedDomain {
        domain: "AppDomain-net.whatsapp",
        what: "the WhatsApp application container",
    },
    ExpectedDomain {
        domain: "AppDomainGroup-group.org.whispersystems.signal",
        what: "Signal messages",
    },
    ExpectedDomain {
        domain: "AppDomain-ph.telegra.Telegraph",
        what: "Telegram messages and media",
    },
];
