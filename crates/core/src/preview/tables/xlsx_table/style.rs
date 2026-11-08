//! Spreadsheet styles mapped to bounded CSS metadata.
use super::*;

/// 边框样式 → CSS（宽度 + 线型），移植 `xlsx.ts::borderCss`。入参为 OOXML 边框样式串。
pub(super) fn border_css(style: &str) -> &'static str {
    match style {
        "thick" => "3px solid",
        "medium" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" => "2px solid",
        "double" => "3px double",
        "dotted" => "1px dotted",
        "dashed" | "dashDot" | "dashDotDot" | "slantDashDot" => "1px dashed",
        _ => "1px solid", // thin / hair / 其它 → 1px solid
    }
}

/// 水平对齐 OOXML 值 → CSS text-align（移植 `xlsx.ts::ALIGN_H`）。
pub(super) fn align_h_css(v: &str) -> Option<&'static str> {
    match v {
        "left" => Some("left"),
        "center" => Some("center"),
        "right" => Some("right"),
        "justify" => Some("justify"),
        "centerContinuous" => Some("center"),
        "distributed" => Some("justify"),
        "fill" => Some("left"),
        _ => None, // general / 未知 → 不设（继承默认）
    }
}

/// 垂直对齐 OOXML 值 → CSS vertical-align（移植 `xlsx.ts::ALIGN_V`；OOXML "center" → "middle"）。
pub(super) fn align_v_css(v: &str) -> Option<&'static str> {
    match v {
        "top" => Some("top"),
        "center" => Some("middle"),
        "bottom" => Some("bottom"),
        "distributed" => Some("middle"),
        "justify" => Some("middle"),
        _ => None,
    }
}

/// ARGB（8 位 AARRGGBB，alpha 在前）/ RGB（6 位）→ `#RRGGBB`（丢弃 alpha）。
/// 非法 / 空输入返回 None（交由调用方跳过该样式，绝不注入未校验字符串到 :style）。
/// 移植 `xlsx.ts::argbToHex`。
pub(super) fn argb_to_hex(argb: &str) -> Option<String> {
    let s = argb.trim();
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("#{}", h[2..].to_uppercase()));
    }
    if h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("#{}", h.to_uppercase()));
    }
    None
}

/// 把 umya Color 解析为 `#RRGGBB`；解析不出返回 None。
///
/// 取色策略（兼顾忠实与主题安全）：
/// 1) 显式直接 ARGB（`get_argb_str()` 非空）→ 直接用。
/// 2) 显式主题色（theme_index ≥ 2，即 dk2/lt2/accent*/hlink 等）→ 用 umya 内置
///    `get_argb_with_theme` 解析（含 tint）。**故意跳过 0/1（dk1/lt1 = 默认正文/背景色）**：
///    未着色单元格常默认引用 text1，若也上色会把所有单元格刷成固定黑/白，破坏暗色主题
///    可读性——这些应继承主题感知的 `--text-primary`。
/// 3) 旧式 indexed 调色板未解析（罕见）→ 降级返回 None。
#[allow(deprecated)]
pub(super) fn resolve_color(
    color: &umya_spreadsheet::Color,
    theme: &umya_spreadsheet::drawing::Theme,
) -> Option<String> {
    let raw = color.get_argb_str();
    if !raw.is_empty() {
        return argb_to_hex(&raw);
    }
    if color.get_theme_index() >= 2 {
        return argb_to_hex(&color.get_argb_with_theme(theme));
    }
    None
}

/// 把 umya Style 拼成内联 CSS（逐项移植 `xlsx.ts::cellStyle`）。
#[allow(deprecated)]
pub(super) fn cell_style(
    st: &umya_spreadsheet::Style,
    theme: &umya_spreadsheet::drawing::Theme,
) -> String {
    let mut css: Vec<String> = Vec::new();

    // 底色：仅取 solid pattern 填充的前景色（对齐 vue-office/JS，忽略渐变/无填充）。
    if let Some(fill) = st.get_fill() {
        if let Some(pf) = fill.get_pattern_fill() {
            if matches!(
                pf.get_pattern_type(),
                umya_spreadsheet::PatternValues::Solid
            ) {
                if let Some(fg) = pf.get_foreground_color() {
                    if let Some(bg) = resolve_color(fg, theme) {
                        css.push(format!("background-color:{bg}"));
                    }
                }
            }
        }
    }

    if let Some(font) = st.get_font() {
        if let Some(fc) = resolve_color(font.get_color(), theme) {
            css.push(format!("color:{fc}"));
        }
        if font.get_bold() {
            css.push("font-weight:700".to_string());
        }
        if font.get_italic() {
            css.push("font-style:italic".to_string());
        }
        let underline = font.get_underline();
        if !underline.is_empty() && underline != "none" {
            css.push("text-decoration:underline".to_string());
        }
        // Excel 字号单位是磅（pt），浏览器原生支持——直接用，最忠实。
        let size = font.get_size();
        if size > 0.0 {
            css.push(format!("font-size:{}pt", trim_float(size)));
        }
    }

    if let Some(al) = st.get_alignment() {
        if let Some(h) = align_h_css(al.get_horizontal().value_string()) {
            css.push(format!("text-align:{h}"));
        }
        if let Some(v) = align_v_css(al.get_vertical().value_string()) {
            css.push(format!("vertical-align:{v}"));
        }
        if al.get_wrap_text() {
            css.push("white-space:normal".to_string());
        }
    }

    if let Some(bd) = st.get_borders() {
        for (side, border) in [
            ("top", bd.get_top()),
            ("right", bd.get_right()),
            ("bottom", bd.get_bottom()),
            ("left", bd.get_left()),
        ] {
            let style = border.get_border_style();
            if !style.is_empty() && style != "none" {
                let color = border
                    .get_color()
                    .as_ref()
                    .and_then(|c| resolve_color(c, theme))
                    .unwrap_or_else(|| "#000000".to_string());
                css.push(format!("border-{side}:{} {color}", border_css(style)));
            }
        }
    }

    css.join(";")
}
