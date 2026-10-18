//! 原类型视图「二维码识别」引擎。模仿 LoveLy-QRCode-Scanner：用 OpenCV 的检测器
//! 配合「暴力变换」—— 对图像做 灰度 / 对比度 / 亮度 / 高斯模糊 / 缩放 / 反色 / 二值化
//! 的组合后反复尝试解码，直到凑齐期望数量或穷举完毕。借此把被 AI 生成、退化、
//! 局部遮挡的二维码也尽量解出来。变换扫描覆盖到「重高斯模糊」（核最大 25，与
//! LoveLy-QRCode-Scanner 一致）—— 重模糊能把 AI 艺术化纹理抹平到只剩定位/校正图案，
//! 因此即便标准检测器也能解出这类退化二维码。
//!
//! 扫描按「成本感知」分阶段排序（标准检测器的单次成本主要由图像面积决定，重模糊
//! 又只在小尺寸上才划算）：① 原尺寸几乎不变换、命中即秒退干净码；② 小尺寸（每次
//! 便宜）配重高斯模糊，低成本攻坚 AI 艺术化 / 退化码（实测 Tiger 类码本阶段首次即解）；
//! ③ 极小尺寸兜底更糊更小的码；④ 大尺寸只做轻处理，规避重模糊在大图上的高耗时；
//! ⑤ 反色 + 强曝光小尺寸兜底。如此把艺术化码从「约 1300 次尝试 / 数十秒」降到亚秒级。
//!
//! 检测器分两档：`qrcode` 用标准 `objdetect::QRCodeDetector`（官方 OpenCV 预编译包即可）；
//! `qrcode-wechat` 另启用 WeChat 检测器（需 OpenCV 带 contrib）。
//! opencv crate 仅在这两个 feature 下编译（需系统 OpenCV + LLVM）；未启用时 `scan`
//! 直接返回带指引的错误，保证默认构建完全不依赖 OpenCV。

use serde::Serialize;

/// 一个识别出的二维码。
#[derive(Debug, Clone, Serialize)]
pub struct QrFinding {
    /// 解码后的文本内容。
    pub text: String,
    /// 4 个角点 `[x, y]`，**原始图像像素坐标**（已按缩放因子还原），供前端画框。
    /// 标准检测器可靠给出 4 点；WeChat 检测器个别情况下可能为空，此时仅展示文本。
    pub points: Vec<[f32; 2]>,
    /// 命中该码的变换组合描述（如「缩放×2 · 灰度 · 二值化」），便于复现。
    pub transform: String,
}

/// 一次扫描的产物。
#[derive(Debug, Clone, Serialize)]
pub struct QrScanOutcome {
    /// 去重后的全部识别结果。
    pub codes: Vec<QrFinding>,
    /// 解码出的原图宽 / 高（像素）—— 前端据此把检测框 SVG 缩放对齐到 `<img>`。
    pub width: u32,
    pub height: u32,
    /// 实际尝试的变换组合次数。
    pub attempts: u32,
    /// 是否被用户取消（取消时返回已找到的部分结果）。
    pub cancelled: bool,
    /// 实际使用的检测器：`"wechat"` 或 `"standard"`。
    pub detector: String,
}

#[cfg(feature = "qrcode-wechat")]
pub use enabled::scan;

/// 未启用 `qrcode-wechat` feature 时的占位实现：保持与真实 `scan` 完全一致的签名，
/// 命令层无需 cfg 即可调用；运行时返回带启用指引的错误。
#[cfg(not(feature = "qrcode-wechat"))]
pub fn scan(
    _bytes: &[u8],
    _expected: u32,
    _on_progress: impl FnMut(u32, u32, u32),
    _is_cancelled: impl Fn() -> bool,
) -> anyhow::Result<QrScanOutcome> {
    anyhow::bail!(
        "二维码识别未启用：请用 `just build-full`（或 `just dev-full`）构建，并预装带 contrib（wechat_qrcode）的 OpenCV + LLVM 后重试。"
    )
}

#[cfg(feature = "qrcode-wechat")]
mod enabled {
    use super::{QrFinding, QrScanOutcome};
    #[cfg(feature = "qrcode-wechat")]
    use crate::core::platform::resources::resource_path;
    use anyhow::Context;
    use opencv::core::{self, Mat, Point2f, Size, Vector};
    use opencv::prelude::*;
    #[cfg(feature = "qrcode-wechat")]
    use opencv::wechat_qrcode;
    use opencv::{imgcodecs, imgproc, objdetect};

    // ---- 暴力变换参数集（成本感知、分阶段排序，命中即退出）----
    /// 一组「同构」的变换参数（按阶段分组）。阶段按「便宜且高命中优先」排序：
    /// 先在原尺寸做极少量尝试秒退干净码，再用小尺寸 + 重高斯模糊低成本地攻坚
    /// AI 艺术化 / 退化二维码，最后才在大尺寸只做轻处理（避免重模糊在大图上的高耗时）。
    struct Stage {
        scales: &'static [f64],
        contrasts: &'static [f64],
        brightnesses: &'static [f64],
        blurs: &'static [i32],
        binarize: &'static [bool],
        invert: &'static [bool],
    }

    const STAGES: &[Stage] = &[
        // 1) 干净码快速通道：原尺寸、几乎不变换，命中即秒退（含反色/二值化覆盖普通与反相码）。
        Stage {
            scales: &[1.0],
            contrasts: &[1.0],
            brightnesses: &[0.0],
            blurs: &[0],
            binarize: &[false, true],
            invert: &[false, true],
        },
        // 2) 艺术化/退化码主力：小尺寸（每次便宜）+ 重模糊（把艺术纹理抹平到只剩定位图案）。
        //    对比度 2 与重模糊 23 排在最前 —— 实测 Tiger 类码在本阶段第 1 次尝试即解出。
        Stage {
            scales: &[0.5, 0.4, 0.6, 0.7],
            contrasts: &[2.0, 3.0, 1.0],
            brightnesses: &[0.0, -25.0],
            blurs: &[23, 25, 21, 19, 17, 15, 13, 11, 9, 7, 5, 3, 0],
            binarize: &[false, true],
            invert: &[false],
        },
        // 3) 极小尺寸兜底：更便宜，覆盖更糊/更小的码。
        Stage {
            scales: &[0.3, 0.25, 0.2],
            contrasts: &[2.0, 1.0, 3.0],
            brightnesses: &[0.0],
            blurs: &[0, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25],
            binarize: &[false, true],
            invert: &[false],
        },
        // 4) 大尺寸仅轻处理：照顾小幅/高密度码，只用轻模糊，规避大图重模糊的高耗时。
        Stage {
            scales: &[1.0, 1.3, 2.0],
            contrasts: &[1.0, 2.0, 3.0],
            brightnesses: &[0.0, -25.0, 25.0],
            blurs: &[0, 3, 5],
            binarize: &[false, true],
            invert: &[false],
        },
        // 5) 反色 + 强曝光兜底：小尺寸便宜地补全反相 / 过曝 / 欠曝场景。
        Stage {
            scales: &[0.5, 0.3],
            contrasts: &[2.0, 1.0, 3.0],
            brightnesses: &[-75.0, -50.0, -25.0, 50.0, 75.0],
            blurs: &[0, 11, 23],
            binarize: &[false, true],
            invert: &[true],
        },
    ];

    /// 判定两个角点集是否指向「同一个码」的中心距阈值（像素）。
    const DUP_DIST: f32 = 50.0;

    /// 检测器抽象：WeChat（需 contrib）优先，标准 `QRCodeDetector` 兜底。
    enum Detector {
        #[cfg(feature = "qrcode-wechat")]
        WeChat(wechat_qrcode::WeChatQRCode),
        Standard(objdetect::QRCodeDetector),
    }

    impl Detector {
        /// 对单张（已变换的）图像检测并解码，返回 `(文本, 角点)` 列表（角点为该图坐标系）。
        fn detect(&mut self, img: &Mat) -> opencv::Result<Vec<(String, Vec<[f32; 2]>)>> {
            match self {
                #[cfg(feature = "qrcode-wechat")]
                Detector::WeChat(d) => {
                    let mut pts = Vector::<Mat>::new();
                    let texts = d.detect_and_decode(img, &mut pts)?;
                    let mut out = Vec::with_capacity(texts.len());
                    for i in 0..texts.len() {
                        let text = texts.get(i)?;
                        let corners = pts.get(i).ok().map(|m| corners_of(&m)).unwrap_or_default();
                        out.push((text, corners));
                    }
                    Ok(out)
                }
                Detector::Standard(d) => {
                    let mut texts = Vector::<String>::new();
                    let mut pts = Mat::default();
                    let mut straight = Vector::<Mat>::new();
                    let found =
                        d.detect_and_decode_multi(img, &mut texts, &mut pts, &mut straight)?;
                    if !found {
                        return Ok(Vec::new());
                    }
                    // 标准检测器把所有码的 4 角点平铺进一个 Mat（每码 4 点，按码顺序）。
                    let flat = corners_of(&pts);
                    let mut out = Vec::with_capacity(texts.len());
                    for i in 0..texts.len() {
                        let text = texts.get(i)?;
                        let corners = flat
                            .get(i * 4..i * 4 + 4)
                            .map(<[[f32; 2]]>::to_vec)
                            .unwrap_or_default();
                        out.push((text, corners));
                    }
                    Ok(out)
                }
            }
        }
    }

    /// 从角点 `Mat` 读出 `[x, y]` 列表，兼容 `CV_32FC2`（Point2f）与 `CV_32F`(N×2) 两种布局。
    fn corners_of(m: &Mat) -> Vec<[f32; 2]> {
        if m.empty() {
            return Vec::new();
        }
        if let Ok(pts) = m.data_typed::<Point2f>() {
            return pts.iter().map(|p| [p.x, p.y]).collect();
        }
        if let Ok(flat) = m.data_typed::<f32>() {
            return flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
        }
        Vec::new()
    }

    /// 扫描图像字节中的二维码。`expected` 为期望数量（达到即停），`on_progress`
    /// 收到 `(已尝试, 总组合数, 已找到)`，`is_cancelled` 用于协作式取消。
    pub fn scan(
        bytes: &[u8],
        expected: u32,
        mut on_progress: impl FnMut(u32, u32, u32),
        is_cancelled: impl Fn() -> bool,
    ) -> anyhow::Result<QrScanOutcome> {
        let buf = Vector::<u8>::from_slice(bytes);
        let base =
            imgcodecs::imdecode(&buf, imgcodecs::IMREAD_COLOR).context("opencv 解码图像失败")?;
        if base.empty() {
            anyhow::bail!("无法解码为图像（可能不是受支持的位图格式）");
        }
        let width = base.cols().max(0) as u32;
        let height = base.rows().max(0) as u32;

        let (mut detector, detector_name) = build_detector()?;

        // 成本感知分阶段：总次数为各阶段笛卡尔积之和（阶段顺序见 STAGES 注释）。
        let total: u32 = STAGES
            .iter()
            .map(|s| {
                (s.scales.len()
                    * s.invert.len()
                    * s.binarize.len()
                    * s.contrasts.len()
                    * s.brightnesses.len()
                    * s.blurs.len()) as u32
            })
            .sum();
        let mut attempts = 0u32;
        let mut codes: Vec<QrFinding> = Vec::new();
        let mut cancelled = false;

        'sweep: for stage in STAGES {
            for &factor in stage.scales {
                let scaled = match resized(&base, factor) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                for &invert in stage.invert {
                    for &binarize in stage.binarize {
                        for &contrast in stage.contrasts {
                            for &brightness in stage.brightnesses {
                                for &blur in stage.blurs {
                                    if is_cancelled() {
                                        cancelled = true;
                                        break 'sweep;
                                    }
                                    attempts += 1;
                                    on_progress(attempts, total, codes.len() as u32);

                                    let adjusted = match adjust(
                                        &scaled, invert, binarize, contrast, brightness, blur,
                                    ) {
                                        Ok(m) => m,
                                        Err(_) => continue,
                                    };
                                    let hits = match detector.detect(&adjusted) {
                                        Ok(h) => h,
                                        Err(_) => continue,
                                    };
                                    for (text, corners) in hits {
                                        if text.is_empty() {
                                            continue;
                                        }
                                        // 角点从「缩放后图」还原回原图坐标。
                                        let points: Vec<[f32; 2]> = corners
                                            .iter()
                                            .map(|p| [p[0] / factor as f32, p[1] / factor as f32])
                                            .collect();
                                        if is_duplicate(&codes, &text, &points) {
                                            continue;
                                        }
                                        codes.push(QrFinding {
                                            text,
                                            points,
                                            transform: describe(
                                                factor, invert, binarize, contrast, brightness,
                                                blur,
                                            ),
                                        });
                                        if codes.len() as u32 >= expected {
                                            break 'sweep;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(QrScanOutcome {
            codes,
            width,
            height,
            attempts,
            cancelled,
            detector: detector_name,
        })
    }

    /// 优先构造 WeChat 检测器（仅 `qrcode-wechat`，带捆绑模型）；否则用标准检测器。
    fn build_detector() -> anyhow::Result<(Detector, String)> {
        #[cfg(feature = "qrcode-wechat")]
        {
            match wechat_detector() {
                Ok(d) => return Ok((Detector::WeChat(d), "wechat".to_string())),
                Err(err) => {
                    tracing::warn!("WeChat 二维码检测器不可用，改用标准检测器：{err}");
                }
            }
        }
        let d = objdetect::QRCodeDetector::default().context("构造标准 QRCodeDetector 失败")?;
        Ok((Detector::Standard(d), "standard".to_string()))
    }

    /// 用外置的 4 个 caffe 模型（`resources/qrcode/`，见 [`resource_path`]）构造 WeChat 检测器。
    /// WeChat 检测器只接受磁盘路径，直接使用这些外置文件。
    #[cfg(feature = "qrcode-wechat")]
    fn wechat_detector() -> anyhow::Result<wechat_qrcode::WeChatQRCode> {
        let p = |rel: &str| -> anyhow::Result<String> {
            Ok(resource_path(rel)?.to_string_lossy().into_owned())
        };
        let detector = wechat_qrcode::WeChatQRCode::new(
            &p("qrcode/detect.prototxt")?,
            &p("qrcode/detect.caffemodel")?,
            &p("qrcode/sr.prototxt")?,
            &p("qrcode/sr.caffemodel")?,
        )
        .context("初始化 WeChat 检测器失败")?;
        Ok(detector)
    }

    /// 按缩放因子重采样；因子≈1 时原样返回。
    fn resized(src: &Mat, factor: f64) -> opencv::Result<Mat> {
        if (factor - 1.0).abs() < f64::EPSILON {
            return src.try_clone();
        }
        let w = (src.cols() as f64 * factor).round() as i32;
        let h = (src.rows() as f64 * factor).round() as i32;
        if w < 1 || h < 1 {
            return src.try_clone();
        }
        let mut dst = Mat::default();
        imgproc::resize(
            src,
            &mut dst,
            Size::new(w, h),
            0.0,
            0.0,
            imgproc::INTER_LINEAR,
        )?;
        Ok(dst)
    }

    /// 图像预处理链：反色 → 灰度 → 二值化 → 对比度/亮度 → 模糊。
    /// 始终转灰度，二值化仅在单通道上做。
    fn adjust(
        src: &Mat,
        invert: bool,
        binarize: bool,
        contrast: f64,
        brightness: f64,
        blur: i32,
    ) -> opencv::Result<Mat> {
        let mut work = src.try_clone()?;
        if invert {
            let mut dst = Mat::default();
            core::bitwise_not_def(&work, &mut dst)?;
            work = dst;
        }
        // 转灰度。
        {
            let mut dst = Mat::default();
            imgproc::cvt_color_def(&work, &mut dst, imgproc::COLOR_BGR2GRAY)?;
            work = dst;
        }
        if binarize && work.channels() == 1 {
            let mut dst = Mat::default();
            imgproc::threshold(
                &work,
                &mut dst,
                128.0,
                255.0,
                imgproc::THRESH_BINARY | imgproc::THRESH_OTSU,
            )?;
            work = dst;
        }
        if (contrast - 1.0).abs() > f64::EPSILON || brightness != 0.0 {
            let mut dst = Mat::default();
            core::convert_scale_abs(&work, &mut dst, contrast, brightness)?;
            work = dst;
        }
        if blur > 0 {
            let k = if blur % 2 == 0 { blur + 1 } else { blur };
            let mut dst = Mat::default();
            imgproc::gaussian_blur_def(&work, &mut dst, Size::new(k, k), 0.0)?;
            work = dst;
        }
        Ok(work)
    }

    /// 同一个码常在多种变换下被反复解出：文本相同且中心点足够接近 → 视为重复。
    /// 任一方缺角点时退化为纯文本去重（避免把同内容反复计入）。
    fn is_duplicate(found: &[QrFinding], text: &str, points: &[[f32; 2]]) -> bool {
        for f in found {
            if f.text != text {
                continue;
            }
            if points.is_empty() || f.points.is_empty() {
                return true;
            }
            let (cx, cy) = centroid(points);
            let (fx, fy) = centroid(&f.points);
            if ((cx - fx).powi(2) + (cy - fy).powi(2)).sqrt() < DUP_DIST {
                return true;
            }
        }
        false
    }

    fn centroid(points: &[[f32; 2]]) -> (f32, f32) {
        let n = points.len().max(1) as f32;
        let (sx, sy) = points
            .iter()
            .fold((0.0f32, 0.0f32), |(ax, ay), p| (ax + p[0], ay + p[1]));
        (sx / n, sy / n)
    }

    fn describe(
        factor: f64,
        invert: bool,
        binarize: bool,
        contrast: f64,
        brightness: f64,
        blur: i32,
    ) -> String {
        let mut parts = vec![format!("缩放×{factor}")];
        if invert {
            parts.push("反色".to_string());
        }
        parts.push("灰度".to_string());
        if binarize {
            parts.push("二值化".to_string());
        }
        if (contrast - 1.0).abs() > f64::EPSILON {
            parts.push(format!("对比度×{contrast}"));
        }
        if brightness != 0.0 {
            parts.push(format!("亮度{brightness:+}"));
        }
        if blur > 0 {
            parts.push(format!("模糊{blur}"));
        }
        parts.join(" · ")
    }
}
