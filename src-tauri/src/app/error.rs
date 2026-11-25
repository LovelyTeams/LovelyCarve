//! Structured errors returned by commands.
//!
//! Engines propagate failures with `anyhow` and raise the ones whose kind
//! matters as typed [`Failure`]s (see [`crate::core::common::error`]).
//! Command handlers convert failures into this stable IPC shape; its `code`
//! comes from the types in the error chain, never from the message, so the
//! frontend and the CLI can tell cancellations, missing evidence, locked
//! cases and retryable source changes apart.

use serde::Serialize;
use serde_json::Value;

use crate::core::casework::case::CaseLocked;
pub use crate::core::common::error::ErrorCode;
use crate::core::common::error::{self, Failure};
use crate::core::extensions::plugin::{PackageRejection, PluginError};
use crate::core::extensions::plugin_market::MarketError;
use crate::core::extensions::task_scheduler::SchedError;
use crate::core::filesystem::fs_dispatch::MissingDataStream;
use crate::task_manager::{TaskFailure, TaskResourceAcquireError};

/// A command failure in its IPC shape.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub context: Option<Value>,
    pub retryable: bool,
}

impl TaskFailure for AppError {
    fn is_cancellation(&self) -> bool {
        self.code == ErrorCode::Cancelled
    }

    fn describe(&self) -> String {
        self.message.clone()
    }
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: None,
            retryable: code.is_retryable(),
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Cancelled, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn source_changed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::SourceChanged, message)
    }

    pub fn resource_limit(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceLimit, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn task_panicked(task: impl Into<String>, error: tokio::task::JoinError) -> Self {
        Self::new(
            ErrorCode::TaskPanicked,
            format!("{} task crashed: {error}", task.into()),
        )
    }

    pub fn from_io(operation: impl Into<String>, error: std::io::Error) -> Self {
        Self::new(
            ErrorCode::of_io(error.kind()),
            format!("{}: {error}", operation.into()),
        )
    }

    /// A failed read of evidence bytes: coded by the types in its chain, and
    /// `io` when they state no code.
    pub fn from_read(operation: impl Into<String>, error: anyhow::Error) -> Self {
        let code = error::classify(&error, cause_code).unwrap_or(ErrorCode::Io);
        Self::new(code, format!("{}: {error:#}", operation.into()))
    }

    pub fn with_context_value(mut self, context: Value) -> Self {
        self.context = Some(context);
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        // A case-lock refusal reads the same wherever it was raised.
        if let Some(locked) = error.downcast_ref::<CaseLocked>() {
            return Self::from(*locked);
        }
        let code = error::classify(&error, cause_code).unwrap_or(ErrorCode::Internal);
        Self::new(code, format!("{error:#}"))
    }
}

impl From<Failure> for AppError {
    fn from(failure: Failure) -> Self {
        Self::new(failure.code(), failure.message())
    }
}

impl From<CaseLocked> for AppError {
    fn from(error: CaseLocked) -> Self {
        Self::new(ErrorCode::CaseLocked, error.to_string())
    }
}

impl From<TaskResourceAcquireError> for AppError {
    fn from(error: TaskResourceAcquireError) -> Self {
        let message = error.to_string();
        match error {
            TaskResourceAcquireError::Cancelled => Self::cancelled(message),
            TaskResourceAcquireError::SemaphoreClosed { .. } => {
                Self::internal(message).retryable(true)
            }
        }
    }
}

impl From<SchedError> for AppError {
    fn from(error: SchedError) -> Self {
        match error {
            SchedError::NoCase | SchedError::TaskNotFound(_) => Self::not_found(error.to_string()),
            SchedError::InvalidState { .. }
            | SchedError::FunctionUnknown(_)
            | SchedError::EvidenceUnknown(_)
            | SchedError::InvalidParams(_) => Self::invalid_input(error.to_string()),
            SchedError::Store(inner) => Self::from(inner),
        }
    }
}

impl From<PluginError> for AppError {
    fn from(error: PluginError) -> Self {
        let app_error = Self::new(plugin_code(&error), error.to_string());
        match &error {
            // The interface states each rejection in its own words.
            PluginError::Package(rejection) => {
                app_error.with_context_value(serde_json::json!({ "reason": rejection.reason() }))
            }
            _ => app_error,
        }
    }
}

impl From<MarketError> for AppError {
    fn from(error: MarketError) -> Self {
        let reason = error.reason();
        let app_error = match error {
            MarketError::Plugin(plugin) => return Self::from(plugin),
            MarketError::Io(io) => return Self::from_io("plugin market", io),
            MarketError::InvalidSource(_)
            | MarketError::InvalidIndex { .. }
            | MarketError::Mismatch { .. } => Self::invalid_input(error.to_string()),
            MarketError::UnknownSource(_)
            | MarketError::UnknownPackage { .. }
            | MarketError::UnknownVersion { .. }
            | MarketError::NoPackageForPlatform { .. } => Self::not_found(error.to_string()),
            MarketError::Fetch { .. } | MarketError::Download { .. } => {
                Self::new(ErrorCode::Io, error.to_string())
            }
            MarketError::Integrity { .. } => Self::new(ErrorCode::CorruptData, error.to_string()),
        };
        match reason {
            Some(reason) => app_error.with_context_value(serde_json::json!({ "reason": reason })),
            None => app_error,
        }
    }
}

/// The code of one cause in an error chain: the core vocabulary, plus the
/// domain errors that travel inside `anyhow` chains.
fn cause_code(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorCode> {
    if let Some(code) = error::cause_code(cause) {
        return Some(code);
    }
    if cause.is::<CaseLocked>() {
        return Some(ErrorCode::CaseLocked);
    }
    if cause.is::<MissingDataStream>() {
        return Some(ErrorCode::NotFound);
    }
    if let Some(plugin) = cause.downcast_ref::<PluginError>() {
        return Some(plugin_code(plugin));
    }
    cause.downcast_ref::<tokio::task::JoinError>().map(|join| {
        if join.is_cancelled() {
            ErrorCode::Cancelled
        } else {
            ErrorCode::TaskPanicked
        }
    })
}

fn plugin_code(error: &PluginError) -> ErrorCode {
    match error {
        PluginError::Io(io) => ErrorCode::of_io(io.kind()),
        PluginError::Rpc { code, .. } => lpp_code(code),
        PluginError::Manifest(_) => ErrorCode::InvalidInput,
        PluginError::Handshake(_) | PluginError::Unavailable(_) => ErrorCode::FfiUnavailable,
        PluginError::Timeout | PluginError::ResourceLimit(_) => ErrorCode::ResourceLimit,
        PluginError::Package(rejection) => match rejection {
            PackageRejection::NoInstallRoot => ErrorCode::NotFound,
            PackageRejection::InvalidArchive(_)
            | PackageRejection::InvalidPackage(_)
            | PackageRejection::NoLibrary { .. }
            | PackageRejection::Bundled { .. } => ErrorCode::InvalidInput,
            PackageRejection::Changed { .. } => ErrorCode::SourceChanged,
            PackageRejection::LibraryRejected(_) => ErrorCode::FfiUnavailable,
            PackageRejection::InUse(_) => ErrorCode::Io,
        },
        PluginError::InvalidFrame(_)
        | PluginError::Serialization(_)
        | PluginError::Deserialization(_)
        | PluginError::ConnectionClosed
        | PluginError::BlockingOnAsyncRuntime => ErrorCode::Internal,
    }
}

/// The code of an LPP error reply. Plugins may send codes of their own; the
/// shared ones keep their meaning.
fn lpp_code(code: &str) -> ErrorCode {
    match code {
        "CANCELLED" => ErrorCode::Cancelled,
        "INVALID_PARAMS" => ErrorCode::InvalidInput,
        "NOT_FOUND" | "EVIDENCE_PATH_NOT_FOUND" | "UNKNOWN_SOURCE" => ErrorCode::NotFound,
        "PERMISSION_DENIED" => ErrorCode::PermissionDenied,
        "RESOURCE_LIMIT" | "READ_TOO_LARGE" | "TOO_MANY_RANGES" => ErrorCode::ResourceLimit,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{anyhow, Context};
    use serde_json::json;

    use super::{AppError, ErrorCode};
    use crate::core::common::error::Failure;
    use crate::core::extensions::plugin::PluginError;
    use crate::task_manager::TaskResourceAcquireError;

    #[test]
    fn serializes_stable_ipc_shape() {
        let error = AppError::not_found("image not loaded")
            .with_context_value(json!({ "image_id": "img-1" }));

        let value = serde_json::to_value(error).unwrap();

        assert_eq!(
            value,
            json!({
                "code": "notFound",
                "message": "image not loaded",
                "context": { "image_id": "img-1" },
                "retryable": false
            })
        );
    }

    #[test]
    fn codes_come_from_failure_types_not_messages() {
        let typed = AppError::from(
            anyhow!(Failure::source_changed("源镜像文件已变化：a.E01"))
                .context("源盘哈希前校验失败"),
        );
        assert_eq!(typed.code, ErrorCode::SourceChanged);
        assert!(typed.retryable);
        assert_eq!(typed.message, "源盘哈希前校验失败: 源镜像文件已变化：a.E01");

        // Wording alone no longer decides anything.
        let untyped = AppError::from(anyhow!("export cancelled: image not found"));
        assert_eq!(untyped.code, ErrorCode::Internal);
        assert!(!untyped.retryable);
    }

    #[test]
    fn cancellation_wins_over_outer_failures() {
        let error = AppError::from(
            anyhow!(Failure::cancelled("memory scan cancelled"))
                .context(Failure::corrupt("BitLocker metadata unreadable")),
        );

        assert_eq!(error.code, ErrorCode::Cancelled);
    }

    #[test]
    fn host_io_errors_keep_their_kind() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let error = AppError::from(anyhow::Error::new(denied).context("open 'memory.raw'"));
        assert_eq!(error.code, ErrorCode::PermissionDenied);

        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(
            AppError::from_io("open case.json", missing).code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn maps_case_lock_refusals_through_anyhow_context() {
        let refusal = crate::core::casework::case::CaseLocked {
            edit: crate::core::casework::case::LockedEdit::AddEvidence,
        };
        let error = AppError::from(anyhow::Error::new(refusal).context("register evidence"));

        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({
                "code": "caseLocked",
                "message": "case is locked; unlock it before adding evidence",
                "context": null,
                "retryable": false
            })
        );
    }

    #[test]
    fn maps_task_resource_acquire_errors() {
        let cancelled = AppError::from(TaskResourceAcquireError::Cancelled);
        assert_eq!(cancelled.code, ErrorCode::Cancelled);
        assert_eq!(cancelled.message, "任务已取消");
        assert!(!cancelled.retryable);

        let closed = AppError::from(TaskResourceAcquireError::SemaphoreClosed {
            label: "per-image random I/O".to_string(),
        });
        assert_eq!(closed.code, ErrorCode::Internal);
        assert_eq!(closed.message, "per-image random I/O semaphore closed");
        assert!(closed.retryable);
    }

    #[test]
    fn maps_plugin_errors_by_variant() {
        let cancelled = PluginError::Rpc {
            code: "CANCELLED".to_owned(),
            message: "request cancelled".to_owned(),
        };
        assert_eq!(AppError::from(cancelled).code, ErrorCode::Cancelled);
        assert_eq!(
            AppError::from(PluginError::Timeout).code,
            ErrorCode::ResourceLimit
        );
        let boxed = anyhow::Error::new(PluginError::Unavailable("host exited".to_owned()))
            .context("start linuxinfo");
        assert_eq!(AppError::from(boxed).code, ErrorCode::FfiUnavailable);
    }

    #[test]
    fn a_missing_data_stream_is_not_found() {
        let missing = crate::core::filesystem::fs_dispatch::MissingDataStream {
            message: "$Secure has no unnamed data stream".to_owned(),
        };
        let error: anyhow::Result<()> = Err(anyhow::Error::new(missing)).context("preview $Secure");

        assert_eq!(AppError::from(error.unwrap_err()).code, ErrorCode::NotFound);
    }
}
