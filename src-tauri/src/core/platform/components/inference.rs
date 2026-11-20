//! Client of the inference component (`lovelycarve_inference`): Magika
//! content types and, in full builds, PP-OCRv6 text recognition. The
//! executables do not link ONNX Runtime or the models themselves; the library
//! is loaded the first time one of these functions needs it.

use std::sync::OnceLock;

use anyhow::bail;
use lovelycarve_component_abi::inference::{
    Capabilities, ContentType, IdentifyParams, OcrExecution, OcrOutcome, RecognizeParams, LIBRARY,
    METHOD_IDENTIFY, METHOD_RECOGNIZE,
};
use lovelycarve_component_abi::{Description, METHOD_DESCRIBE};

use super::Component;

pub use lovelycarve_component_abi::inference::SAMPLE_WINDOW;

/// The loaded component and what its build can do.
pub struct Inference {
    component: Component,
    description: Description<Capabilities>,
}

impl Inference {
    pub fn path(&self) -> &std::path::Path {
        self.component.path()
    }

    pub fn version(&self) -> &str {
        &self.description.version
    }

    pub fn capabilities(&self) -> Capabilities {
        self.description.capabilities
    }
}

static LOADED: OnceLock<Inference> = OnceLock::new();

/// Loads the component on first use. A failed load is not cached, so a
/// component installed later is picked up by the next call.
pub fn load() -> anyhow::Result<&'static Inference> {
    if let Some(loaded) = LOADED.get() {
        return Ok(loaded);
    }
    let component = Component::load(LIBRARY)?;
    let description: Description<Capabilities> = component.call(METHOD_DESCRIBE, &(), &[])?;
    if description.name != LIBRARY {
        bail!(
            "{} 报告自己是组件 {}，不是 {LIBRARY}",
            component.path().display(),
            description.name
        );
    }
    // Another thread may have loaded it first; both handles are equivalent.
    Ok(LOADED.get_or_init(|| Inference {
        component,
        description,
    }))
}

/// How OCR runs in the component, if it has been loaded already. Never loads
/// it, so resource planning can ask without touching the disk.
pub fn loaded_ocr_execution() -> Option<OcrExecution> {
    LOADED
        .get()
        .and_then(|inference| inference.description.capabilities.ocr)
}

/// Identifies a file's content type from its first and last
/// [`SAMPLE_WINDOW`] bytes (each the whole file when it is shorter).
pub fn identify(head: &[u8], tail: &[u8], file_len: u64) -> anyhow::Result<ContentType> {
    let params = IdentifyParams {
        head_len: head.len() as u64,
        file_len,
    };
    load()?
        .component
        .call(METHOD_IDENTIFY, &params, &[head, tail].concat())
}

/// Recognizes text in an encoded image with the given PP-OCRv6 models.
pub fn recognize(image: &[u8], models: &RecognizeParams) -> anyhow::Result<OcrOutcome> {
    load()?.component.call(METHOD_RECOGNIZE, models, image)
}
