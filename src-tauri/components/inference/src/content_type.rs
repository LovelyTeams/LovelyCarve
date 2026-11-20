//! `magika.identify`: Google Magika (standard_v3_3 model, local ONNX
//! inference) over the head and tail windows the host sampled.
//!
//! The model's features only cover one `block_size` (4 KiB) at each end of
//! the file, so the host streams the file once and sends just those two
//! windows; the tail is the real end of the file, not a truncation point.

use std::sync::{Mutex, OnceLock};

use abi::inference::{ContentType, IdentifyParams, METHOD_IDENTIFY};
use abi::CallError;

/// The ONNX session is created once (the model is about 3 MiB) and used
/// serially: one inference takes milliseconds, so batch identification is
/// bound by evidence reads rather than this lock.
static SESSION: OnceLock<Result<Mutex<magika::Session>, String>> = OnceLock::new();

fn session() -> Result<&'static Mutex<magika::Session>, CallError> {
    let slot = SESSION.get_or_init(|| {
        magika::Session::new()
            .map(Mutex::new)
            .map_err(|error| format!("Magika 模型初始化失败：{error}"))
    });
    slot.as_ref()
        .map_err(|message| CallError::Failed(message.clone()))
}

pub(crate) fn identify(params: &[u8], payload: &[u8]) -> Result<ContentType, CallError> {
    let params: IdentifyParams = crate::decode_params(METHOD_IDENTIFY, params)?;
    let mut input = HeadTailInput::new(params, payload)?;
    let session = session()?;
    let mut session = session
        .lock()
        .map_err(|_| CallError::Failed("Magika 会话不可用（锁已失效）".to_string()))?;
    let file_type = session
        .identify_content_sync(&mut input)
        .map_err(|error| CallError::Failed(format!("Magika 推理失败：{error}")))?;
    drop(session);

    let inferred = match &file_type {
        magika::FileType::Inferred(_) => true,
        magika::FileType::Ruled(_) => false,
        // Content identification never yields these two; matching them keeps
        // a new upstream variant from slipping through unnoticed.
        magika::FileType::Directory | magika::FileType::Symlink => false,
    };
    let info = file_type.info();
    Ok(ContentType {
        label: info.label.to_string(),
        mime_type: info.mime_type.to_string(),
        group: info.group.to_string(),
        description: info.description.to_string(),
        extensions: info
            .extensions
            .iter()
            .map(|extension| extension.to_string())
            .collect(),
        is_text: info.is_text,
        score: file_type.score(),
        inferred,
    })
}

/// The sampled windows as magika's `SyncInput`. Feature extraction reads
/// `[0, min(block, len))` and `[len - min(block, len), len)`, both inside the
/// windows; any other read is an error rather than invented bytes.
struct HeadTailInput<'a> {
    head: &'a [u8],
    tail: &'a [u8],
    len: u64,
}

impl<'a> HeadTailInput<'a> {
    fn new(params: IdentifyParams, payload: &'a [u8]) -> Result<Self, CallError> {
        let head_len = usize::try_from(params.head_len)
            .ok()
            .filter(|head_len| *head_len <= payload.len())
            .ok_or_else(|| {
                CallError::InvalidArgument(format!(
                    "{METHOD_IDENTIFY}: head window of {} bytes exceeds the {}-byte payload",
                    params.head_len,
                    payload.len()
                ))
            })?;
        let (head, tail) = payload.split_at(head_len);
        if head.len() as u64 > params.file_len || tail.len() as u64 > params.file_len {
            return Err(CallError::InvalidArgument(format!(
                "{METHOD_IDENTIFY}: windows of {} and {} bytes exceed the {}-byte file",
                head.len(),
                tail.len(),
                params.file_len
            )));
        }
        Ok(Self {
            head,
            tail,
            len: params.file_len,
        })
    }
}

impl magika::SyncInput for HeadTailInput<'_> {
    fn length(&self) -> magika::Result<u64> {
        Ok(self.len)
    }

    fn read_at(&mut self, buffer: &mut [u8], offset: u64) -> magika::Result<()> {
        let want = buffer.len() as u64;
        let outside = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("magika read outside sampled windows: offset={offset} len={want}"),
            )
        };
        let end = offset.checked_add(want).ok_or_else(outside)?;
        if end <= self.head.len() as u64 {
            let start = offset as usize;
            buffer.copy_from_slice(&self.head[start..start + buffer.len()]);
            return Ok(());
        }
        let tail_start = self.len - self.tail.len() as u64;
        if offset >= tail_start && end <= self.len {
            let start = (offset - tail_start) as usize;
            buffer.copy_from_slice(&self.tail[start..start + buffer.len()]);
            return Ok(());
        }
        Err(outside().into())
    }
}
