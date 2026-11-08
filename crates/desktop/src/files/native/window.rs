//! Win32 host for the Oracle Outside In Viewer 8.5.7 x64 control.
use std::{
    ffi::{OsStr, OsString},
    io::{self, Write as _},
    os::windows::ffi::OsStrExt as _,
    path::Path,
    ptr::{null, null_mut},
};

use winapi::{
    shared::{
        minwindef::{LPARAM, LRESULT, UINT, WPARAM},
        windef::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, HWND, RECT},
    },
    um::{
        libloaderapi::{
            AddDllDirectory, GetModuleHandleW, LoadLibraryExW, SetDefaultDllDirectories,
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        },
        winuser::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            GetDlgItem, GetMessageW, IsWindow, LoadCursorW, MoveWindow, PostQuitMessage,
            RegisterClassW, SendMessageW, SetFocus, SetProcessDpiAwarenessContext, SetWindowPos,
            ShowWindow, TranslateMessage, IDC_ARROW, MSG, SWP_NOACTIVATE, SWP_NOZORDER,
            SWP_SHOWWINDOW, SW_HIDE, WM_DESTROY, WM_SETFOCUS, WM_SIZE, WNDCLASSW, WS_CHILD,
            WS_CLIPCHILDREN, WS_VISIBLE,
        },
    },
};

use super::{surface::LAYOUT_MESSAGE, ViewerWindow};

const VIEWER_ID: i32 = 1;
// The 8.5.7 Windows message and spec IDs verified against the x64 runtime.
const SCCVW_VIEWFILE: UINT = 0x081c;
const IOTYPE_UNICODEPATH: u32 = 4;

// SCCVWVIEWFILE80 as accepted by the x64 control. The two reserved slots are
// pointer-sized in this ABI. The path and display name are UTF-16.
#[repr(C)]
struct ViewFile {
    size: u32,
    spec_type: u32,
    spec: *const u16,
    view_as: u32,
    use_display_name: u32,
    display_name: [u16; 260],
    delete_on_close: u32,
    flags: u32,
    reserved: [usize; 2],
}

const _: () = assert!(size_of::<ViewFile>() == 0x238);

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

fn win32_error(context: &str) -> String {
    format!("{context}：{}", io::Error::last_os_error())
}

pub(super) fn run(
    parent: ViewerWindow,
    directory: &Path,
    file: &Path,
    name: &OsStr,
) -> Result<(), String> {
    super::validate_runtime(directory)?;
    let directory_wide = wide(directory);
    let library_path = wide(directory.join("sccvw.dll"));
    let class_name = wide("LovelyCarveOutsideIn");
    let mut title = OsString::from("原类型视图 · ");
    title.push(name);
    let title = wide(title);
    let viewer_class = wide("SCCVIEWER");

    // SAFETY: This is the dedicated viewer process and GUI thread. All
    // strings passed to Win32 are NUL-terminated and outlive each call.
    // Keep the loaded library and search directory until the process exits
    // because the control loads its display engines on demand.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            tracing::debug!("{}", win32_error("设置原类型视图 DPI 模式失败"));
        }
        if SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) == 0
            || AddDllDirectory(directory_wide.as_ptr()).is_null()
        {
            return Err(win32_error("注册 Outside In DLL 目录失败"));
        }
        let library = LoadLibraryExW(
            library_path.as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        );
        if library.is_null() {
            return Err(win32_error("加载 Outside In x64 Viewer 失败"));
        }
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            return Err(win32_error("注册原类型视图窗口失败"));
        }
        let host = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_CLIPCHILDREN,
            0,
            0,
            640,
            480,
            parent.hwnd(),
            null_mut(),
            instance,
            null_mut(),
        );
        if host.is_null() {
            return Err(win32_error("创建原类型视图窗口失败"));
        }
        let viewer = CreateWindowExW(
            0,
            viewer_class.as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            0,
            0,
            host,
            VIEWER_ID as _,
            library,
            null_mut(),
        );
        let result = if viewer.is_null() {
            Err(win32_error("创建 Outside In 控件失败"))
        } else {
            resize_viewer(host, viewer);
            view_file(viewer, file, name).and_then(|()| {
                let mut output = io::stdout().lock();
                writeln!(output, "{}{}", super::READY, host as isize)
                    .and_then(|()| output.flush())
                    .map_err(|error| format!("报告原类型视图状态失败：{error}"))?;
                drop(output);
                message_loop()
            })
        };
        if IsWindow(host) != 0 && DestroyWindow(host) == 0 {
            tracing::debug!("{}", win32_error("销毁原类型视图窗口失败"));
        }
        result
    }
}

fn view_file(viewer: HWND, path: &Path, name: &OsStr) -> Result<(), String> {
    let path = wide(path);
    let mut display_name = [0; 260];
    for (unit, value) in display_name[..259].iter_mut().zip(name.encode_wide()) {
        *unit = value;
    }
    if (0xd800..=0xdbff).contains(&display_name[258]) {
        display_name[258] = 0;
    }
    let file = ViewFile {
        size: size_of::<ViewFile>() as u32,
        spec_type: IOTYPE_UNICODEPATH,
        spec: path.as_ptr(),
        view_as: 0,
        use_display_name: 1,
        display_name,
        delete_on_close: 0,
        flags: 0,
        reserved: [0; 2],
    };
    // SAFETY: The structure and path remain valid throughout this synchronous
    // call on the viewer's owning thread.
    let result = unsafe { SendMessageW(viewer, SCCVW_VIEWFILE, 0, &raw const file as LPARAM) };
    if result != 0 {
        return Err(format!("Outside In 无法预览此文件（错误码 {result}）"));
    }
    Ok(())
}

fn message_loop() -> Result<(), String> {
    // SAFETY: GetMessageW initializes the MSG before dispatch on this thread.
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err(win32_error("处理原类型视图消息失败")),
                0 => return Ok(()),
                _ => {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

fn resize_viewer(host: HWND, viewer: HWND) {
    // SAFETY: Both windows are owned by this thread. GetClientRect writes the
    // bounds to the provided RECT on success.
    unsafe {
        let mut bounds = std::mem::zeroed::<RECT>();
        if GetClientRect(host, &mut bounds) != 0 {
            MoveWindow(viewer, 0, 0, bounds.right, bounds.bottom, 1);
        }
    }
}

unsafe extern "system" fn window_proc(
    host: HWND,
    message: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Win32 calls this with a live host on its owning thread. The
    // parent sends placement as packed integers, with zero size to hide.
    unsafe {
        match message {
            WM_SIZE => {
                let viewer = GetDlgItem(host, VIEWER_ID);
                if !viewer.is_null() {
                    resize_viewer(host, viewer);
                }
            }
            WM_SETFOCUS => {
                let viewer = GetDlgItem(host, VIEWER_ID);
                if !viewer.is_null() {
                    SetFocus(viewer);
                }
            }
            LAYOUT_MESSAGE => {
                if lparam == 0 {
                    ShowWindow(host, SW_HIDE);
                } else {
                    SetWindowPos(
                        host,
                        null_mut(),
                        wparam as i32,
                        (wparam >> 32) as i32,
                        lparam as i32,
                        (lparam >> 32) as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
            }
            WM_DESTROY => PostQuitMessage(0),
            _ => return DefWindowProcW(host, message, wparam, lparam),
        }
    }
    0
}
