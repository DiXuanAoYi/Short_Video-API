//! 剪贴板监听：复制到受支持的链接时通知前端，主窗口不在前台时弹出迷你窗。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::providers::{self, DetectedLink};
use crate::{download, tray, AppState};

pub const EVT_CLIPBOARD: &str = "clipboard://link";
pub const EVT_PARSE_REQUEST: &str = "app://parse-request";
pub const EVT_AUTO_RESULT: &str = "clipboard://auto-result";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardLink {
    pub text: String,
    pub links: Vec<DetectedLink>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResult {
    pub url: String,
    pub ok: bool,
    pub message: String,
}

pub fn start_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        // 启动时剪贴板里已有的内容不触发
        let mut last = app.clipboard().read_text().unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(800));
            let Ok(text) = app.clipboard().read_text() else { continue };
            if text == last {
                continue;
            }
            last = text.clone();
            let st = app.state::<Arc<AppState>>().inner().clone();
            {
                let mut ignore = st.clipboard_ignore.lock().unwrap_or_else(|e| e.into_inner());
                if ignore.as_deref() == Some(text.as_str()) {
                    *ignore = None;
                    continue;
                }
            }
            let settings = st.settings();
            if !settings.watch_clipboard || st.clipboard_paused.load(Ordering::SeqCst) || !settings.disclaimer_accepted {
                continue;
            }
            let links = providers::detect_links(&text);
            if links.is_empty() {
                continue;
            }
            on_links(&app, text, links, settings.auto_download);
        }
    });
}

fn on_links(app: &AppHandle, text: String, links: Vec<DetectedLink>, auto_download: bool) {
    let _ = app.emit(EVT_CLIPBOARD, ClipboardLink { text, links: links.clone() });
    if auto_download {
        for link in links {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let result = resolve_and_enqueue(&app, &link.url).await;
                let payload = match result {
                    Ok(title) => AutoResult { url: link.url, ok: true, message: format!("已加入下载：{title}") },
                    Err(e) => AutoResult { url: link.url, ok: false, message: e.to_string() },
                };
                let _ = app.emit(EVT_AUTO_RESULT, payload);
            });
        }
    }
    if !tray::main_is_focused(app) {
        tray::show_mini(app);
    }
}

/// 解析并把默认资源加入下载队列，返回作品标题。
pub async fn resolve_and_enqueue(app: &AppHandle, text: &str) -> crate::model::AppResult<String> {
    let st = app.state::<Arc<AppState>>().inner().clone();
    let settings = st.settings();
    let info = providers::resolve_text(&st.parse_ctx(&settings), text).await?;
    let _ = st.db.upsert_history(&info);
    let (ids, post) = crate::quality::auto_selection(&info, &settings);
    let title = info.title.clone();
    if info.kind == crate::model::MediaKind::Playlist {
        return Err(crate::model::AppError::invalid(format!("“{title}”是一个列表（{} 条），请在主窗口选择要下载的条目。", info.entries.len())));
    }
    let r = download::enqueue_with(app, info, &ids, post)?;
    if r.tasks.is_empty() && r.already_downloaded > 0 {
        return Ok(format!("{title}（之前已下载）"));
    }
    if r.tasks.is_empty() && r.already_queued > 0 {
        return Ok(format!("{title}（已在队列中）"));
    }
    Ok(title)
}

/// 全局快捷键 / 托盘菜单：打开主窗口并解析当前剪贴板。
pub fn parse_clipboard_now(app: &AppHandle) {
    let text = app.clipboard().read_text().unwrap_or_default();
    tray::show_main(app);
    let _ = app.emit(EVT_PARSE_REQUEST, text);
}
