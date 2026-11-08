use thiserror::Error;

pub type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid LPP frame: {0}")]
    InvalidFrame(String),
    #[error("LPP serialization failed: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("LPP deserialization failed: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    #[error("plugin protocol error {code}: {message}")]
    Rpc { code: String, message: String },
    #[error("plugin connection closed")]
    ConnectionClosed,
    #[error("plugin handshake failed: {0}")]
    Handshake(String),
    #[error("invalid plugin manifest: {0}")]
    Manifest(String),
    #[error("plugin runtime unavailable: {0}")]
    Unavailable(String),
    #[error("plugin request timed out")]
    Timeout,
    #[error("plugin resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("synchronous plugin RPC was called from an async runtime worker")]
    BlockingOnAsyncRuntime,
}
