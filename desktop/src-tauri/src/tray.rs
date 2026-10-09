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
/// 与 tauri.conf.json 中 float 窗口的尺寸一致
const FLOAT_W: f64 = 156.0;
const FLOAT_H: f64 = 64.0;

pub struct TrayWatchItem(pub CheckMenuItem<Wry>);

/// 托盘菜单里需要随语言改文字的菜单项
pub struct TrayLabels {
    show: MenuItem<Wry>,
    parse: MenuItem<Wry>,
    lock: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// 按界面语言刷新托盘菜单文字。
pub fn apply_language(app: &AppHandle, en: bool) {
    use crate::i18n::tr;
    if let Some(l) = app.try_state::<TrayLabels>() {
        let _ = l.show.set_text(tr(en, "显示主窗口"));
        let _ = l.parse.set_text(tr(en, "解析剪贴板"));
        let _ = l.lock.set_text(tr(en, "锁定"));
        let _ = l.quit.set_text(tr(en, "退出清影"));
    }
    if let Some(w) = app.try_state::<TrayWatchItem>() {
        let _ = w.0.set_text(tr(en, "监听剪贴板"));
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let watching = app.state::<Arc<AppState>>().settings().watch_clipboard;
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let parse = MenuItem::with_id(app, "parse", "解析剪贴板", true, None::<&str>)?;
    let watch = CheckMenuItem::with_id(app, "watch", "监听剪贴板", true, watching, None::<&str>)?;
    let lock = MenuItem::with_id(app, "lock", "锁定", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出清影", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &parse, &watch, &lock, &PredefinedMenuItem::separator(app)?, &quit])?;
    app.manage(TrayWatchItem(watch));
    app.manage(TrayLabels { show: show.clone(), parse: parse.clone(), lock: lock.clone(), quit: quit.clone() });

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
    apply_language(app, crate::i18n::is_en(&app.state::<Arc<AppState>>().settings_raw().language));
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

/// 托盘提示里的速度，例如 `3.2 MB/s`。
pub fn speed_text(bps: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bps as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB/s", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB/s", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB/s", b / KB)
    } else {
        format!("{bps} B/s")
    }
}

/// 托盘提示（鼠标悬停时显示）：总下载速度和任务数。只显示数字，不显示任何标题。
pub fn tooltip_for(en: bool, speed: u64, running: usize, queued: usize) -> String {
    let name = if en { "ClearClip" } else { "清影" };
    match (running, queued, en) {
        (0, 0, _) => "清影 ClearClip".to_string(),
        (r, 0, false) => format!("{name} · ↓ {} · {r} 个任务", speed_text(speed)),
        (r, q, false) => format!("{name} · ↓ {} · {r} 个下载中，{q} 个等待", speed_text(speed)),
        (r, 0, true) => format!("{name} · ↓ {} · {r} task{}", speed_text(speed), if r == 1 { "" } else { "s" }),
        (r, q, true) => format!("{name} · ↓ {} · {r} downloading, {q} waiting", speed_text(speed)),
    }
}

/// 每 2 秒刷新托盘提示（macOS 同时在图标旁显示速度）。
pub fn spawn_speed_ticker(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let st = app.state::<Arc<AppState>>();
            let snaps = st.downloads.snapshots();
            let running: Vec<_> = snaps.iter().filter(|s| s.status == crate::download::TaskStatus::Running).collect();
            let queued = snaps.iter().filter(|s| s.status == crate::download::TaskStatus::Queued).count();
            let speed: u64 = running.iter().map(|s| s.speed).sum();
            let tip = tooltip_for(crate::i18n::is_en(&st.settings_raw().language), speed, running.len(), queued);
            if tip == last {
                continue;
            }
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_tooltip(Some(&tip));
                #[cfg(target_os = "macos")]
                let _ = tray.set_title(if running.is_empty() { None } else { Some(speed_text(speed)) });
            }
            last = tip;
        }
    });
}

pub fn main_is_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main").map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false)).unwrap_or(false)
}

/// 显示或隐藏悬浮拖拽窗；首次显示时放在屏幕右侧、迷你窗上方。
pub fn apply_float(app: &AppHandle, on: bool) {
    let Some(w) = app.get_webview_window("float") else { return };
    if !on {
        let _ = w.hide();
        return;
    }
    if app.try_state::<Arc<AppState>>().is_some_and(|st| crate::security::status(&st).locked) {
        return;
    }
    if !w.is_visible().unwrap_or(false) {
        if let Ok(Some(monitor)) = w.current_monitor().or_else(|_| w.primary_monitor()) {
            let scale = monitor.scale_factor();
            let size = LogicalSize::new(FLOAT_W, FLOAT_H).to_physical::<u32>(scale);
            let area = monitor.work_area();
            let x = area.position.x + area.size.width as i32 - size.width as i32 - (24.0 * scale) as i32;
            let y = area.position.y + area.size.height as i32 - size.height as i32 - (MINI_H * scale) as i32 - (40.0 * scale) as i32;
            let _ = w.set_position(PhysicalPosition::new(x, y));
        }
        let _ = w.set_size(LogicalSize::new(FLOAT_W, FLOAT_H));
    }
    let _ = w.show();
    // 有的系统在窗口第一次显示时会忽略显示前设置的尺寸，显示后再设置一次
    let _ = w.set_size(LogicalSize::new(FLOAT_W, FLOAT_H));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_units() {
        assert_eq!(speed_text(0), "0 B/s");
        assert_eq!(speed_text(900), "900 B/s");
        assert_eq!(speed_text(2048), "2 KB/s");
        assert_eq!(speed_text(3_355_443), "3.2 MB/s");
        assert_eq!(speed_text(5 * 1024 * 1024 * 1024), "5.0 GB/s");
    }

    #[test]
    fn tooltip_shows_only_numbers() {
        assert_eq!(tooltip_for(false, 0, 0, 0), "清影 ClearClip");
        assert_eq!(tooltip_for(false, 1_048_576, 2, 0), "清影 · ↓ 1.0 MB/s · 2 个任务");
        assert_eq!(tooltip_for(false, 1_048_576, 1, 3), "清影 · ↓ 1.0 MB/s · 1 个下载中，3 个等待");
        assert_eq!(tooltip_for(true, 1_048_576, 1, 0), "ClearClip · ↓ 1.0 MB/s · 1 task");
        assert_eq!(tooltip_for(true, 1_048_576, 3, 2), "ClearClip · ↓ 1.0 MB/s · 3 downloading, 2 waiting");
    }
}
