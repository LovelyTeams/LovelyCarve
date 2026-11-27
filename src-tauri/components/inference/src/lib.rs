//! LovelyCarve inference component (`lovelycarve_inference`).
//!
//! Keeps ONNX Runtime, the embedded Magika model and the optional PP-OCRv6
//! pipeline out of the executables: they load this library through the C ABI
//! of `lovelycarve-component-abi` the first time a file type is identified
//! or text is recognized. The methods and their payloads are listed in
//! `abi::inference`.

use abi::inference::{self, Capabilities, ContentType, OcrExecution};
use abi::{Buffer, Bytes, CallError, ComponentApiV1, Description};

// ONNX Runtime publishes no prebuilt library for Intel Macs, so the component
// built there carries no Magika model (see Cargo.toml).
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
mod content_type;
mod ocr;

/// Whether this build links ONNX Runtime and the Magika model.
const FILE_TYPES: bool = cfg!(not(all(target_os = "macos", target_arch = "x86_64")));

/// Entry point the host resolves after loading the library.
///
/// # Safety
///
/// `api` must be null or point to writable memory for one [`ComponentApiV1`].
#[no_mangle]
pub unsafe extern "C" fn lovelycarve_component_v1(api: *mut ComponentApiV1) -> i32 {
    unsafe { abi::serve_entry(api, call, free_buffer) }
}

unsafe extern "C" fn call(method: Bytes, params: Bytes, payload: Bytes, out: *mut Buffer) -> i32 {
    unsafe { abi::serve_call(method, params, payload, out, dispatch) }
}

unsafe extern "C" fn free_buffer(buffer: *mut Buffer) {
    unsafe { abi::serve_free_buffer(buffer) }
}

fn dispatch(method: &str, params: &[u8], payload: &[u8]) -> Result<Vec<u8>, CallError> {
    match method {
        abi::METHOD_DESCRIBE => encode(&description()),
        inference::METHOD_IDENTIFY => encode(&identify(params, payload)?),
        inference::METHOD_RECOGNIZE => encode(&ocr::recognize(params, payload)?),
        other => Err(CallError::InvalidArgument(format!(
            "{} does not implement method `{other}`",
            inference::LIBRARY
        ))),
    }
}

fn description() -> Description<Capabilities> {
    Description {
        name: inference::LIBRARY.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: Capabilities {
            file_types: FILE_TYPES,
            ocr: ocr_execution(),
        },
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
fn identify(params: &[u8], payload: &[u8]) -> Result<ContentType, CallError> {
    content_type::identify(params, payload)
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn identify(_params: &[u8], _payload: &[u8]) -> Result<ContentType, CallError> {
    Err(CallError::Failed(
        "本平台的推理组件不含 ONNX Runtime（Intel Mac 没有预编译库），无法识别文件类型".to_string(),
    ))
}

fn ocr_execution() -> Option<OcrExecution> {
    if cfg!(feature = "ocr-cuda") {
        Some(OcrExecution::Cuda)
    } else if cfg!(feature = "ocr") {
        Some(OcrExecution::Cpu)
    } else {
        None
    }
}

fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, CallError> {
    serde_json::to_vec(value)
        .map_err(|error| CallError::Failed(format!("编码组件应答失败：{error}")))
}

/// Parses JSON params, reporting malformed input as an invalid argument.
fn decode_params<T: serde::de::DeserializeOwned>(
    method: &str,
    params: &[u8],
) -> Result<T, CallError> {
    serde_json::from_slice(params).map_err(|error| {
        CallError::InvalidArgument(format!("{method} params are invalid: {error}"))
    })
}

#[cfg(test)]
mod tests;
