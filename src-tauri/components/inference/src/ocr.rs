//! `ocr.recognize`: PP-OCRv6-tiny text detection and recognition through
//! `oar-ocr`, on the same ONNX Runtime as Magika and without OpenCV.
//!
//! The three model files stay outside the library (`resources/ocr/` beside
//! the executable); the host locates them and passes their paths. The engine
//! is built once and reused serially. With `ocr-cuda` it prefers NVIDIA CUDA
//! and falls back to the CPU (`oar-ocr` registers the CUDA provider without
//! `error_on_failure`). Builds without `ocr` report `ocr: null` in their
//! capabilities, and the host explains how to get a full build.

#[cfg(feature = "ocr")]
pub(crate) use enabled::recognize;

/// Builds without the `ocr` feature keep the method so a direct call fails
/// clearly instead of as an unknown method.
#[cfg(not(feature = "ocr"))]
pub(crate) fn recognize(
    _params: &[u8],
    _payload: &[u8],
) -> Result<abi::inference::OcrOutcome, abi::CallError> {
    Err(abi::CallError::Failed(format!(
        "this build of {} has no OCR (build it with the `ocr` feature)",
        abi::inference::LIBRARY
    )))
}

#[cfg(feature = "ocr")]
mod enabled {
    use std::sync::{Mutex, OnceLock};

    use abi::inference::{OcrOutcome, OcrRegion, RecognizeParams, METHOD_RECOGNIZE};
    use abi::CallError;
    use oar_ocr::domain::tasks::TextDetectionConfig;
    use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};

    /// Decodes the image, runs the shared engine and keeps the regions that
    /// carry text (points in original image pixels).
    pub(crate) fn recognize(params: &[u8], payload: &[u8]) -> Result<OcrOutcome, CallError> {
        let params: RecognizeParams = crate::decode_params(METHOD_RECOGNIZE, params)?;
        let image = image::load_from_memory(payload)
            .map_err(|error| CallError::Failed(format!("解码图片失败：{error}")))?
            .to_rgb8();

        let engine = engine(&params)?;
        let result = {
            let guard = engine
                .lock()
                .map_err(|_| CallError::Failed("OCR 引擎不可用（锁已失效）".to_string()))?;
            let results = guard
                .predict(vec![image])
                .map_err(|error| CallError::Failed(format!("OCR 推理失败：{error}")))?;
            results
                .into_iter()
                .next()
                .ok_or_else(|| CallError::Failed("OCR 未返回结果".to_string()))?
        };

        let width = result.input_img.width();
        let height = result.input_img.height();
        let mut regions = Vec::new();
        for region in &result.text_regions {
            // Detected boxes without recognized text say nothing about the evidence.
            if let Some((text, confidence)) = region.text_with_confidence() {
                let points = region
                    .bounding_box
                    .points
                    .iter()
                    .map(|point| [point.x, point.y])
                    .collect();
                regions.push(OcrRegion {
                    text: text.to_string(),
                    confidence,
                    points,
                });
            }
        }
        let count = regions.len() as u32;
        Ok(OcrOutcome {
            regions,
            width,
            height,
            count,
        })
    }

    /// The process-wide engine; the ONNX models load once. `predict` takes
    /// `&self`, and the mutex serializes the infrequent, manually triggered
    /// OCR calls across host threads.
    fn engine(params: &RecognizeParams) -> Result<&'static Mutex<OAROCR>, CallError> {
        static ENGINE: OnceLock<Mutex<OAROCR>> = OnceLock::new();
        if let Some(engine) = ENGINE.get() {
            return Ok(engine);
        }
        let built = build_engine(params)?;
        // Another thread may have finished first; its engine is equivalent.
        Ok(ENGINE.get_or_init(|| Mutex::new(built)))
    }

    /// Builds the pipeline from the external models with the configured
    /// execution providers and the official PP-OCRv6 detection thresholds.
    fn build_engine(params: &RecognizeParams) -> Result<OAROCR, CallError> {
        OAROCRBuilder::new(
            params.detection_model.clone(),
            params.recognition_model.clone(),
            params.dictionary.clone(),
        )
        .ort_session(execution_providers())
        .text_detection_config(TextDetectionConfig {
            score_threshold: 0.2,
            box_threshold: 0.45,
            unclip_ratio: 1.4,
            ..Default::default()
        })
        .build()
        .map_err(|error| CallError::Failed(format!("构建 OCR 引擎失败：{error}")))
    }

    /// `ocr-cuda`: CUDA first, CPU fallback. A CUDA provider that fails to
    /// register (no GPU or CUDA libraries) falls through to the CPU.
    #[cfg(feature = "ocr-cuda")]
    fn execution_providers() -> oar_ocr_core::core::config::OrtSessionConfig {
        use oar_ocr_core::core::config::{OrtExecutionProvider, OrtSessionConfig};
        OrtSessionConfig::new().with_execution_providers(vec![
            OrtExecutionProvider::CUDA {
                device_id: Some(0),
                gpu_mem_limit: None,
                arena_extend_strategy: None,
                cudnn_conv_algo_search: None,
                cudnn_conv_use_max_workspace: None,
            },
            OrtExecutionProvider::CPU,
        ])
    }

    /// `ocr` without CUDA: the default CPU provider. A CUDA variant would
    /// fail here because `oar-ocr-core` lacks its cuda feature.
    #[cfg(not(feature = "ocr-cuda"))]
    fn execution_providers() -> oar_ocr_core::core::config::OrtSessionConfig {
        oar_ocr_core::core::config::OrtSessionConfig::new()
    }
}
