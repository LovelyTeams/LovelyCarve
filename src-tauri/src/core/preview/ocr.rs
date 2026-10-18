//! 原类型视图「文字识别（OCR）」引擎。用 PP-OCRv6-tiny（检测 + 识别）经 `oar-ocr` 跑 OCR,
//! 复用 magika 同版本的 ONNX Runtime（`ort` 2.0.0-rc.12），纯 Rust、不依赖 OpenCV。三件套模型
//! **外置**于 `resources/ocr/`（不再 `include_bytes!` 内嵌进二进制），运行时经
//! [`crate::core::platform::resources::resource_path`] 从可执行文件旁 / 仓库定位（`oar-ocr` 的构造器
//! 直接吃这些磁盘路径，无需再落临时目录）。
//!
//! 执行提供者：`ocr-cuda` feature 下优先 NVIDIA CUDA、回退 CPU（`oar-ocr` 注册 CUDA EP 不带
//! `error_on_failure`，无 GPU / CUDA 库时自动用 CPU）；仅 `ocr`（无 cuda）时走 CPU。引擎构造
//! 一次后全局缓存（模型只加载一次），`recognize` 串行复用。未启用 `ocr` feature 时 `recognize`
//! 返回带启用指引的错误，保证默认 / 轻量构建都能编译、命令层无需 cfg。

use serde::Serialize;

/// 一个识别出的文字区域。
#[derive(Debug, Clone, Serialize)]
pub struct OcrRegion {
    /// 识别出的文本内容。
    pub text: String,
    /// 识别置信度（0~1）。
    pub confidence: f32,
    /// 文字区域多边形角点 `[x, y]`，**原始图像像素坐标**，供前端叠框。
    pub points: Vec<[f32; 2]>,
}

/// 一次 OCR 的产物。
#[derive(Debug, Clone, Serialize)]
pub struct OcrOutcome {
    /// 全部识别结果（仅保留同时有文本与置信度者）。
    pub regions: Vec<OcrRegion>,
    /// 解码出的原图宽 / 高（像素）—— 前端据此把检测框 SVG 缩放对齐到 `<img>`。
    pub width: u32,
    pub height: u32,
    /// 识别到的文字区域数量。
    pub count: u32,
}

#[cfg(feature = "ocr")]
pub use enabled::recognize;

/// 未启用 `ocr` feature 时的占位实现：与真实 `recognize` 完全同签名，命令层无需 cfg 即可调用；
/// 运行时返回带启用指引的错误。
#[cfg(not(feature = "ocr"))]
pub fn recognize(_bytes: &[u8]) -> anyhow::Result<OcrOutcome> {
    anyhow::bail!(
        "文字识别（OCR）未启用：请用 `just build-full`（或 `just dev-full`）构建；\
         完整构建启用 OCR，无可用 CUDA 时回退 CPU。"
    )
}

#[cfg(feature = "ocr")]
mod enabled {
    use super::{OcrOutcome, OcrRegion};
    use crate::core::platform::resources::resource_path;
    use anyhow::Context;
    use oar_ocr::domain::tasks::TextDetectionConfig;
    use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
    use std::sync::{Mutex, OnceLock};

    /// 对图片字节跑 OCR：解码 → 复用全局引擎 `predict` → 收集有文本的区域（点坐标为原图像素）。
    pub fn recognize(bytes: &[u8]) -> anyhow::Result<OcrOutcome> {
        let img = image::load_from_memory(bytes)
            .context("解码图片失败")?
            .to_rgb8();

        let engine = engine()?;
        let result = {
            let guard = engine
                .lock()
                .map_err(|_| anyhow::anyhow!("OCR 引擎不可用（锁已失效）"))?;
            let results = guard
                .predict(vec![img])
                .map_err(|e| anyhow::anyhow!("OCR 推理失败：{e}"))?;
            results.into_iter().next().context("OCR 未返回结果")?
        };

        let width = result.input_img.width();
        let height = result.input_img.height();
        let mut regions = Vec::new();
        for region in &result.text_regions {
            // 仅保留同时有文本与置信度的区域；检测到但识别为空的框对取证无意义。
            if let Some((text, confidence)) = region.text_with_confidence() {
                let points = region
                    .bounding_box
                    .points
                    .iter()
                    .map(|p| [p.x, p.y])
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

    /// 全局缓存的 OCR 引擎（ONNX 模型只加载一次）。`predict` 取 `&self`，用 `Mutex` 包裹以便
    /// 跨 `spawn_blocking` 线程共享并串行复用——OCR 是手动触发的低频操作，串行可接受。
    fn engine() -> anyhow::Result<&'static Mutex<OAROCR>> {
        static ENGINE: OnceLock<Mutex<OAROCR>> = OnceLock::new();
        if ENGINE.get().is_none() {
            let built = build_engine()?;
            // set 失败仅意味着另一线程已抢先构造好——用对方的即可（多构造一次无害）。
            let _ = ENGINE.set(Mutex::new(built));
        }
        ENGINE.get().context("OCR 引擎初始化失败")
    }

    /// 用外置的三件套模型（`resources/ocr/`，见 [`resource_path`]）+ 配置好的执行提供者构造
    /// OCR 管线。检测阈值使用官方 PP-OCRv6 默认值。
    fn build_engine() -> anyhow::Result<OAROCR> {
        let det = resource_path("ocr/pp-ocrv6_tiny_det.onnx")?;
        let rec = resource_path("ocr/pp-ocrv6_tiny_rec.onnx")?;
        let dict = resource_path("ocr/ppocrv6_tiny_dict.txt")?;
        OAROCRBuilder::new(det, rec, dict)
            .ort_session(ep_config())
            .text_detection_config(TextDetectionConfig {
                score_threshold: 0.2,
                box_threshold: 0.45,
                unclip_ratio: 1.4,
                ..Default::default()
            })
            .build()
            .map_err(|e| anyhow::anyhow!("构建 OCR 引擎失败：{e}"))
    }

    /// `ocr-cuda` 下：CUDA 优先、CPU 回退。CUDA EP 注册失败（无 GPU / CUDA 库）会自动落到 CPU。
    #[cfg(feature = "ocr-cuda")]
    fn ep_config() -> oar_ocr_core::core::config::OrtSessionConfig {
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

    /// 仅 `ocr`（无 cuda）：默认 CPU。不可传 CUDA 变体——`oar-ocr-core` 无 cuda feature 时会报
    /// 「requested but feature not enabled」。
    #[cfg(not(feature = "ocr-cuda"))]
    fn ep_config() -> oar_ocr_core::core::config::OrtSessionConfig {
        oar_ocr_core::core::config::OrtSessionConfig::new()
    }
}
