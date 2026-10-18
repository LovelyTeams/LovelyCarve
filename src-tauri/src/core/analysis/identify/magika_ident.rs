//! AI 文件类型识别：Google Magika（standard_v3_3 模型，本地 ONNX 推理）。
//!
//! 模型特征只取文件头、尾各一个 `block_size`（4 KiB）窗口，所以这里用
//! `stream_file_chunks` 滚动保留首尾窗口——任意大小的文件都只占两个窗口
//! 的内存，且尾部特征是真实文件尾，不是截断处。

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;

/// magika 标准模型的 block_size。窗口取小了特征会缺，取大了只是白拷贝。
const WINDOW: usize = 4096;

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

/// ONNX 会话全局复用：模型加载一次（~3 MiB），推理串行即可——单文件推理
/// 在毫秒级，右键批量识别的瓶颈在读盘不在锁。
static SESSION: OnceLock<Result<Mutex<magika::Session>, String>> = OnceLock::new();

fn session() -> anyhow::Result<&'static Mutex<magika::Session>> {
    let slot = SESSION.get_or_init(|| {
        magika::Session::new()
            .map(Mutex::new)
            .map_err(|e| format!("Magika 模型初始化失败：{e}"))
    });
    match slot {
        Ok(session) => Ok(session),
        Err(msg) => Err(anyhow::anyhow!("{msg}")),
    }
}

/// 头尾采样窗口，喂给 magika 的 `SyncInput`。特征提取只会发起两次读：
/// `[0, min(block, len))` 与 `[len - min(block, len), len)`，都落在窗口内。
struct HeadTailInput {
    head: Vec<u8>,
    tail: Vec<u8>,
    len: u64,
}

impl magika::SyncInput for HeadTailInput {
    fn length(&self) -> magika::Result<u64> {
        Ok(self.len)
    }

    fn read_at(&mut self, buffer: &mut [u8], offset: u64) -> magika::Result<()> {
        let want = buffer.len() as u64;
        if offset + want <= self.head.len() as u64 {
            let start = offset as usize;
            buffer.copy_from_slice(&self.head[start..start + buffer.len()]);
            return Ok(());
        }
        let tail_start = self.len - self.tail.len() as u64;
        if offset >= tail_start && offset + want <= self.len {
            let start = (offset - tail_start) as usize;
            buffer.copy_from_slice(&self.tail[start..start + buffer.len()]);
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("magika read outside sampled windows: offset={offset} len={want}"),
        )
        .into())
    }
}

/// 识别镜像内单个文件的内容类型。读盘在调用方的 `spawn_blocking` 里进行。
pub fn identify_file(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<TypeIdentReport> {
    let mut head: Vec<u8> = Vec::with_capacity(WINDOW);
    let mut tail: Vec<u8> = Vec::with_capacity(2 * WINDOW);
    let len = fs_tree::stream_file_chunks(image, node_id, 1024 * 1024, |chunk| {
        if head.len() < WINDOW {
            let take = (WINDOW - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        tail.extend_from_slice(chunk);
        if tail.len() > WINDOW {
            tail.drain(..tail.len() - WINDOW);
        }
        Ok(())
    })
    .context("读取文件内容失败")?;

    let mut input = HeadTailInput { head, tail, len };
    let session = session()?;
    let mut session = session
        .lock()
        .map_err(|_| anyhow::anyhow!("Magika 会话不可用（锁已失效）"))?;
    let file_type = session
        .identify_content_sync(&mut input)
        .context("Magika 推理失败")?;
    drop(session);

    let inferred = match &file_type {
        magika::FileType::Inferred(_) => true,
        magika::FileType::Ruled(_) => false,
        // 我们走的是 content 路径，目录/符号链接两个变体不可达；穷举以防
        // 上游加变体时静默漏分支。
        magika::FileType::Directory | magika::FileType::Symlink => false,
    };
    let info = file_type.info();
    Ok(TypeIdentReport {
        label: info.label.to_string(),
        mime_type: info.mime_type.to_string(),
        group: info.group.to_string(),
        description: info.description.to_string(),
        extensions: info.extensions.iter().map(|e| e.to_string()).collect(),
        is_text: info.is_text,
        score: file_type.score(),
        inferred,
        file_len: len,
    })
}
