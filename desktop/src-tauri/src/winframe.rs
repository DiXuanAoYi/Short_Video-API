//! Windows：去掉无边框小窗（迷你窗、悬浮拖拽窗）上残留的原生标题栏。
//!
//! 窗口库在 Windows 上做“无边框”的办法，是保留窗口的标题栏样式、再在 `WM_NCCALCSIZE` 里把非客户区缩成 0；
//! 每次显示、隐藏窗口时样式又会被重新设置。一旦非客户区不为 0（例如带阴影的无边框窗口），系统会在窗口上方
//! 画出完整的原生标题栏，而且点不动。这里在窗口显示之后直接把标题栏、系统菜单、边框相关的样式去掉。

/// 去掉窗口的标题栏、系统菜单、最大化 / 最小化按钮和可拖动边框样式。其他平台什么也不做。
#[cfg(windows)]
pub fn strip_caption(w: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION,
        WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };
    let Ok(hwnd) = w.hwnd() else { return };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    // SAFETY: `hwnd` 是本进程里刚刚取得的有效窗口句柄；这些调用只修改窗口样式并通知系统重算边框。
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let wanted = style & !(WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME);
        if wanted != style {
            SetWindowLongW(hwnd, GWL_STYLE, wanted as i32);
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }
}

#[cfg(not(windows))]
pub fn strip_caption(_: &tauri::WebviewWindow) {}
