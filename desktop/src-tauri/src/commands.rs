//! 前端可调用的命令。涉及数据库、文件和网络的命令都是异步命令，不占用界面线程。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::cookies::{self, AccountSummary};
use crate::db::{HistoryItem, LibraryFilter, LibraryItem};
use crate::download::{self, EnqueueResult, OrphanPart, PostOptions, TaskSnapshot};
use crate::model::{AppError, AppResult, MediaInfo, MediaKind, PlaylistEntry, SeriesInfo};
use crate::providers::{self, DetectedLink};
use crate::settings::Settings;
use crate::{clipboard, diagnostics, phone, quality, subs, tools, tray, AppState};

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
    // 手机发送的令牌和已配对设备只通过专门的命令修改，避免被设置页的旧数据覆盖
    s.phone = old.phone.clone();
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
    if old.shortcut != s.shortcut {
        apply_shortcut(&app, &s.shortcut);
    }
    if old.network != s.network {
        state.net.invalidate();
    }
    tray::sync_watch_item(&app);
    if !s.prevent_sleep {
        crate::power::keep_awake(false);
    }
    if old.launch_at_login != s.launch_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let r = if s.launch_at_login { app.autolaunch().enable() } else { app.autolaunch().disable() };
        if let Err(e) = r {
            log::warn!("autostart change failed: {e}");
        }
    }
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
pub async fn enqueue(app: AppHandle, media: MediaInfo, asset_ids: Vec<String>, post: Option<PostOptions>) -> AppResult<EnqueueResult> {
    if asset_ids.is_empty() {
        return Err(AppError::invalid("请至少选择一项要下载的内容。"));
    }
    download::enqueue_with(&app, media, &asset_ids, post.unwrap_or_default())
}

pub const EVT_BATCH: &str = "batch://progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    pub title: String,
    pub done: usize,
    pub total: usize,
    pub queued: usize,
    /// 之前已下载或已在队列中而跳过的数量
    pub skipped: usize,
    pub failed: Vec<String>,
    pub finished: bool,
}

/// 把播放列表 / 合集中选中的条目逐个解析并加入队列（后台进行，进度通过事件通知）。
#[tauri::command]
pub async fn enqueue_entries(app: AppHandle, state: St<'_>, playlist: MediaInfo, entry_ids: Vec<String>) -> AppResult<usize> {
    let entries: Vec<PlaylistEntry> = playlist.entries.iter().filter(|e| entry_ids.contains(&e.id)).cloned().collect();
    if entries.is_empty() {
        return Err(AppError::invalid("请至少选择一个条目。"));
    }
    let n = entries.len();
    let st = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let settings = st.settings();
        let mut p = BatchProgress { title: playlist.title.clone(), done: 0, total: n, queued: 0, skipped: 0, failed: vec![], finished: false };
        let _ = app.emit(EVT_BATCH, p.clone());
        for e in entries {
            let ctx = st.parse_ctx(&settings);
            match providers::resolve_url(&ctx, &e.url).await {
                Ok(mut info) if info.kind != MediaKind::Playlist => {
                    // 列表里的条目按“列表名 / 第 N 集”归档
                    if info.series.is_none() {
                        info.series = Some(SeriesInfo { name: playlist.title.clone(), season: None, episode: Some(e.index) });
                    }
                    let (ids, post) = quality::auto_selection(&info, &settings);
                    match download::enqueue_with(&app, info, &ids, post) {
                        Ok(r) => {
                            p.queued += r.tasks.len();
                            p.skipped += r.already_downloaded + r.already_queued;
                        }
                        Err(err) => p.failed.push(format!("{}：{err}", e.title)),
                    }
                }
                Ok(_) => p.failed.push(format!("{}：嵌套的列表请单独打开", e.title)),
                Err(err) => p.failed.push(format!("{}：{err}", e.title)),
            }
            p.done += 1;
            let _ = app.emit(EVT_BATCH, p.clone());
        }
        p.finished = true;
        let _ = app.emit(EVT_BATCH, p);
    });
    Ok(n)
}

// ---------- 手机发送 ----------

#[tauri::command]
pub fn phone_info(app: AppHandle) -> phone::PhoneInfo {
    phone::info(&app)
}

#[tauri::command]
pub async fn phone_enable(app: AppHandle, on: bool) -> AppResult<phone::PhoneInfo> {
    phone::set_enabled(&app, on).await
}

#[tauri::command]
pub fn phone_reset_token(app: AppHandle) -> AppResult<phone::PhoneInfo> {
    phone::reset_token(&app)
}

#[tauri::command]
pub fn phone_revoke(app: AppHandle, device_id: String) -> AppResult<phone::PhoneInfo> {
    phone::revoke(&app, &device_id)
}

#[tauri::command]
pub fn phone_pair_respond(app: AppHandle, device_id: String, accept: bool) -> AppResult<phone::PhoneInfo> {
    phone::respond_pair(&app, &device_id, accept)
}

// ---------- 订阅 ----------

#[tauri::command]
pub fn subs_list(app: AppHandle) -> AppResult<Vec<subs::Subscription>> {
    subs::list(&app)
}

#[tauri::command]
pub async fn subs_preview(app: AppHandle, url: String) -> AppResult<providers::listing::ListResult> {
    let url = providers::extract_urls(&url).into_iter().next().ok_or_else(|| AppError::invalid("没有找到链接。"))?;
    subs::preview(&app, &url).await
}

#[tauri::command]
pub async fn subs_add(app: AppHandle, url: String, title: Option<String>, settings: subs::SubSettings) -> AppResult<subs::Subscription> {
    let url = providers::extract_urls(&url).into_iter().next().ok_or_else(|| AppError::invalid("没有找到链接。"))?;
    subs::add(&app, &url, title, settings).await
}

#[tauri::command]
pub fn subs_update(app: AppHandle, state: St<'_>, id: i64, title: String, settings: subs::SubSettings) -> AppResult<()> {
    let mut settings = settings;
    settings.normalize();
    state.db.sub_update(id, title.trim(), &settings)?;
    let _ = app.emit(subs::EVT_SUBS, ());
    Ok(())
}

#[tauri::command]
pub fn subs_set_paused(app: AppHandle, state: St<'_>, id: i64, paused: bool) -> AppResult<()> {
    state.db.sub_set_status(id, if paused { "paused" } else { "active" })?;
    let _ = app.emit(subs::EVT_SUBS, ());
    Ok(())
}

#[tauri::command]
pub fn subs_delete(app: AppHandle, state: St<'_>, id: i64) -> AppResult<()> {
    state.db.sub_delete(id)?;
    let _ = app.emit(subs::EVT_SUBS, ());
    Ok(())
}

#[tauri::command]
pub async fn subs_check(app: AppHandle, id: i64) -> AppResult<usize> {
    subs::check(&app, id).await
}

#[tauri::command]
pub fn subs_items(state: St<'_>, id: i64, statuses: Vec<String>) -> AppResult<Vec<subs::SubItem>> {
    let refs: Vec<&str> = statuses.iter().map(String::as_str).collect();
    state.db.sub_items(id, &refs)
}

#[tauri::command]
pub async fn subs_download_items(app: AppHandle, id: i64, item_ids: Vec<String>) -> AppResult<usize> {
    subs::download_items(&app, id, &item_ids).await
}

#[tauri::command]
pub fn subs_ignore_items(app: AppHandle, id: i64, item_ids: Vec<String>) -> AppResult<()> {
    subs::ignore_items(&app, id, &item_ids)
}

#[tauri::command]
pub fn subs_clear_new(state: St<'_>, id: i64) -> AppResult<()> {
    state.db.sub_clear_new(id)
}

// ---------- 组件（yt-dlp / ffmpeg） ----------

fn tool_of(id: &str) -> AppResult<tools::Tool> {
    tools::Tool::parse(id).ok_or_else(|| AppError::invalid("未知组件"))
}

#[tauri::command]
pub async fn tools_status(state: St<'_>) -> AppResult<Vec<tools::ToolStatus>> {
    let mut out = vec![];
    for t in tools::Tool::all() {
        out.push(tools::status(&state, t).await);
    }
    Ok(out)
}

#[tauri::command]
pub async fn install_tool(app: AppHandle, tool: String) -> AppResult<tools::ToolStatus> {
    tools::install(&app, tool_of(&tool)?).await
}

#[tauri::command]
pub async fn rollback_tool(state: St<'_>, tool: String) -> AppResult<tools::ToolStatus> {
    tools::rollback(&state, tool_of(&tool)?).await
}

#[tauri::command]
pub async fn import_tool(state: St<'_>, tool: String, path: String) -> AppResult<tools::ToolStatus> {
    tools::import(&state, tool_of(&tool)?, std::path::Path::new(&path)).await
}

#[tauri::command]
pub async fn list_extractors(state: St<'_>) -> AppResult<Vec<String>> {
    tools::list_extractors(&state).await
}

#[tauri::command]
pub async fn open_tools_dir(app: AppHandle, state: St<'_>) -> AppResult<()> {
    std::fs::create_dir_all(&state.tools_dir)?;
    app.opener().open_path(state.tools_dir.to_string_lossy(), None::<&str>).map_err(|e| AppError::msg(e.to_string()))
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
pub async fn list_library(state: St<'_>, filter: Option<LibraryFilter>) -> AppResult<Vec<LibraryItem>> {
    state.db.search_library(&filter.unwrap_or_default(), 5000)
}

#[tauri::command]
pub async fn library_platforms(state: St<'_>) -> AppResult<Vec<crate::db::PlatformCount>> {
    state.db.library_platforms()
}

/// 重新下载媒体库里的一项（文件被移动或删除时）：重新解析原链接，下载同一个资源。
#[tauri::command]
pub async fn redownload(app: AppHandle, state: St<'_>, id: i64) -> AppResult<EnqueueResult> {
    let item = state.db.library_item(id)?.ok_or_else(|| AppError::not_found("记录不存在"))?;
    if item.source_url.is_empty() {
        return Err(AppError::invalid("这条记录没有保存原链接，无法重新下载。请重新粘贴链接解析。"));
    }
    if item.exists {
        return Err(AppError::invalid("文件还在原位置，不需要重新下载。"));
    }
    let settings = state.settings();
    let info = providers::resolve_url(&state.parse_ctx(&settings), &item.source_url).await?;
    let (ids, post) = match info.asset(&item.asset_id) {
        Some(a) => (vec![a.id.clone()], PostOptions::default()),
        None => quality::auto_selection(&info, &settings),
    };
    download::enqueue_with(&app, info, &ids, post)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResult {
    platform: String,
    name: String,
    sample: Option<String>,
    ok: bool,
    millis: u64,
    message: String,
    kind: Option<crate::error::ErrorKind>,
}

/// 内置示例链接（长期稳定的公开内容）；其他平台用最近一次成功解析的链接。
const HEALTH_SAMPLES: &[(&str, &str)] =
    &[("bilibili", "https://www.bilibili.com/video/BV1GJ411x7h7"), ("yt-dlp", "https://www.youtube.com/watch?v=jNQXAC9IVRw")];

/// 平台健康检查：用示例链接测试各平台解析是否正常。
#[tauri::command]
pub async fn health_check(state: St<'_>) -> AppResult<Vec<HealthResult>> {
    let settings = state.settings();
    let mut targets: Vec<(String, String)> = providers::all().iter().map(|p| (p.id().to_string(), p.name().to_string())).collect();
    targets.push(("yt-dlp".into(), "yt-dlp（其他网站）".into()));
    let mut out = vec![];
    for (id, name) in targets {
        let sample = state.db.latest_source_url(&id)?.or_else(|| HEALTH_SAMPLES.iter().find(|(p, _)| *p == id).map(|(_, u)| u.to_string()));
        let Some(url) = sample.clone() else {
            out.push(HealthResult {
                platform: id,
                name,
                sample: None,
                ok: false,
                millis: 0,
                message: "没有示例链接：成功解析过一次该平台的链接后即可检测".into(),
                kind: None,
            });
            continue;
        };
        let ctx = state.parse_ctx(&settings);
        if id == "yt-dlp" && ctx.ytdlp.is_none() {
            out.push(HealthResult {
                platform: id,
                name,
                sample,
                ok: false,
                millis: 0,
                message: "未安装 yt-dlp".into(),
                kind: Some(crate::error::ErrorKind::NeedUpdate),
            });
            continue;
        }
        let t = std::time::Instant::now();
        let r = if id == "yt-dlp" {
            tokio::time::timeout(std::time::Duration::from_secs(60), providers::ytdlp::resolve(&ctx, &url)).await
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(30), providers::resolve_url(&ctx, &url)).await
        };
        let millis = t.elapsed().as_millis() as u64;
        out.push(match r {
            Ok(Ok(info)) => HealthResult {
                platform: id,
                name,
                sample,
                ok: true,
                millis,
                message: format!("正常：{}（{} 个资源）", info.title, info.assets.len().max(info.entries.len())),
                kind: None,
            },
            Ok(Err(e)) => HealthResult { platform: id, name, sample, ok: false, millis, message: e.message.clone(), kind: Some(e.kind) },
            Err(_) => HealthResult { platform: id, name, sample, ok: false, millis, message: "超时".into(), kind: Some(crate::error::ErrorKind::Network) },
        });
    }
    Ok(out)
}

// ---------- 队列排序、定时、完成后动作 ----------

#[tauri::command]
pub fn move_task(app: AppHandle, id: i64, to: String) {
    download::move_task(&app, id, &to)
}

#[tauri::command]
pub fn schedule_task(app: AppHandle, id: i64, start_at: Option<i64>) {
    download::schedule_task(&app, id, start_at)
}

#[tauri::command]
pub fn get_after_all_done(state: St<'_>) -> String {
    state.after_all_done.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[tauri::command]
pub fn set_after_all_done(state: St<'_>, action: String) -> AppResult<()> {
    if !matches!(action.as_str(), "none" | "sleep" | "shutdown") {
        return Err(AppError::invalid("未知操作"));
    }
    *state.after_all_done.lock().unwrap_or_else(|e| e.into_inner()) = action;
    Ok(())
}

/// 读取拖入的文本文件（只读前 1 MB），用于批量导入链接。
#[tauri::command]
pub async fn read_links_file(path: String) -> AppResult<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(&path)?;
    let mut buf = vec![];
    f.by_ref().take(1024 * 1024).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if providers::extract_urls(&text).is_empty() {
        return Err(AppError::invalid("文件里没有找到链接。"));
    }
    Ok(text)
}

#[tauri::command]
pub fn is_portable() -> bool {
    crate::portable_dir().is_some()
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
