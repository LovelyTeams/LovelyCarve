//! Raw file content: a range the transport streams to where it goes (stdout,
//! a backend connection, a batch response), so a large read never sits in
//! memory. A short read or a read over zero-filled bad sectors fails: raw
//! bytes cannot carry the audit that would say so.
use serde::{Deserialize, Serialize};

use super::{exec::Exec, CliError};
use crate::core::{
    common::ids::ImageId,
    evidence::image::ReadFaultReport,
    filesystem::{evidence_session::EvidenceSession, tree::fs_tree::FsSession},
};

/// The largest read a JSON result (a batch response, `--json`) carries inline.
pub const MAX_BUFFERED: u64 = 16 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;
const TOO_LARGE: &str = "一次最多返回 16 MiB：用 --offset/--length 分段读取，或用 export 导出文件";

/// Collects at most [`MAX_BUFFERED`] bytes; anything longer is an error,
/// never a silently shortened result.
#[derive(Default)]
pub struct Bounded(Vec<u8>);

impl Bounded {
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl std::io::Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() as u64 + bytes.len() as u64 > MAX_BUFFERED {
            return Err(std::io::Error::other(TOO_LARGE));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A range of one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadPlan {
    pub image_id: ImageId,
    pub node_id: String,
    pub offset: u64,
    /// To the end of the file when absent.
    pub length: Option<u64>,
}

/// Byte-level commands need the original evidence; index-only sessions can
/// still browse.
pub fn require_source(image: &EvidenceSession) -> Result<(), CliError> {
    if image.is_index_only() {
        image.require_source().map_err(CliError::from)
    } else {
        image.validate_source_file_stable().map_err(CliError::from)
    }
}

/// Raw bytes cannot carry provenance; a read over synthesized zeroes fails
/// and carries the report in its details.
pub fn reject_read_faults(faults: &ReadFaultReport) -> Result<(), CliError> {
    if faults.read_faults_detected {
        let mut error = CliError::new(
            "readFaultsDetected",
            format!(
                "读取包含 {} 字节合成补零；输出不可作为完整源数据（需要保留补零数据和审计报告时用 export）",
                faults.synthetic_bytes
            ),
        );
        error.details = Some(serde_json::to_value(super::wire::Camel(faults))?);
        return Err(error);
    }
    Ok(())
}

/// The byte count `plan` reads, checked against the file's size.
pub async fn length(exec: &Exec, plan: &ReadPlan) -> Result<u64, CliError> {
    let image = exec.image(&plan.image_id).await?;
    let node_id = plan.node_id.clone();
    let (offset, requested) = (plan.offset, plan.length);
    tokio::task::spawn_blocking(move || {
        require_source(&image)?;
        let size = FsSession::new(&image).file_stat(&node_id)?.size;
        if offset > size {
            return Err(CliError::invalid(format!(
                "偏移 {offset} 超过文件大小 {size}"
            )));
        }
        Ok(requested.unwrap_or(size - offset).min(size - offset))
    })
    .await
    .map_err(super::internal_error)?
}

/// Stream `plan` into `sink` and return the byte count.
pub async fn stream(
    exec: &Exec,
    plan: ReadPlan,
    mut sink: impl FnMut(&[u8]) -> std::io::Result<()> + Send + 'static,
) -> Result<u64, CliError> {
    let length = length(exec, &plan).await?;
    let image = exec.image(&plan.image_id).await?;
    tokio::task::spawn_blocking(move || {
        let mut session = FsSession::new(&image);
        let (result, faults) = image.collect_read_faults(|| {
            session.stream_file_range(&plan.node_id, plan.offset, length, CHUNK, |chunk| {
                sink(chunk)?;
                Ok(())
            })
        });
        let read = result?;
        if read != length {
            return Err(CliError::new(
                "sourceChanged",
                format!("文件未完整读取：期望 {length} 字节，实际 {read} 字节"),
            ));
        }
        reject_read_faults(&faults)?;
        image.validate_source_file_stable()?;
        Ok(read)
    })
    .await
    .map_err(super::internal_error)?
}

/// Read `plan` into memory; at most [`MAX_BUFFERED`] bytes.
pub async fn buffer(exec: &Exec, plan: ReadPlan) -> Result<Vec<u8>, CliError> {
    let length = length(exec, &plan).await?;
    if length > MAX_BUFFERED {
        return Err(CliError::invalid(TOO_LARGE));
    }
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(
        usize::try_from(length).unwrap_or_default(),
    )));
    let sink = std::sync::Arc::clone(&collected);
    stream(exec, plan, move |chunk| {
        sink.lock()
            .map_err(|_| std::io::Error::other("读取缓冲区不可用"))?
            .extend_from_slice(chunk);
        Ok(())
    })
    .await?;
    let bytes = match std::sync::Arc::try_unwrap(collected) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(shared) => shared.lock().map(|bytes| bytes.clone()).unwrap_or_default(),
    };
    Ok(bytes)
}
