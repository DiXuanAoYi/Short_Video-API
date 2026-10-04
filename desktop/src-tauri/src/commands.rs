//! 前端可调用的命令。涉及数据库、文件和网络的命令都是异步命令，不占用界面线程。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::cookies::{self, AccountSummary};
use crate::db::{HistoryItem, LibraryItem};
use crate::download::{self, EnqueueResult, OrphanPart, TaskSnapshot};
use crate::model::{AppError, AppResult, MediaInfo};
use crate::providers::{self, DetectedLink};
use crate::settings::Settings;
use crate::{clipboard, diagnostics, tray, AppState};

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
    /// Cookie 加密密钥是否存放在系统钥匙串
    key_in_keyring: bool,
    shortcut_error: Option<String>,
}

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: St<'_>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        providers: providers::all().iter().map(|p| ProviderInfo { id: p.id(), name: p.name() }).collect(),
        repo: REPO,
        os: std::env::consts::OS,
        key_in_keyring: state.cookies.key_in_keyring,
        shortcut_error: state.shortcut_error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    }
}

#[tauri::command]
pub fn get_settings(state: St<'_>) -> Settings {
    state.settings()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsResult {
    settings: Settings,
    /// 全局快捷键注册失败的原因（设置仍会保存）
    shortcut_error: Option<String>,
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, state: St<'_>, settings: Settings) -> AppResult<SaveSettingsResult> {
    let mut s = settings;
    s.normalize();
    let old = state.settings();
    // 旧版明文 Cookie 字段不再写回
    s.cookies.clear();
    s.cookie_updated_at.clear();
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
    if old.shortcut != s.shortcut {
        apply_shortcut(&app, &s.shortcut);
    }
    if old.network != s.network {
        state.net.invalidate();
    }
    tray::sync_watch_item(&app);
    if old.concurrency != s.concurrency || old.per_site_concurrency != s.per_site_concurrency {
        download::schedule(&app);
    }
    let shortcut_error = state.shortcut_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(SaveSettingsResult { settings: s, shortcut_error })
}

/// 注册全局快捷键，失败原因记录到状态里供界面显示。
pub fn apply_shortcut(app: &AppHandle, shortcut: &str) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let shortcut = shortcut.trim();
    let error = if shortcut.is_empty() {
        None
    } else {
        gs.register(shortcut).err().map(|e| {
            log::warn!("register shortcut {shortcut} failed: {e}");
            format!("快捷键 {shortcut} 注册失败，可能格式不对或已被其他软件占用：{e}")
        })
    };
    if let Some(st) = app.try_state::<Arc<AppState>>() {
        *st.shortcut_error.lock().unwrap_or_else(|e| e.into_inner()) = error;
    }
}

#[tauri::command]
pub fn detect_links(text: String) -> Vec<DetectedLink> {
    providers::detect_links(&text)
}

#[tauri::command]
pub async fn resolve_link(state: St<'_>, text: String) -> AppResult<MediaInfo> {
    let settings = state.settings();
    let info = providers::resolve_text(&state.parse_ctx(&settings), &text).await.inspect_err(|e| log::info!("resolve failed ({:?}): {e}", e.kind))?;
    let _ = state.db.upsert_history(&info);
    Ok(info)
}

#[tauri::command]
pub async fn resolve_and_enqueue(app: AppHandle, text: String) -> AppResult<String> {
    clipboard::resolve_and_enqueue(&app, &text).await
}

#[tauri::command]
pub async fn enqueue(app: AppHandle, media: MediaInfo, asset_ids: Vec<String>) -> AppResult<EnqueueResult> {
    if asset_ids.is_empty() {
        return Err(AppError::invalid("请至少选择一项要下载的内容。"));
    }
    download::enqueue(&app, media, &asset_ids)
}

#[tauri::command]
pub fn list_tasks(state: St<'_>) -> Vec<TaskSnapshot> {
    state.downloads.snapshots()
}

#[tauri::command]
pub fn pause_task(app: AppHandle, id: i64) {
    download::pause(&app, id)
}

#[tauri::command]
pub fn resume_task(app: AppHandle, id: i64) {
    download::resume(&app, id)
}

#[tauri::command]
pub fn cancel_task(app: AppHandle, id: i64) {
    download::cancel(&app, id)
}

#[tauri::command]
pub async fn remove_task(app: AppHandle, id: i64) -> AppResult<()> {
    download::remove(&app, id);
    Ok(())
}

#[tauri::command]
pub async fn clear_finished(app: AppHandle) -> AppResult<()> {
    download::clear_finished(&app);
    Ok(())
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
pub async fn list_orphan_parts(app: AppHandle) -> AppResult<Vec<OrphanPart>> {
    Ok(download::orphan_parts(&app))
}

#[tauri::command]
pub async fn delete_orphan_parts(app: AppHandle, paths: Vec<String>) -> AppResult<usize> {
    Ok(download::delete_orphans(&app, &paths))
}

#[tauri::command]
pub async fn list_history(state: St<'_>, query: Option<String>) -> AppResult<Vec<HistoryItem>> {
    state.db.list_history(query.as_deref().unwrap_or(""), 500)
}

#[tauri::command]
pub async fn delete_history(state: St<'_>, id: i64) -> AppResult<()> {
    state.db.delete_history(id)
}

#[tauri::command]
pub async fn clear_history(state: St<'_>) -> AppResult<()> {
    state.db.clear_history()
}

#[tauri::command]
pub async fn list_library(state: St<'_>, query: Option<String>) -> AppResult<Vec<LibraryItem>> {
    state.db.list_library(query.as_deref().unwrap_or(""), 1000)
}

#[tauri::command]
pub async fn delete_library(state: St<'_>, id: i64, delete_file: bool) -> AppResult<()> {
    if let Some(path) = state.db.delete_library(id)? {
        if delete_file {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(AppError::new(crate::error::ErrorKind::Disk, format!("记录已删除，但文件删除失败：{e}"))),
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
        return Err(AppError::invalid("只能打开网页链接。"));
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

// ---------- 账号与 Cookie ----------

/// 登录地址：内置平台用平台的登录页；其他网站用传入的地址（必须是 https）。
fn login_target(site: &str, url: Option<&str>) -> AppResult<(String, Url)> {
    if let Some(p) = providers::by_id(site) {
        return Ok((p.name().to_string(), p.login_url().parse().expect("valid url")));
    }
    let raw = url.map(String::from).unwrap_or_else(|| format!("https://www.{site}/"));
    let u = Url::parse(&raw).map_err(|_| AppError::invalid("登录地址格式不正确。"))?;
    if u.scheme() != "https" {
        return Err(AppError::invalid("登录地址必须是 https 网页。"));
    }
    Ok((site.to_string(), u))
}

fn login_label(site: &str) -> String {
    format!("login-{}", site.replace(|c: char| !c.is_ascii_alphanumeric(), "-"))
}

/// 打开登录窗口。窗口内是网站官网，没有任何调用本程序的权限。
#[tauri::command]
pub fn open_login(app: AppHandle, site: String, url: Option<String>) -> AppResult<()> {
    let label = login_label(&site);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let (name, target) = login_target(&site, url.as_deref())?;
    WebviewWindowBuilder::new(&app, label, WebviewUrl::External(target))
        .title(format!("登录{name}：登录完成后回到清影点击“保存登录状态”"))
        .inner_size(1100.0, 780.0)
        .build()?;
    Ok(())
}

/// 读取登录窗口里属于该网站的全部 Cookie，保存为一个账号，然后关闭登录窗口。
#[tauri::command]
pub async fn save_login_cookies(app: AppHandle, state: St<'_>, site: String, label: Option<String>) -> AppResult<Vec<AccountSummary>> {
    let window = app.get_webview_window(&login_label(&site)).ok_or_else(|| AppError::invalid("登录窗口已关闭。请先点击“打开登录窗口”并完成登录。"))?;
    let (_, target) = login_target(&site, None).or_else(|_| window.url().map(|u| (site.clone(), u)).map_err(AppError::from))?;
    let domain = target.host_str().map(cookies::registrable_domain).unwrap_or_default();
    let mut list: Vec<cookies::StoredCookie> =
        window.cookies().unwrap_or_default().iter().filter_map(cookies::from_webview).filter(|c| cookies::registrable_domain(&c.domain) == domain).collect();
    if list.is_empty() {
        list = window.cookies_for_url(target)?.iter().filter_map(cookies::from_webview).collect();
    }
    if list.is_empty() {
        return Err(AppError::need_login("没有读取到 Cookie，请确认已在登录窗口中完成登录。"));
    }
    state.cookies.upsert(&site, label.as_deref().unwrap_or("默认"), list)?;
    let _ = window.close();
    log::info!("saved login cookies for {site}");
    Ok(state.cookies.summaries())
}

#[tauri::command]
pub fn list_accounts(state: St<'_>) -> Vec<AccountSummary> {
    state.cookies.summaries()
}

/// 导入 Netscape 格式的 cookies.txt 文件。
#[tauri::command]
pub async fn import_cookies_file(state: St<'_>, path: String, label: Option<String>) -> AppResult<Vec<AccountSummary>> {
    let meta = std::fs::metadata(&path)?;
    if meta.len() > 5 * 1024 * 1024 {
        return Err(AppError::invalid("文件过大，不像是 cookies.txt。"));
    }
    let text = std::fs::read_to_string(&path)?;
    let (sites, n) = state.cookies.import_netscape(&text, label.as_deref().unwrap_or("导入"))?;
    log::info!("imported {n} cookies for {sites} sites");
    Ok(state.cookies.summaries())
}

/// 导入粘贴的内容：Netscape 格式，或 `a=1; b=2` 形式的 Cookie 头（需要指定网站）。
#[tauri::command]
pub async fn import_cookies_text(state: St<'_>, site: String, text: String, label: Option<String>) -> AppResult<Vec<AccountSummary>> {
    let label = label.unwrap_or_else(|| "手动填写".into());
    if text.contains('\t') {
        state.cookies.import_netscape(&text, &label)?;
    } else {
        let site = site.trim().to_ascii_lowercase();
        if site.is_empty() {
            return Err(AppError::invalid("请选择或填写这些 Cookie 属于哪个网站。"));
        }
        let (_, target) = login_target(&site, None)?;
        let domain = target.host_str().map(cookies::registrable_domain).unwrap_or(site.clone());
        state.cookies.upsert(&site, &label, cookies::parse_header(&text, &domain))?;
    }
    Ok(state.cookies.summaries())
}

#[tauri::command]
pub async fn rename_account(state: St<'_>, id: String, label: String) -> AppResult<Vec<AccountSummary>> {
    state.cookies.rename(&id, &label)?;
    Ok(state.cookies.summaries())
}

#[tauri::command]
pub async fn set_default_account(state: St<'_>, id: String) -> AppResult<Vec<AccountSummary>> {
    state.cookies.set_default(&id)?;
    Ok(state.cookies.summaries())
}

#[tauri::command]
pub async fn delete_account(state: St<'_>, id: String) -> AppResult<Vec<AccountSummary>> {
    state.cookies.delete(&id)?;
    Ok(state.cookies.summaries())
}

/// 检查账号是否仍处于登录状态（目前支持 B站、微博），并记录用户名。
#[tauri::command]
pub async fn check_account(state: St<'_>, id: String) -> AppResult<providers::AccountStatus> {
    let site = state.cookies.account_site(&id).ok_or_else(|| AppError::not_found("账号不存在。"))?;
    let provider = providers::by_id(&site).ok_or_else(|| AppError::unsupported("暂不支持检查这个网站的登录状态。"))?;
    let settings = state.settings();
    let mut ctx = state.parse_ctx(&settings);
    ctx.account = Some(id.clone());
    let status = provider.account_status(&ctx).await?.ok_or_else(|| AppError::unsupported(format!("暂不支持检查{}的登录状态。", provider.name())))?;
    let name = match (&status.user_name, &status.vip) {
        (Some(n), Some(v)) => Some(format!("{n}（{v}）")),
        (n, _) => n.clone(),
    };
    state.cookies.set_user_name(&id, if status.logged_in { name } else { None })?;
    Ok(status)
}

/// 测试某个地址在当前网络规则下能否访问。
#[tauri::command]
pub async fn test_route(state: St<'_>, url: String) -> AppResult<crate::net::RouteTest> {
    let url = if url.starts_with("http") { url } else { format!("https://{url}/") };
    state.net.test(&state.settings().network, &url).await
}

// ---------- 诊断 ----------

/// 生成诊断信息（已隐去 Cookie 和令牌），用户可以复制后反馈问题。
#[tauri::command]
pub async fn get_diagnostics(app: AppHandle, state: St<'_>) -> AppResult<String> {
    let s = state.settings();
    let tasks = state.downloads.snapshots();
    let count = |st: download::TaskStatus| tasks.iter().filter(|t| t.status == st).count();
    let accounts: Vec<String> = state.cookies.summaries().iter().map(|a| format!("{}（{} 个 Cookie）", a.site, a.cookie_count)).collect();
    let mut out = String::new();
    out.push_str(&format!("清影 ClearClip {}\n", app.package_info().version));
    out.push_str(&format!("系统：{} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    out.push_str(&format!("时间：{}\n\n", chrono::Utc::now().to_rfc3339()));
    out.push_str(&format!(
        "设置：解析模式 {:?}，远程 API {}，并发 {}，重试 {} 次，自动继续 {}，剪贴板监听 {}，录制样本 {}\n",
        s.parse_mode,
        if s.remote_endpoint.is_empty() { "未设置" } else { "已设置" },
        s.concurrency,
        s.max_retries,
        s.auto_resume,
        s.watch_clipboard,
        s.record_samples
    ));
    out.push_str(&format!("Cookie 密钥位置：{}\n", if state.cookies.key_in_keyring { "系统钥匙串" } else { "本地密钥文件" }));
    out.push_str(&format!("已保存账号：{}\n", if accounts.is_empty() { "无".to_string() } else { accounts.join("，") }));
    out.push_str(&format!(
        "任务：等待 {}，下载中 {}，暂停 {}，失败 {}，完成 {}\n",
        count(download::TaskStatus::Queued),
        count(download::TaskStatus::Running),
        count(download::TaskStatus::Paused),
        count(download::TaskStatus::Failed),
        count(download::TaskStatus::Done)
    ));
    for t in tasks.iter().filter(|t| t.status == download::TaskStatus::Failed).take(10) {
        out.push_str(&format!("  失败：[{}] {:?} {}\n", t.platform, t.error_kind, t.error.as_deref().unwrap_or("")));
    }
    out.push_str("\n最近日志：\n");
    out.push_str(&diagnostics::log_tail(&state.log_dir, 200));
    Ok(diagnostics::redact(&out))
}

#[tauri::command]
pub fn open_log_dir(app: AppHandle, state: St<'_>) -> AppResult<()> {
    let _ = std::fs::create_dir_all(&state.log_dir);
    app.opener().open_path(state.log_dir.to_string_lossy().into_owned(), None::<&str>).map_err(|e| AppError::msg(format!("无法打开日志目录：{e}")))
}

#[tauri::command]
pub fn open_samples_dir(app: AppHandle, state: St<'_>) -> AppResult<()> {
    let _ = std::fs::create_dir_all(&state.samples_dir);
    app.opener().open_path(state.samples_dir.to_string_lossy().into_owned(), None::<&str>).map_err(|e| AppError::msg(format!("无法打开样本目录：{e}")))
}

// ---------- 更新 ----------

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
        return Err(AppError::from_status(resp.status().as_u16(), "GitHub "));
    }
    let data: serde_json::Value = resp.json().await?;
    let tag = data.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let tag = tag.trim_start_matches("desktop-").trim_start_matches(['v', 'V']).to_string();
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
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn login_targets() {
        assert_eq!(login_target("bilibili", None).unwrap().1.as_str(), "https://www.bilibili.com/");
        assert_eq!(login_target("youtube.com", None).unwrap().1.as_str(), "https://www.youtube.com/");
        assert!(login_target("x.com", Some("http://x.com/")).is_err());
        assert_eq!(login_label("youtube.com"), "login-youtube-com");
    }
}
