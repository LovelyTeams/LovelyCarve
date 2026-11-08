//! The welcome screen's entrance and its idle illustration loops.
use std::{f32::consts::TAU, time::Duration};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    div, img, px, radians, svg, Animation, AnimationExt, AnyElement, App, ElementId, IntoElement,
    ParentElement as _, Styled, Transformation,
};

use crate::{assets, motion};

/// Rise into place `delay` after the page opens, or appear at once when the
/// welcome motion is off.
pub(super) fn rise<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    delay: Duration,
    distance: f32,
    enabled: bool,
    cx: &App,
) -> AnyElement {
    if enabled {
        motion::rise(element, id, delay, distance, cx).into_any_element()
    } else {
        element.into_any_element()
    }
}

/// The evidence tray with its spinning disc and floating sheet. Both loops
/// follow the display's frames on one shared clock: a frame-rate cap would
/// fall back to Windows' 15.6 ms timer ticks, which judder.
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
                Animation::new(Duration::from_secs(8)).repeat_synced(),
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
                // A full sine period: the sheet lifts and settles without a
                // corner at either end.
                Animation::new(Duration::from_millis(3200))
                    .repeat_synced()
                    .with_easing(|phase| (1. - (TAU * phase).cos()) / 2.),
                |sheet, lift| sheet.top(px(-5. * lift)),
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
