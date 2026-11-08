//! The light and dark theme tokens: surfaces, text, lines, accent and the
//! severity hues every view reads its colors from.
use gpui_kit::{component::Theme, px, rgb, rgba, Hsla};

pub const PANEL_RADIUS: f32 = 12.;
pub const DIALOG_RADIUS: f32 = 16.;

struct Palette {
    canvas: u32,
    frame: u32,
    strip: u32,
    surface: u32,
    recess: u32,
    raised: u32,
    text: u32,
    muted_text: u32,
    border: u32,
    accent: u32,
    soft: u32,
    selected: u32,
    hover: u32,
    danger: u32,
    warning: u32,
    success: u32,
    info: u32,
    charts: [u32; 5],
    backdrop: u32,
}

const LIGHT: Palette = Palette {
    canvas: 0xfcfcfb,
    frame: 0xf5f4f0,
    strip: 0xf9f9f7,
    surface: 0xffffff,
    recess: 0xf3f3f0,
    raised: 0xffffff,
    text: 0x1f1e1c,
    muted_text: 0x65635e,
    border: 0xe6e5df,
    accent: 0xa6482a,
    soft: 0xc964421a,
    selected: 0xc9644217,
    hover: 0x1f1e1c0d,
    danger: 0xb91c1c,
    warning: 0x835800,
    success: 0x16722f,
    info: 0x1f5faa,
    charts: [0x3f7fd0, 0x4f9a6a, 0xd4a032, 0x8a6bc8, 0xd97757],
    backdrop: 0x1f1e1c52,
};

const DARK: Palette = Palette {
    canvas: 0x171716,
    frame: 0x0f0f0f,
    strip: 0x171716,
    surface: 0x1f1f1e,
    recess: 0x1b1b1a,
    raised: 0x393936,
    text: 0xf1f0ec,
    muted_text: 0xa7a59f,
    border: 0x2d2d2b,
    accent: 0xe28c6c,
    soft: 0xd9775724,
    selected: 0xd9775724,
    hover: 0xffffff0d,
    danger: 0xfca5a5,
    warning: 0xf0c25a,
    success: 0x8bd6a0,
    info: 0x93c1f5,
    charts: [0x6ea6ea, 0x74bf8c, 0xe3b65a, 0xa990e0, 0xe8906d],
    backdrop: 0x00000099,
};

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

pub fn evidence_artwork(svg: &str, dark: bool) -> Vec<u8> {
    let palette = if dark { &DARK } else { &LIGHT };
    svg.replace("INK", &format!("#{:06x}", palette.text))
        .replace("LINE", &format!("#{:06x}", palette.muted_text))
        .replace("SURFACE", &format!("#{:06x}", palette.surface))
        .replace("RECESS", &format!("#{:06x}", palette.recess))
        .replace("ACCENT", &format!("#{:06x}", palette.accent))
        .into_bytes()
}

pub fn apply(theme: &mut Theme) {
    let palette = if theme.is_dark() { &DARK } else { &LIGHT };
    let accent = color(palette.accent);
    let surface = color(palette.surface);
    let strip = color(palette.strip);
    let recess = color(palette.recess);
    let foreground = color(palette.text);
    let muted = color(palette.muted_text);
    let border = color(palette.border);
    let frame = color(palette.frame);
    let selected: Hsla = rgba(palette.selected).into();
    let hover: Hsla = rgba(palette.hover).into();
    let transparent = theme.transparent;

    theme.radius = px(8.);
    theme.radius_lg = px(DIALOG_RADIUS);

    theme.background = color(palette.canvas);
    theme.foreground = foreground;
    theme.muted = recess;
    theme.muted_foreground = muted;
    theme.border = border;
    theme.input = border;
    theme.popover = surface;
    theme.popover_foreground = foreground;
    theme.overlay = rgba(palette.backdrop).into();
    theme.group_box = surface;
    theme.group_box_foreground = foreground;
    theme.description_list_label = strip;
    theme.description_list_label_foreground = muted;

    theme.accent = rgba(palette.soft).into();
    theme.accent_foreground = accent;
    theme.primary = accent;
    theme.primary_hover = accent;
    theme.primary_active = accent;
    theme.primary_foreground = if theme.is_dark() { frame } else { surface };
    theme.link = accent;
    theme.link_hover = accent;
    theme.link_active = accent;
    theme.ring = accent;
    theme.caret = accent;
    theme.selection = accent.opacity(0.25);
    theme.progress_bar = accent;
    theme.drop_target = selected;
    theme.drag_border = accent;
    theme.secondary = recess;
    theme.secondary_foreground = foreground;
    theme.secondary_hover = hover;
    theme.secondary_active = selected;
    theme.button = foreground.opacity(0.06);
    theme.button_foreground = foreground;
    theme.button_hover = foreground.opacity(0.10);
    theme.button_active = foreground.opacity(0.14);
    theme.button_secondary = theme.button;
    theme.button_secondary_foreground = foreground;
    theme.button_secondary_hover = theme.button_hover;
    theme.button_secondary_active = theme.button_active;
    theme.button_primary = color(0xb5532f);
    theme.button_primary_foreground = color(0xffffff);
    theme.button_primary_hover = color(0xa0472a);
    theme.button_primary_active = theme.button_primary_hover;
    theme.button_danger = color(0xdc2626);
    theme.button_danger_foreground = color(0xffffff);
    theme.button_danger_hover = color(0xb91c1c);
    theme.button_danger_active = theme.button_danger_hover;
    theme.danger = color(palette.danger);
    theme.warning = color(palette.warning);
    theme.success = color(palette.success);
    theme.info = color(palette.info);
    // GPUI Kit also uses these colors as fills for solid status tags.
    let status_text = if theme.is_dark() { frame } else { surface };
    theme.danger_foreground = status_text;
    theme.warning_foreground = status_text;
    theme.success_foreground = status_text;
    theme.info_foreground = status_text;
    [
        theme.colors.chart_1,
        theme.colors.chart_2,
        theme.colors.chart_3,
        theme.colors.chart_4,
        theme.colors.chart_5,
    ] = palette.charts.map(color);

    theme.title_bar = frame;
    theme.title_bar_border = transparent;
    theme.sidebar = frame;
    theme.sidebar_border = border;
    theme.sidebar_foreground = muted;
    theme.sidebar_primary = accent;
    theme.sidebar_primary_foreground = selected;
    theme.sidebar_accent = selected;
    theme.sidebar_accent_foreground = accent;
    theme.tab = transparent;
    theme.tab_bar = transparent;
    theme.tab_bar_segmented = recess;
    theme.tab_foreground = muted;
    theme.tab_active = color(palette.raised);
    theme.tab_active_foreground = foreground;

    theme.colors.list = surface;
    theme.list_head = strip;
    theme.list_even = strip;
    theme.list_hover = hover;
    theme.list_active = selected;
    theme.list_active_border = accent;
    theme.table = surface;
    theme.table_head = strip;
    theme.table_head_foreground = muted;
    theme.table_foot = strip;
    theme.table_foot_foreground = muted;
    theme.table_even = strip;
    theme.table_hover = hover;
    theme.table_active = selected;
    theme.table_active_border = accent;
    theme.table_row_border = border;
    theme.scrollbar = transparent;
    theme.scrollbar_thumb = muted.opacity(0.35);
    theme.scrollbar_thumb_hover = muted.opacity(0.6);
}
