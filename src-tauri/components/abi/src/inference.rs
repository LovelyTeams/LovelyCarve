//! Contract of the inference component (`lovelycarve_inference`): content
//! type identification with Google Magika and, in full builds, PP-OCRv6 text
//! recognition, both on the ONNX Runtime the component links statically.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Library name without platform prefix or suffix
/// (`lovelycarve_inference.dll`, `liblovelycarve_inference.so`, …).
pub const LIBRARY: &str = "lovelycarve_inference";

/// Magika's block size: the most it reads at each end of a file, and so the
/// window the host samples there.
pub const SAMPLE_WINDOW: usize = 4096;

/// Identifies content from sampled windows. Params: [`IdentifyParams`];
/// payload: the head window followed by the tail window; result:
/// [`ContentType`].
pub const METHOD_IDENTIFY: &str = "magika.identify";

/// Recognizes text in an encoded image. Params: [`RecognizeParams`];
/// payload: the image file bytes; result: [`OcrOutcome`].
pub const METHOD_RECOGNIZE: &str = "ocr.recognize";

/// What one build of the component can do (the `capabilities` of its
/// [`crate::Description`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// [`METHOD_IDENTIFY`] is available (every build).
    pub file_types: bool,
    /// How [`METHOD_RECOGNIZE`] runs; `None` when the build has no OCR.
    pub ocr: Option<OcrExecution>,
}

/// Execution provider order of an OCR-enabled build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OcrExecution {
    /// CPU only.
    Cpu,
    /// NVIDIA CUDA first, falling back to the CPU when no usable GPU or CUDA
    /// libraries are present.
    Cuda,
}

/// Params of [`METHOD_IDENTIFY`]. The payload holds `head_len` bytes from
/// the start of the file followed by the last bytes of the file; each window
/// is at most the model's block size and at most the file length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyParams {
    pub head_len: u64,
    pub file_len: u64,
}

/// Result of [`METHOD_IDENTIFY`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentType {
    /// Magika label such as `pdf`, `elf` or `python`.
    pub label: String,
    pub mime_type: String,
    /// Group such as document, executable, code or archive.
    pub group: String,
    pub description: String,
    pub extensions: Vec<String>,
    pub is_text: bool,
    /// Confidence between 0 and 1; always 1 for rule decisions.
    pub score: f32,
    /// True for a model inference, false for a rule decision (empty or very
    /// short content).
    pub inferred: bool,
}

/// Params of [`METHOD_RECOGNIZE`]: the PP-OCRv6 model files the host located
/// beside its executable. The first call builds the engine from them; later
/// calls reuse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecognizeParams {
    pub detection_model: PathBuf,
    pub recognition_model: PathBuf,
    pub dictionary: PathBuf,
}

/// One recognized text region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrRegion {
    /// Recognized text.
    pub text: String,
    /// Recognition confidence between 0 and 1.
    pub confidence: f32,
    /// Corner points `[x, y]` of the region in original image pixels, so the
    /// frontend can draw the box.
    pub points: Vec<[f32; 2]>,
}

/// Result of [`METHOD_RECOGNIZE`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrOutcome {
    /// Regions that have both text and a confidence.
    pub regions: Vec<OcrRegion>,
    /// Decoded image width and height in pixels; the frontend scales the
    /// region overlay to the displayed image with them.
    pub width: u32,
    pub height: u32,
    /// Number of regions.
    pub count: u32,
}
