//! Structured errors returned by Tauri commands.
//!
//! Core modules can keep using `anyhow` or domain-specific errors; command
//! handlers translate failures into this stable IPC shape so the frontend can
//! distinguish retryable source changes, missing images, cancellations and
//! internal worker failures without parsing display strings forever.

use serde::Serialize;
use serde_json::Value;

use crate::core::extensions::task_scheduler::SchedError;
use crate::task_manager::{TaskFailure, TaskResourceAcquireError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    Cancelled,
    PermissionDenied,
    UnsupportedFormat,
    CorruptData,
    SourceChanged,
    ResourceLimit,
    FfiUnavailable,
    TaskPanicked,
    NotFound,
    InvalidInput,
    Io,
    Internal,
}

#[derive(Debug, Clone, Serialize)]
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
        let retryable = matches!(code, ErrorCode::SourceChanged | ErrorCode::Io);
        Self {
            code,
            message: message.into(),
            context: None,
            retryable,
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

    pub fn task_panicked(task: impl Into<String>, error: tokio::task::JoinError) -> Self {
        Self::new(
            ErrorCode::TaskPanicked,
            format!("{} task crashed: {error}", task.into()),
        )
    }

    pub fn from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(classify_message(&message), message)
    }

    pub fn from_anyhow(error: anyhow::Error) -> Self {
        Self::from_message(format!("{error:#}"))
    }

    pub fn from_io(operation: impl Into<String>, error: std::io::Error) -> Self {
        let operation = operation.into();
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::Io,
        };
        Self::new(code, format!("{operation}: {error}"))
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
        Self::from_anyhow(error)
    }
}

impl From<TaskResourceAcquireError> for AppError {
    fn from(error: TaskResourceAcquireError) -> Self {
        match error {
            TaskResourceAcquireError::Cancelled => Self::cancelled("任务已取消"),
            TaskResourceAcquireError::SemaphoreClosed { label } => {
                Self::new(ErrorCode::Internal, format!("{label} semaphore closed")).retryable(true)
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
            SchedError::Store(inner) => Self::from_anyhow(inner),
        }
    }
}

fn classify_message(message: &str) -> ErrorCode {
    let lower = message.to_ascii_lowercase();
    if lower.contains("cancelled") || message.contains("已取消") || message.contains("取消") {
        ErrorCode::Cancelled
    } else if message.contains("源镜像文件已变化") {
        ErrorCode::SourceChanged
    } else if lower.contains("permission denied")
        || lower.contains("access denied")
        || message.contains("拒绝访问")
        || message.contains("权限")
    {
        ErrorCode::PermissionDenied
    } else if lower.contains("unsupported")
        || lower.contains("not support")
        || lower.contains("is not bitlocker")
        || message.contains("不支持")
        || message.contains("暂不支持")
    {
        ErrorCode::UnsupportedFormat
    } else if lower.contains("corrupt")
        || lower.contains("malformed")
        || lower.contains("crc")
        || lower.contains("checksum")
        || lower.contains("ended early")
        || lower.contains("synthetic zero")
        || message.contains("损坏")
        || message.contains("校验和")
        || message.contains("补零字节")
    {
        ErrorCode::CorruptData
    } else if lower.contains("too large")
        || lower.contains("resource")
        || lower.contains("limit")
        || lower.contains("overflow")
        || message.contains("最多")
        || message.contains("上限")
        || message.contains("溢出")
    {
        ErrorCode::ResourceLimit
    } else if lower.contains("dll")
        || lower.contains("load library")
        || message.contains("无法加载")
    {
        ErrorCode::FfiUnavailable
    } else if lower.contains("not found")
        || lower.contains("not loaded")
        || message.contains("不存在")
        || message.contains("未加载")
    {
        ErrorCode::NotFound
    } else if lower.contains("invalid")
        || lower.contains("bad input")
        || lower.contains("not unlocked")
        || lower.contains("does not unlock")
        || lower.contains("is required")
        || message.contains("无效")
        || message.contains("非法")
    {
        ErrorCode::InvalidInput
    } else if lower.starts_with("io ")
        || lower.starts_with("open ")
        || lower.starts_with("read ")
        || lower.starts_with("write ")
        || lower.starts_with("create ")
        || lower.starts_with("stat ")
        || lower.starts_with("flush ")
    {
        ErrorCode::Io
    } else {
        ErrorCode::Internal
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AppError, ErrorCode};
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
    fn classifies_common_command_messages() {
        assert_eq!(
            AppError::from_message("源盘哈希前校验失败：源镜像文件已变化").code,
            ErrorCode::SourceChanged
        );
        assert_eq!(
            AppError::from_message("fs_read_range length overflow").code,
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            AppError::from_message("export cancelled").code,
            ErrorCode::Cancelled
        );
        assert_eq!(
            AppError::from_message("严格读取在偏移 0 的数据块检测到补零字节").code,
            ErrorCode::CorruptData
        );
        assert_eq!(
            AppError::from_message(
                "strict read detected synthetic zero bytes in the block at offset 0"
            )
            .code,
            ErrorCode::CorruptData
        );
        assert_eq!(
            AppError::from_message("image not loaded").code,
            ErrorCode::NotFound
        );
        assert_eq!(
            AppError::from_message("partition 1 is not BitLocker").code,
            ErrorCode::UnsupportedFormat
        );
        assert_eq!(
            AppError::from_message("BitLocker partition 1 is not unlocked").code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            AppError::from_message("recovery password or password is required").code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            AppError::from_message("the supplied VMK does not unlock this BitLocker volume").code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            AppError::from_message("decrypted BitLocker stream ended early at 0 of 4096 bytes")
                .code,
            ErrorCode::CorruptData
        );
        assert_eq!(
            AppError::from_message("memory scan cancelled").code,
            ErrorCode::Cancelled
        );
        assert_eq!(
            AppError::from_message("open 'memory.raw': Access denied").code,
            ErrorCode::PermissionDenied
        );
    }

    #[test]
    fn maps_task_resource_acquire_errors_without_text_classification() {
        let cancelled = AppError::from(TaskResourceAcquireError::Cancelled);
        assert_eq!(cancelled.code, ErrorCode::Cancelled);
        assert!(!cancelled.retryable);

        let closed = AppError::from(TaskResourceAcquireError::SemaphoreClosed {
            label: "per-image random I/O".to_string(),
        });
        assert_eq!(closed.code, ErrorCode::Internal);
        assert_eq!(closed.message, "per-image random I/O semaphore closed");
        assert!(closed.retryable);
    }
}
