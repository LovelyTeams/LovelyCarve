//! Places the native preview on the area painted by the active GPUI tab.
use std::{
    cell::{Cell, RefCell},
    num::NonZeroIsize,
};

use gpui_kit::{
    canvas,
    component::{menu::PopupMenu, WindowExt as _},
    px, App, Bounds, Focusable as _, Global, IntoElement, Pixels, Styled as _, WeakFocusHandle,
    Window,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winapi::{
    shared::windef::HWND,
    um::winuser::{
        GetWindowLongPtrW, PostMessageW, SetWindowLongPtrW, GWL_STYLE, WM_APP, WS_CLIPCHILDREN,
    },
};

pub(super) const LAYOUT_MESSAGE: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ViewerWindow(NonZeroIsize);

impl ViewerWindow {
    pub fn from_gpui(window: &Window) -> Result<Self, String> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| format!("无法获取 GPUI 窗口句柄：{error}"))?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => {
                let parent = Self(handle.hwnd);
                // SAFETY: This is the live HWND owned by the current GPUI
                // thread. Preserve its existing styles and clip child paints.
                unsafe {
                    let style = GetWindowLongPtrW(parent.hwnd(), GWL_STYLE);
                    if style & WS_CLIPCHILDREN as isize == 0
                        && SetWindowLongPtrW(
                            parent.hwnd(),
                            GWL_STYLE,
                            style | WS_CLIPCHILDREN as isize,
                        ) == 0
                    {
                        return Err(format!(
                            "无法配置 GPUI 预览区域：{}",
                            std::io::Error::last_os_error()
                        ));
                    }
                }
                Ok(parent)
            }
            _ => Err("原类型视图需要 Win32 窗口".to_owned()),
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        value.parse().ok().map(Self)
    }

    pub(super) fn number(self) -> isize {
        self.0.get()
    }

    pub(super) fn hwnd(self) -> HWND {
        self.number() as HWND
    }

    fn place(self, rect: Placement) {
        // SAFETY: The private message only transports integers. Posting it
        // avoids blocking the GPUI thread if the viewer is parsing a file.
        let posted = unsafe {
            PostMessageW(
                self.hwnd(),
                LAYOUT_MESSAGE,
                rect.origin as usize,
                rect.size as isize,
            )
        };
        if posted == 0 {
            tracing::debug!("原类型视图已关闭，忽略布局更新");
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Placement {
    origin: u64,
    size: u64,
}

impl Placement {
    fn from_bounds(bounds: Bounds<Pixels>, scale: f32) -> Self {
        let physical = |pixels: Pixels| (f32::from(pixels) * scale).round() as i32;
        let x = physical(bounds.origin.x);
        let y = physical(bounds.origin.y);
        let width = physical(bounds.right()) - x;
        let height = physical(bounds.bottom()) - y;
        let pack =
            |first: i32, second: i32| u64::from(first as u32) | (u64::from(second as u32) << 32);
        Self {
            origin: pack(x, y),
            size: pack(width, height),
        }
    }
}

#[derive(Default)]
pub struct ViewerLayer {
    pending: Cell<Option<(ViewerWindow, Placement)>>,
    visible: Cell<Option<(ViewerWindow, Placement)>>,
    file_menu: RefCell<Option<WeakFocusHandle>>,
}

impl Global for ViewerLayer {}

impl ViewerLayer {
    pub fn track_file_menu(menu: &PopupMenu, cx: &App) {
        cx.global::<Self>()
            .file_menu
            .replace(Some(menu.focus_handle(cx).downgrade()));
    }
}

pub fn viewport(viewer: ViewerWindow) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                let placement = Placement::from_bounds(bounds, window.scale_factor());
                cx.global::<ViewerLayer>()
                    .pending
                    .set(Some((viewer, placement)));
            }
        },
        |_, (), _, _| {},
    )
    .size_full()
}

/// Commit after prepaint, including deferred inspectors on narrow windows.
/// An inactive pane leaves no placement for this frame.
pub fn finish_frame(task_center_open: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, cx: &mut App| {
            let pending = cx.global::<ViewerLayer>().pending.take();
            let requested = pending.filter(|_| {
                let menu_focused = cx
                    .global::<ViewerLayer>()
                    .file_menu
                    .borrow()
                    .as_ref()
                    .and_then(WeakFocusHandle::upgrade)
                    .is_some_and(|focus| focus.contains_focused(window, cx));
                !task_center_open
                    && !menu_focused
                    && !window.has_active_dialog(cx)
                    && !window.has_active_sheet(cx)
                    && !cx.has_active_drag()
            });
            let layer = cx.global::<ViewerLayer>();
            let previous = layer.visible.replace(requested);
            if requested == previous {
                return;
            }
            if let Some((viewer, _)) = previous {
                if requested.is_none_or(|(current, _)| current != viewer) {
                    viewer.place(Placement::default());
                }
            }
            if let Some((viewer, placement)) = requested {
                viewer.place(placement);
            }
        },
    )
    .absolute()
    .size(px(0.))
}
