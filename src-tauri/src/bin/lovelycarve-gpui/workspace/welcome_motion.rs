//! The welcome screen's entrance and its idle illustration loops.
use std::{f32::consts::TAU, time::Duration};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    bounce, div, ease_in_out, ease_out_quint, img, px, radians, svg, Animation, AnimationExt, App,
    ElementId, IntoElement, ParentElement as _, Styled, Transformation,
};

use crate::assets;

pub(super) fn rise<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    delay_ms: u64,
    distance: f32,
    enabled: bool,
) -> impl IntoElement {
    if !enabled {
        return element.into_any_element();
    }
    let duration_ms = 400;
    let total_ms = delay_ms + duration_ms;
    let ease = ease_out_quint();
    element
        .with_animation(
            id,
            Animation::new(Duration::from_millis(total_ms)).with_easing(move |phase| {
                let elapsed = phase * total_ms as f32;
                ease(((elapsed - delay_ms as f32) / duration_ms as f32).clamp(0., 1.))
            }),
            move |element, progress| {
                element
                    .relative()
                    .top(px(distance * (1. - progress)))
                    .opacity(progress)
            },
        )
        .into_any_element()
}

pub(super) fn artwork(animate: bool, dragging: bool, cx: &App) -> impl IntoElement {
    let (tray, sheet) = if cx.theme().is_dark() {
        (assets::EVIDENCE_TRAY_DARK, assets::EVIDENCE_SHEET_DARK)
    } else {
        (assets::EVIDENCE_TRAY_LIGHT, assets::EVIDENCE_SHEET_LIGHT)
    };
    let sheet = img(sheet).absolute().size_full().left_0().top_0();
    let sector = svg()
        .path(assets::EVIDENCE_SECTOR)
        .absolute()
        .left(px(72.))
        .top(px(32.))
        .size(px(44.))
        .text_color(cx.theme().accent_foreground);
    let sector = if animate {
        sector
            .with_animation(
                "welcome-disc-spin",
                Animation::new(Duration::from_secs(8))
                    .repeat()
                    .with_max_fps(30.),
                |sector, phase| {
                    sector.with_transformation(Transformation::rotate(radians(TAU * phase)))
                },
            )
            .into_any_element()
    } else {
        sector.into_any_element()
    };
    let sheet = if animate && !dragging {
        sheet
            .with_animation(
                "welcome-sheet-float",
                Animation::new(Duration::from_millis(2600))
                    .repeat()
                    .with_max_fps(30.)
                    .with_easing(bounce(ease_in_out)),
                |sheet, phase| sheet.top(px(-5. * phase)),
            )
            .into_any_element()
    } else {
        sheet.into_any_element()
    };
    div()
        .relative()
        .w(px(240.))
        .h(px(128.))
        .flex_shrink_0()
        .child(img(tray).size_full())
        .child(sector)
        .child(sheet)
}
