//! 原类型视图「文字识别（OCR）」。PP-OCRv6-tiny（检测 + 识别）经 `oar-ocr` 在推理组件
//! `lovelycarve_inference` 里运行（见 [`crate::core::platform::components::inference`]），
//! 与 Magika 共用组件内的 ONNX Runtime，主程序不链接它们。三件套模型**外置**于
//! `resources/ocr/`，由这里经 [`resource_path`] 从可执行文件旁 / 仓库定位后把路径交给组件。
//!
//! 是否带 OCR、走 CUDA 还是 CPU 由组件的构建特性决定（完整构建为 CUDA 优先、CPU 回退）；
//! 组件不带 OCR 时 [`ensure_available`] / [`recognize`] 返回带启用指引的错误。

use anyhow::bail;
use lovelycarve_component_abi::inference::RecognizeParams;

use crate::core::platform::components::inference;
use crate::core::platform::resources::resource_path;

pub use lovelycarve_component_abi::inference::{OcrOutcome, OcrRegion};

/// 加载推理组件并确认它带 OCR。命令在登记任务前调用，组件的执行方式决定任务占 GPU 还是 CPU 槽位。
pub fn ensure_available() -> anyhow::Result<()> {
    if inference::load()?.capabilities().ocr.is_none() {
        bail!(
            "文字识别（OCR）未启用：请用 `just build-full`（或 `just dev-full`）构建；\
             完整构建启用 OCR，无可用 CUDA 时回退 CPU。"
        );
    }
    Ok(())
}

/// 对图片字节跑 OCR，区域点坐标为原图像素。
pub fn recognize(bytes: &[u8]) -> anyhow::Result<OcrOutcome> {
    ensure_available()?;
    let models = RecognizeParams {
        detection_model: resource_path("ocr/pp-ocrv6_tiny_det.onnx")?,
        recognition_model: resource_path("ocr/pp-ocrv6_tiny_rec.onnx")?,
        dictionary: resource_path("ocr/ppocrv6_tiny_dict.txt")?,
    };
    inference::recognize(bytes, &models)
}
