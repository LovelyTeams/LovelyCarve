//! AI 文件类型识别：Google Magika（standard_v3_3 模型，本地 ONNX 推理）。
//!
//! 推理在推理组件 `lovelycarve_inference` 里运行（见
//! [`crate::core::platform::components::inference`]），主程序不链接 ONNX Runtime。
//! 模型特征只取文件头、尾各一个 `block_size`（4 KiB）窗口，所以这里用
//! `stream_file_chunks` 滚动保留首尾窗口——任意大小的文件都只占两个窗口
//! 的内存，且尾部特征是真实文件尾，不是截断处。

use std::sync::Arc;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;
use crate::core::platform::components::inference::{self, SAMPLE_WINDOW};

/// 单文件识别结果，整体序列化给前端。`Deserialize` 是为了能从检材的
/// `facts.db`（`core::casework::facts_store`）读回——存档里的就是这份结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeIdentReport {
    /// magika 类型标签（如 `pdf`、`elf`、`python`）。
    pub label: String,
    pub mime_type: String,
    /// 大类分组（document / executable / code / archive …）。
    pub group: String,
    pub description: String,
    pub extensions: Vec<String>,
    pub is_text: bool,
    /// 置信度 0~1；规则直判恒为 1。
    pub score: f32,
    /// true = 模型推理；false = 规则直判（空文件、过短文件的 txt/unknown 兜底）。
    pub inferred: bool,
    pub file_len: u64,
}

/// 识别镜像内单个文件的内容类型。读盘在调用方的 `spawn_blocking` 里进行。
pub fn identify_file(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<TypeIdentReport> {
    let mut head: Vec<u8> = Vec::with_capacity(SAMPLE_WINDOW);
    let mut tail: Vec<u8> = Vec::with_capacity(2 * SAMPLE_WINDOW);
    let len = fs_tree::stream_file_chunks(image, node_id, 1024 * 1024, |chunk| {
        if head.len() < SAMPLE_WINDOW {
            let take = (SAMPLE_WINDOW - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        tail.extend_from_slice(chunk);
        if tail.len() > SAMPLE_WINDOW {
            tail.drain(..tail.len() - SAMPLE_WINDOW);
        }
        Ok(())
    })
    .context("读取文件内容失败")?;

    let content_type = inference::identify(&head, &tail, len)?;
    Ok(TypeIdentReport {
        label: content_type.label,
        mime_type: content_type.mime_type,
        group: content_type.group,
        description: content_type.description,
        extensions: content_type.extensions,
        is_text: content_type.is_text,
        score: content_type.score,
        inferred: content_type.inferred,
        file_len: len,
    })
}
