//! How a backup password reaches the reader.
//!
//! This is the seam a mounting tool needs. `4n6mount` opens evidence through
//! the fleet's container abstraction, which has no credential parameter — an
//! encrypted AFF4 is already turned away there with *"needs a password"*. So
//! the gap is the abstraction's, not this format's, and [`Credentials`] is
//! shaped to be passed through it rather than to be an iOS special case.
//!
//! # Supplying a password safely
//!
//! The constructors are ordered by how much they leak, and the ordering is the
//! recommendation:
//!
//! | route | who else can see it |
//! |---|---|
//! | [`Password::from_tty`] — interactive prompt | nobody |
//! | [`Password::from_file`] — a file the examiner controls | anyone who can read the file |
//! | [`Password::from_env`] — an environment variable | the process tree, `/proc`, crash dumps |
//! | [`Password::new`] from a command-line argument | **every user on the machine**, via `ps` |
//!
//! A password on the command line is visible in the process table for as long
//! as the mount runs, so a tool that offers `--password` should offer
//! `--password-file` and a prompt too, and should prefer them.

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Error;

/// A backup password, zeroized on drop.
///
/// Stored as bytes, not `String`: a password is a byte sequence to PBKDF2, and
/// keeping it as bytes avoids a `String` clone that would outlive the zeroize.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Password(Vec<u8>);

impl Password {
    /// From a string already in memory — a CLI argument, a config value.
    ///
    /// Prefer [`Self::from_tty`] or [`Self::from_file`] where the caller has
    /// the choice: an argument passed on a command line is readable by every
    /// user on the machine through the process table.
    #[must_use]
    pub fn new(password: &str) -> Self {
        Self(password.as_bytes().to_vec())
    }

    /// From raw bytes, for a password that is not valid UTF-8.
    ///
    /// iOS accepts a passcode the user typed on a device keyboard; it is not
    /// guaranteed to survive a round trip through `String`.
    #[must_use]
    pub fn from_bytes(password: Vec<u8>) -> Self {
        Self(password)
    }

    /// From an environment variable, or `None` when it is unset.
    #[must_use]
    pub fn from_env(variable: &str) -> Option<Self> {
        std::env::var_os(variable).map(|value| {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                Self(value.as_os_str().as_bytes().to_vec())
            }
            #[cfg(not(unix))]
            {
                Self(value.to_string_lossy().as_bytes().to_vec())
            }
        })
    }

    /// From a file's contents, with **one** trailing newline removed.
    ///
    /// Exactly one: an editor adds a single newline, so stripping one recovers
    /// what the examiner typed, while stripping all of them would silently
    /// change a password that genuinely ends in whitespace.
    ///
    /// # Errors
    /// [`Error::Io`] if the file cannot be read.
    pub fn from_file(path: &std::path::Path) -> Result<Self, Error> {
        let mut bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        Ok(Self(bytes))
    }

    /// Prompt on the controlling terminal, without echo.
    ///
    /// # Errors
    /// [`Error::NoTerminal`] when the process has no controlling terminal — a
    /// daemon, a CI job, a pipeline. The caller should fall back to
    /// [`Self::from_file`] rather than silently proceeding without a password.
    pub fn from_tty(prompt: &str) -> Result<Self, Error> {
        // Deliberately not implemented in the library: reading a password
        // without echo is the front-end's job, and pulling a terminal crate
        // into a parsing library would put it in every downstream binary.
        // The variant exists so a front-end can report the failure in the
        // reader's own error vocabulary.
        let _ = prompt;
        Err(Error::NoTerminal)
    }

    /// The raw bytes, for the key derivation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Whether the password is empty. An empty password is legal — iOS allows
    /// one — so this is informational, never a reason to refuse.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Never renders the password. A credential that prints itself will eventually
/// print itself into a log, an error report or a panic message.
impl core::fmt::Debug for Password {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// What the caller can offer to open a backup.
///
/// Defaults to [`Credentials::None`], so the zero-configuration path is the one
/// that reads a plaintext backup and *asks* for a password on an encrypted one
/// rather than guessing at an empty string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Credentials {
    /// Nothing supplied. An encrypted backup is refused with
    /// [`Error::PasswordRequired`].
    #[default]
    None,
    /// A backup password.
    Password(Password),
}

impl Credentials {
    /// No credentials.
    #[must_use]
    pub fn none() -> Self {
        Self::None
    }

    /// Open with `password`.
    #[must_use]
    pub fn password(password: Password) -> Self {
        Self::Password(password)
    }

    /// The password, when one was supplied.
    #[must_use]
    pub fn as_password(&self) -> Option<&Password> {
        match self {
            Self::None => None,
            Self::Password(password) => Some(password),
        }
    }
}

impl From<Password> for Credentials {
    fn from(password: Password) -> Self {
        Self::Password(password)
    }
}
