use thiserror::Error;

#[derive(Error, Debug)]
/// UBIFS Dumper 错误类型
pub enum UbiDumpError {
    /// I/O错误
    #[error("I/O错误: {0}")]
    Io(#[from] std::io::Error),

    /// 解析错误
    #[error("解析错误: {0}")]
    ParseError(String),

    /// CRC校验错误
    #[error("CRC校验错误: 预期 {expected}, 实际 {actual}")]
    CrcError {
        /// 预期的CRC值
        expected: u32,
        /// 实际的CRC值
        actual: u32,
    },

    /// 魔术数字错误
    #[error("魔术数字错误: 预期 {expected}, 实际 {actual}")]
    MagicError {
        /// 预期的魔术数字
        expected: u32,
        /// 实际的魔术数字
        actual: u32,
    },

    /// 卷未找到错误
    #[error("卷未找到: {0}")]
    VolumeNotFound(u32),

    /// 压缩错误
    #[error("压缩/解压缩错误: {0}")]
    CompressError(String),

    /// 未实现功能错误
    #[error("未实现功能: {0}")]
    NotImplemented(String),
}

/// UBIFS Dumper 结果类型
pub type Result<T> = std::result::Result<T, UbiDumpError>;
