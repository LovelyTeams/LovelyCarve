//! The failure vocabulary shared by every engine, the desktop app, the CLI
//! and the Python binding.
//!
//! Engines propagate failures with `anyhow` and describe each step with
//! `.context(...)`. A failure whose kind a caller acts on is raised as a
//! [`Failure`], which states its [`ErrorCode`]:
//!
//! ```ignore
//! anyhow::bail!(Failure::not_found(format!("node {id} not found")));
//! anyhow::ensure!(!cancel.is_cancelled(), Failure::cancelled("export cancelled"));
//! let end = offset.checked_add(length).context(Failure::corrupt("extent end overflow"))?;
//! ```
//!
//! [`code_of`] reads the code back from the types in an error chain; nothing
//! classifies a failure by its message. Host I/O errors carry their own kind,
//! and a failure raised without one is [`ErrorCode::Internal`].

use std::{borrow::Cow, error::Error, io};

use serde::Serialize;

/// The kind of a failure: the stable error code of the IPC protocol and the
/// CLI, serialized as [`ErrorCode::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The operation stopped because its task was cancelled or its evidence
    /// was closed.
    Cancelled,
    /// The host refused access to a file, device or credential.
    PermissionDenied,
    /// The evidence uses a format, version or feature this build does not
    /// read.
    UnsupportedFormat,
    /// Evidence structures failed validation, or a source read came back
    /// short or synthesized.
    CorruptData,
    /// The source image changed after it was opened.
    SourceChanged,
    /// A size, count or work limit was reached.
    ResourceLimit,
    /// A native component, driver or library is not available.
    FfiUnavailable,
    /// A background worker panicked.
    TaskPanicked,
    /// The requested evidence, node, record or task does not exist.
    NotFound,
    /// The request is malformed or does not apply: bad parameters, ids or
    /// credentials.
    InvalidInput,
    /// The active case is locked and refuses the edit.
    CaseLocked,
    /// A host input/output operation failed.
    Io,
    /// Anything else, including defects.
    Internal,
}

impl ErrorCode {
    /// The code's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::PermissionDenied => "permissionDenied",
            Self::UnsupportedFormat => "unsupportedFormat",
            Self::CorruptData => "corruptData",
            Self::SourceChanged => "sourceChanged",
            Self::ResourceLimit => "resourceLimit",
            Self::FfiUnavailable => "ffiUnavailable",
            Self::TaskPanicked => "taskPanicked",
            Self::NotFound => "notFound",
            Self::InvalidInput => "invalidInput",
            Self::CaseLocked => "caseLocked",
            Self::Io => "io",
            Self::Internal => "internal",
        }
    }

    /// The code of a host I/O error kind.
    pub fn of_io(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                Self::PermissionDenied
            }
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => Self::CorruptData,
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidFilename => Self::InvalidInput,
            io::ErrorKind::Unsupported => Self::UnsupportedFormat,
            io::ErrorKind::OutOfMemory
            | io::ErrorKind::StorageFull
            | io::ErrorKind::QuotaExceeded
            | io::ErrorKind::FileTooLarge => Self::ResourceLimit,
            // `io::ErrorKind` is non-exhaustive; every other kind is a host
            // I/O failure.
            _ => Self::Io,
        }
    }

    /// Whether repeating the same request may succeed.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::SourceChanged | Self::Io)
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A failure of a stated kind. Its message is the whole of its display, so
/// raising one changes no user-visible text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    code: ErrorCode,
    message: Cow<'static, str>,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn cancelled(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Cancelled, message)
    }

    pub fn permission_denied(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    pub fn unsupported(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::UnsupportedFormat, message)
    }

    pub fn corrupt(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::CorruptData, message)
    }

    pub fn source_changed(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::SourceChanged, message)
    }

    pub fn resource_limit(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::ResourceLimit, message)
    }

    pub fn unavailable(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::FfiUnavailable, message)
    }

    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn invalid_input(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn io(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Io, message)
    }
}

/// The code `error` states through its types, if any.
///
/// A cancellation anywhere in the chain wins, so a task stopped deep inside
/// a step is reported as cancelled whatever its callers added. Otherwise the
/// outermost typed cause decides: a [`Failure`] raised or attached as
/// context, then the causes below in order.
pub fn code_of(error: &anyhow::Error) -> Option<ErrorCode> {
    classify(error, cause_code)
}

/// [`code_of`] with `cause_code` stating the code of each cause, for callers
/// that know more error types than this module.
pub fn classify(
    error: &anyhow::Error,
    cause_code: impl Fn(&(dyn Error + 'static)) -> Option<ErrorCode>,
) -> Option<ErrorCode> {
    let typed = || error.chain().filter_map(&cause_code);
    if typed().any(|code| code == ErrorCode::Cancelled) {
        return Some(ErrorCode::Cancelled);
    }
    // anyhow's own downcast also sees context layers, which `chain()`
    // presents as opaque wrappers.
    if let Some(failure) = error.downcast_ref::<Failure>() {
        return Some(failure.code);
    }
    typed().next()
}

/// The code one error in a chain states by its type: a [`Failure`] or a
/// host I/O error.
pub fn cause_code(cause: &(dyn Error + 'static)) -> Option<ErrorCode> {
    if let Some(failure) = cause.downcast_ref::<Failure>() {
        return Some(failure.code);
    }
    cause
        .downcast_ref::<io::Error>()
        .map(|error| ErrorCode::of_io(error.kind()))
}

#[cfg(test)]
mod tests {
    use anyhow::{anyhow, Context};

    use super::{code_of, ErrorCode, Failure};

    #[test]
    fn untyped_failures_state_no_code() {
        assert_eq!(code_of(&anyhow!("index row 3 is missing")), None);
        assert_eq!(
            code_of(&anyhow!("partition not found").context("open volume")),
            None
        );
    }

    #[test]
    fn a_raised_failure_states_its_code_through_context() {
        let error = anyhow!(Failure::not_found("node 7 not found"))
            .context("list directory")
            .context("refresh tree");

        assert_eq!(code_of(&error), Some(ErrorCode::NotFound));
        assert_eq!(
            format!("{error:#}"),
            "refresh tree: list directory: node 7 not found"
        );
    }

    #[test]
    fn a_failure_attached_as_context_classifies_the_error_below() {
        let missing: Option<u64> = None;
        let error = missing
            .context(Failure::corrupt("extent end overflow"))
            .unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::CorruptData));

        let io = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let error = Err::<(), _>(io)
            .context(Failure::invalid_input("evidence path is a directory"))
            .unwrap_err();
        assert_eq!(code_of(&error), Some(ErrorCode::InvalidInput));
    }

    #[test]
    fn host_io_errors_state_their_kind() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        let error = anyhow::Error::new(missing).context("open segment 2");
        assert_eq!(code_of(&error), Some(ErrorCode::NotFound));

        let short = std::io::Error::from(std::io::ErrorKind::UnexpectedEof);
        assert_eq!(
            code_of(&anyhow::Error::new(short)),
            Some(ErrorCode::CorruptData)
        );
    }

    #[test]
    fn a_cancellation_below_other_failures_wins() {
        let error = anyhow!(Failure::cancelled("snapshot build cancelled"))
            .context(Failure::corrupt("MFT record 5 is unreadable"))
            .context("build snapshot");

        assert_eq!(code_of(&error), Some(ErrorCode::Cancelled));
    }

    #[test]
    fn the_outermost_typed_cause_decides() {
        let io = std::io::Error::from(std::io::ErrorKind::NotFound);
        let error = anyhow::Error::new(io).context(Failure::unsupported("AD1 encryption"));

        assert_eq!(code_of(&error), Some(ErrorCode::UnsupportedFormat));
    }
}
