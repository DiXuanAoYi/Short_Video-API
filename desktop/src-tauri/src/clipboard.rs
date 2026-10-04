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
            let links = providers::detect_links_with(&text, &settings);
            if links.is_empty() {
                continue;
            }
            on_links(&app, text, links, settings.auto_download);
        }
    });
}

pub(crate) fn on_links(app: &AppHandle, text: String, links: Vec<DetectedLink>, auto_download: bool) {
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
    let r = resolve_and_enqueue_full(app, text).await?;
    Ok(r.message())
}

/// 自动加入队列的结果。
pub struct AutoEnqueued {
    pub title: String,
    pub task_ids: Vec<i64>,
    pub already_downloaded: usize,
    pub already_queued: usize,
}

impl AutoEnqueued {
    pub fn message(&self) -> String {
        if self.task_ids.is_empty() && self.already_downloaded > 0 {
            format!("{}（之前已下载）", self.title)
        } else if self.task_ids.is_empty() && self.already_queued > 0 {
            format!("{}（已在队列中）", self.title)
        } else {
            self.title.clone()
        }
    }
}

/// 解析并按默认选项加入队列（剪贴板自动下载、手机发送）。
pub async fn resolve_and_enqueue_full(app: &AppHandle, text: &str) -> crate::model::AppResult<AutoEnqueued> {
    let st = app.state::<Arc<AppState>>().inner().clone();
    let settings = st.settings();
    let info = providers::resolve_text(&st.parse_ctx(&settings), text).await?;
    let _ = st.db.upsert_history(&info);
    let (ids, post) = crate::quality::auto_selection(&info, &settings);
    let title = info.title.clone();
    if info.kind == crate::model::MediaKind::Playlist {
        return Err(crate::model::AppError::invalid(format!("“{title}”是一个列表（{} 条），请在电脑上选择要下载的条目。", info.entries.len())));
    }
    let r = download::enqueue_with(app, info, &ids, post)?;
    Ok(AutoEnqueued { title, task_ids: r.tasks.iter().map(|t| t.id).collect(), already_downloaded: r.already_downloaded, already_queued: r.already_queued })
}

/// 全局快捷键 / 托盘菜单：打开主窗口并解析当前剪贴板。
pub fn parse_clipboard_now(app: &AppHandle) {
    let text = app.clipboard().read_text().unwrap_or_default();
    tray::show_main(app);
    let _ = app.emit(EVT_PARSE_REQUEST, text);
}
