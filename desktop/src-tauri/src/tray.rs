//! 系统托盘、主窗口 / 迷你窗的显示控制。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, Wry};

use crate::{clipboard, AppState};

/// 与 tauri.conf.json 中 mini 窗口的尺寸一致
const MINI_W: f64 = 320.0;
const MINI_H: f64 = 196.0;

pub struct TrayWatchItem(pub CheckMenuItem<Wry>);

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let watching = app.state::<Arc<AppState>>().settings().watch_clipboard;
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let parse = MenuItem::with_id(app, "parse", "解析剪贴板", true, None::<&str>)?;
    let watch = CheckMenuItem::with_id(app, "watch", "监听剪贴板", true, watching, None::<&str>)?;
    let lock = MenuItem::with_id(app, "lock", "锁定", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出清影", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &parse, &watch, &lock, &PredefinedMenuItem::separator(app)?, &quit])?;
    app.manage(TrayWatchItem(watch));

    let mut builder = TrayIconBuilder::with_id("main").tooltip("清影 ClearClip").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "parse" => clipboard::parse_clipboard_now(app),
            "watch" => {
                let st = app.state::<Arc<AppState>>();
                let paused = st.clipboard_paused.load(Ordering::SeqCst);
                st.clipboard_paused.store(!paused, Ordering::SeqCst);
                sync_watch_item(app);
            }
            "lock" => {
                // 没有设置应用锁时，只是把窗口收起来
                if !crate::security::lock_now(app) {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 托盘勾选状态 = 设置里开启 且 没有临时暂停。
pub fn sync_watch_item(app: &AppHandle) {
    let st = app.state::<Arc<AppState>>();
    let on = st.settings().watch_clipboard && !st.clipboard_paused.load(Ordering::SeqCst);
    if let Some(item) = app.try_state::<TrayWatchItem>() {
        let _ = item.0.set_checked(on);
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    if let Some(m) = app.get_webview_window("mini") {
        let _ = m.hide();
    }
}

pub fn main_is_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main").map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false)).unwrap_or(false)
}

/// 在屏幕右下角显示迷你窗（不抢焦点）。
pub fn show_mini(app: &AppHandle) {
    // 锁定时迷你窗会露出任务标题，不显示
    if app.try_state::<Arc<AppState>>().is_some_and(|st| crate::security::status(&st).locked) {
        return;
    }
    let Some(w) = app.get_webview_window("mini") else { return };
    if let Ok(Some(monitor)) = w.current_monitor().or_else(|_| w.primary_monitor()) {
        let scale = monitor.scale_factor();
        // 首次显示前窗口尺寸可能还是 1×1，此时按配置的逻辑尺寸计算
        let size: tauri::PhysicalSize<u32> =
            w.outer_size().ok().filter(|s| s.width > 100).unwrap_or_else(|| LogicalSize::new(MINI_W, MINI_H).to_physical(scale));
        let area = monitor.work_area();
        let margin = (16.0 * scale) as i32;
        let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
        let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.set_size(LogicalSize::new(MINI_W, MINI_H));
    let _ = w.show();
}
