//! How views enter: pages, panels and list rows fade in while they move a
//! few pixels into place. Durations and curves come from GPUI Kit's motion
//! tokens, so views move like the components around them. Every animation
//! runs through GPUI's animation element or Base motion, which hold still
//! when the system asks for reduced motion (on Windows: Accessibility →
//! Visual effects → Animation effects).
use std::time::Duration;

use gpui_kit::{
    component::ActiveTheme as _, px, Animation, AnimationElement, AnimationExt as _, App,
    ElementId, IntoElement, Styled,
};

/// The gap between consecutive rows of a staggered entrance.
const STAGGER_STEP: Duration = Duration::from_millis(35);
/// Rows past this one enter together, so a long list does not trail.
const STAGGER_LIMIT: u32 = 8;

/// The delay of the `index`-th row of a staggered entrance.
pub fn stagger(index: usize) -> Duration {
    STAGGER_STEP * u32::try_from(index).unwrap_or(u32::MAX).min(STAGGER_LIMIT)
}

/// A one-shot entrance: at rest for `delay`, then the slow token duration on
/// the entering curve. The animated value runs from 0 to 1.
pub fn entrance(delay: Duration, cx: &App) -> Animation {
    let tokens = cx.theme().motion_tokens();
    let curve = tokens.easing_enter.clone();
    let (wait, duration) = (delay.as_secs_f32(), tokens.duration_slow.as_secs_f32());
    Animation::new(delay + tokens.duration_slow)
        .with_easing(move |phase| curve.sample(entrance_progress(phase, wait, duration)))
}

/// How far an entrance has run, from 0 to 1, at `phase` of a timeline that
/// waits `delay` seconds and then runs for `duration` seconds. A zero
/// duration jumps to the end once the wait is over.
fn entrance_progress(phase: f32, delay: f32, duration: f32) -> f32 {
    let active = phase * (delay + duration) - delay;
    if duration > 0. {
        (active / duration).clamp(0., 1.)
    } else if active >= 0. {
        1.
    } else {
        0.
    }
}

/// Fade `element` in while it rises `distance` pixels into place, starting
/// `delay` after it first renders. It plays again whenever `id` changes or
/// the element leaves the frame. The rise is a relative offset, so the
/// element must sit in the normal flow.
pub fn rise<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    delay: Duration,
    distance: f32,
    cx: &App,
) -> AnimationElement<E> {
    element.with_animation(id, entrance(delay, cx), move |element, progress| {
        element
            .relative()
            .top(px(distance * (1. - progress)))
            .opacity(progress)
    })
}

/// Like [`rise`], but the element slides in sideways: from the left for a
/// positive `distance`, from the right for a negative one.
pub fn slide<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    distance: f32,
    cx: &App,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        entrance(Duration::ZERO, cx),
        move |element, progress| {
            element
                .relative()
                .left(px(-distance * (1. - progress)))
                .opacity(progress)
        },
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{entrance_progress, stagger};

    #[test]
    fn stagger_spaces_rows_and_stops_growing() {
        assert_eq!(stagger(0), Duration::ZERO);
        assert_eq!(stagger(2), Duration::from_millis(70));
        assert_eq!(stagger(8), stagger(500));
    }

    #[test]
    fn entrance_waits_out_its_delay_then_runs() {
        // 0.1 s of delay, then 0.2 s of motion: a third of the timeline waits.
        let progress = |phase| entrance_progress(phase, 0.1, 0.2);
        assert_eq!(progress(0.), 0.);
        assert!(progress(1. / 3.).abs() < 1e-6);
        assert!((progress(2. / 3.) - 0.5).abs() < 1e-6);
        assert_eq!(progress(1.), 1.);
    }

    #[test]
    fn zero_length_entrance_ends_without_dividing_by_zero() {
        assert_eq!(entrance_progress(1., 0., 0.), 1.);
        assert_eq!(entrance_progress(0.5, 0.1, 0.), 0.);
        assert_eq!(entrance_progress(1., 0.1, 0.), 1.);
    }
}
