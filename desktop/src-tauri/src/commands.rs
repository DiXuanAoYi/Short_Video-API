//! 前端可调用的命令。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tauri_plugin_opener::OpenerExt;

use crate::db::{HistoryItem, LibraryItem};
use crate::download::{self, TaskSnapshot};
use crate::model::{AppError, AppResult, MediaInfo};
use crate::providers::{self, DetectedLink};
use crate::settings::Settings;
use crate::{clipboard, tray, AppState};

const REPO: &str = "DiXuanAoYi/Short_Video-API";

type St<'a> = State<'a, Arc<AppState>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    id: &'static str,
    name: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    providers: Vec<ProviderInfo>,
    repo: &'static str,
    os: &'static str,
}

#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        providers: providers::all().iter().map(|p| ProviderInfo { id: p.id(), name: p.name() }).collect(),
        repo: REPO,
        os: std::env::consts::OS,
    }
}

#[tauri::command]
pub fn get_settings(state: St<'_>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: St<'_>, settings: Settings) -> AppResult<Settings> {
    let mut s = settings;
    s.normalize();
    let old = state.settings();
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
    if old.shortcut != s.shortcut {
        apply_shortcut(&app, &s.shortcut);
    }
    tray::sync_watch_item(&app);
    if old.concurrency != s.concurrency {
        download::schedule(&app);
    }
    Ok(s)
}

pub fn apply_shortcut(app: &AppHandle, shortcut: &str) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        return;
    }
    if let Err(e) = gs.register(shortcut) {
        eprintln!("register shortcut {shortcut} failed: {e}");
    }
}

#[tauri::command]
pub fn detect_links(text: String) -> Vec<DetectedLink> {
    providers::detect_links(&text)
}

#[tauri::command]
pub async fn resolve_link(state: St<'_>, text: String) -> AppResult<MediaInfo> {
    let settings = state.settings();
    let info = providers::resolve_text(&state.client, &settings, &text).await?;
    let _ = state.db.upsert_history(&info);
    Ok(info)
}

#[tauri::command]
pub async fn resolve_and_enqueue(app: AppHandle, text: String) -> AppResult<String> {
    clipboard::resolve_and_enqueue(&app, &text).await
}

#[tauri::command]
pub fn enqueue(app: AppHandle, media: MediaInfo, asset_ids: Vec<String>) -> AppResult<Vec<TaskSnapshot>> {
    if asset_ids.is_empty() {
        return Err(AppError::msg("请至少选择一项要下载的内容。"));
    }
    download::enqueue(&app, media, &asset_ids)
}

#[tauri::command]
pub fn list_tasks(state: St<'_>) -> Vec<TaskSnapshot> {
    state.downloads.snapshots()
}

#[tauri::command]
pub fn pause_task(app: AppHandle, id: u64) {
    download::pause(&app, id)
}

#[tauri::command]
pub fn resume_task(app: AppHandle, id: u64) {
    download::resume(&app, id)
}

#[tauri::command]
pub fn cancel_task(app: AppHandle, id: u64) {
    download::cancel(&app, id)
}

#[tauri::command]
pub fn remove_task(app: AppHandle, id: u64) {
    download::remove(&app, id)
}

#[tauri::command]
pub fn clear_finished(app: AppHandle) {
    download::clear_finished(&app)
}

#[tauri::command]
pub fn pause_all(app: AppHandle) {
    download::pause_all(&app)
}

#[tauri::command]
pub fn resume_all(app: AppHandle) {
    download::resume_all(&app)
}

#[tauri::command]
pub fn list_history(state: St<'_>, query: Option<String>) -> AppResult<Vec<HistoryItem>> {
    state.db.list_history(query.as_deref().unwrap_or(""), 500)
}

#[tauri::command]
pub fn delete_history(state: St<'_>, id: i64) -> AppResult<()> {
    state.db.delete_history(id)
}

#[tauri::command]
pub fn clear_history(state: St<'_>) -> AppResult<()> {
    state.db.clear_history()
}

#[tauri::command]
pub fn list_library(state: St<'_>, query: Option<String>) -> AppResult<Vec<LibraryItem>> {
    state.db.list_library(query.as_deref().unwrap_or(""), 1000)
}

#[tauri::command]
pub fn delete_library(state: St<'_>, id: i64, delete_file: bool) -> AppResult<()> {
    if let Some(path) = state.db.delete_library(id)? {
        if delete_file {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(AppError::msg(format!("记录已删除，但文件删除失败：{e}"))),
            }
        }
    }
    Ok(())
}

/// 写入剪贴板，并让监听忽略这次变化。
#[tauri::command]
pub fn copy_text(app: AppHandle, state: St<'_>, text: String) -> AppResult<()> {
    *state.clipboard_ignore.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
    app.clipboard().write_text(text).map_err(|e| AppError::msg(format!("复制失败：{e}")))
}

#[tauri::command]
pub fn open_file(app: AppHandle, path: String) -> AppResult<()> {
    app.opener().open_path(path, None::<&str>).map_err(|e| AppError::msg(format!("无法打开文件：{e}")))
}

#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) -> AppResult<()> {
    let p = std::path::PathBuf::from(&path);
    if p.is_dir() {
        return app.opener().open_path(path, None::<&str>).map_err(|e| AppError::msg(format!("无法打开文件夹：{e}")));
    }
    app.opener().reveal_item_in_dir(p).map_err(|e| AppError::msg(format!("无法打开所在文件夹：{e}")))
}

#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> AppResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::msg("只能打开网页链接。"));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| AppError::msg(format!("无法打开链接：{e}")))
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    tray::show_main(&app)
}

#[tauri::command]
pub fn hide_mini(app: AppHandle) {
    if let Some(w) = app.get_webview_window("mini") {
        let _ = w.hide();
    }
}

fn login_url(platform: &str) -> AppResult<&'static str> {
    providers::by_id(platform).map(|p| p.login_url()).ok_or_else(|| AppError::msg("这个平台不支持登录。"))
}

/// 打开平台登录窗口。窗口内是平台官网，不能调用本程序的任何命令。
#[tauri::command]
pub fn open_login(app: AppHandle, platform: String) -> AppResult<()> {
    let label = format!("login-{platform}");
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let url = login_url(&platform)?;
    let name = providers::by_id(&platform).map(|p| p.name()).unwrap_or("平台");
    WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url.parse().expect("valid url")))
        .title(format!("登录{name}：登录完成后回到清影点击“保存登录状态”"))
        .inner_size(1100.0, 780.0)
        .build()?;
    Ok(())
}

/// 读取登录窗口的 Cookie 并保存到设置，然后关闭登录窗口。
#[tauri::command]
pub fn save_login_cookies(app: AppHandle, state: St<'_>, platform: String) -> AppResult<Settings> {
    let label = format!("login-{platform}");
    let window = app.get_webview_window(&label).ok_or_else(|| AppError::msg("登录窗口已关闭。请先点击“打开登录窗口”并完成登录。"))?;
    let url: url::Url = login_url(&platform)?.parse().expect("valid url");
    let cookies = window.cookies_for_url(url)?;
    if cookies.is_empty() {
        return Err(AppError::msg("没有读取到 Cookie，请确认已在登录窗口中完成登录。"));
    }
    let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
    let mut s = state.settings();
    s.cookies.insert(platform.clone(), header);
    s.cookie_updated_at.insert(platform, crate::db::now());
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
    let _ = window.close();
    Ok(s)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current: String,
    latest: Option<String>,
    has_update: bool,
    url: String,
}

/// 通过 GitHub Releases 检查新版本（不自动安装，只提示并打开下载页）。
#[tauri::command]
pub async fn check_update(app: AppHandle, state: St<'_>) -> AppResult<UpdateInfo> {
    let current = app.package_info().version.to_string();
    let resp = state
        .client
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "ClearClip")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    let releases_page = format!("https://github.com/{REPO}/releases");
    if resp.status().as_u16() == 404 {
        return Ok(UpdateInfo { current, latest: None, has_update: false, url: releases_page });
    }
    if !resp.status().is_success() {
        return Err(AppError::msg(format!("检查更新失败：GitHub 返回 {}", resp.status())));
    }
    let data: serde_json::Value = resp.json().await?;
    let tag = data.get("tag_name").and_then(|v| v.as_str()).unwrap_or("").trim_start_matches(['v', 'V']).to_string();
    let url = data.get("html_url").and_then(|v| v.as_str()).unwrap_or(&releases_page).to_string();
    let has_update = is_newer(&tag, &current);
    Ok(UpdateInfo { current, latest: Some(tag).filter(|t| !t.is_empty()), has_update, url })
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> { s.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parse(latest), parse(current));
    !latest.is_empty() && a > b
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_compare() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }
}
