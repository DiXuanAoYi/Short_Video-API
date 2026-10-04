//! 下载队列：并发控制、暂停 / 继续（断点续传）、失败重试、直链过期自动重新解析。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::db::{self, NewDownload};
use crate::model::{AppResult, Asset, AssetKind, MediaInfo};
use crate::{naming, providers, AppState};

const CTRL_RUN: u8 = 0;
const CTRL_PAUSE: u8 = 1;
const CTRL_CANCEL: u8 = 2;
const MAX_NET_RETRIES: u32 = 3;

pub const EVT_TASKS: &str = "tasks://updated";
pub const EVT_PROGRESS: &str = "tasks://progress";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: u64,
    pub platform: String,
    pub platform_name: String,
    pub media_id: String,
    pub title: String,
    pub author: String,
    pub cover: Option<String>,
    pub asset_id: String,
    pub asset_label: String,
    pub asset_kind: AssetKind,
    pub file_path: String,
    pub status: TaskStatus,
    pub received: u64,
    pub total: Option<u64>,
    pub speed: u64,
    pub error: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

struct Entry {
    snap: TaskSnapshot,
    asset: Asset,
    media: Arc<MediaInfo>,
    control: Arc<AtomicU8>,
}

#[derive(Default)]
pub struct DownloadManager {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    tasks: Vec<Entry>,
    next_id: u64,
}

impl DownloadManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        self.lock().tasks.iter().map(|e| e.snap.clone()).collect()
    }

    pub fn has_active(&self) -> bool {
        self.lock().tasks.iter().any(|e| matches!(e.snap.status, TaskStatus::Running | TaskStatus::Queued))
    }
}

fn state(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

fn emit_all(app: &AppHandle) {
    let _ = app.emit(EVT_TASKS, state(app).downloads.snapshots());
}

/// 把作品中选中的资源加入队列。
pub fn enqueue(app: &AppHandle, media: MediaInfo, asset_ids: &[String]) -> AppResult<Vec<TaskSnapshot>> {
    let st = state(app);
    let settings = st.settings.read().unwrap_or_else(|e| e.into_inner()).clone();
    let media = Arc::new(media);
    let dir = naming::target_dir(&settings.download_root(), &media, settings.subfolder_by_platform);
    let base = naming::render_base(&settings.filename_template, &media);

    let mut created = Vec::new();
    {
        let mut inner = st.downloads.lock();
        for asset in media.assets.iter().filter(|a| asset_ids.contains(&a.id)) {
            inner.next_id += 1;
            let id = inner.next_id;
            let existing = if settings.skip_existing { st.db.existing_download(&media.platform, &media.id, &asset.id)? } else { None };
            let (status, path, note) = match existing {
                Some(p) => (TaskStatus::Done, PathBuf::from(p), Some("之前已下载，已跳过".to_string())),
                None => {
                    let wanted = dir.join(naming::file_name(&base, asset));
                    let taken_by_queue: Vec<String> =
                        inner.tasks.iter().filter(|e| e.snap.status != TaskStatus::Canceled).map(|e| e.snap.file_path.clone()).collect();
                    let path = naming::unique_path(wanted, &|p: &Path| p.exists() || part_path(p).exists() || taken_by_queue.iter().any(|t| Path::new(t) == p));
                    (TaskStatus::Queued, path, None)
                }
            };
            let snap = TaskSnapshot {
                id,
                platform: media.platform.clone(),
                platform_name: media.platform_name.clone(),
                media_id: media.id.clone(),
                title: media.title.clone(),
                author: media.author.clone(),
                cover: media.cover.clone(),
                asset_id: asset.id.clone(),
                asset_label: asset.label.clone(),
                asset_kind: asset.kind,
                file_path: path.to_string_lossy().into_owned(),
                status,
                received: 0,
                total: None,
                speed: 0,
                error: None,
                note,
                created_at: db::now(),
                finished_at: if status == TaskStatus::Done { Some(db::now()) } else { None },
            };
            created.push(snap.clone());
            inner.tasks.push(Entry { snap, asset: asset.clone(), media: media.clone(), control: Arc::new(AtomicU8::new(CTRL_RUN)) });
        }
    }
    schedule(app);
    Ok(created)
}

/// 按并发数启动等待中的任务。
pub fn schedule(app: &AppHandle) {
    let st = state(app);
    let concurrency = st.settings.read().map(|s| s.concurrency).unwrap_or(3);
    let mut to_start = Vec::new();
    {
        let mut inner = st.downloads.lock();
        let mut running = inner.tasks.iter().filter(|e| e.snap.status == TaskStatus::Running).count();
        for e in inner.tasks.iter_mut() {
            if running >= concurrency {
                break;
            }
            if e.snap.status == TaskStatus::Queued {
                e.snap.status = TaskStatus::Running;
                e.snap.error = None;
                e.control.store(CTRL_RUN, Ordering::SeqCst);
                running += 1;
                to_start.push(e.snap.id);
            }
        }
    }
    for id in to_start {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_task(app, id).await });
    }
    emit_all(app);
}

async fn run_task(app: AppHandle, id: u64) {
    let st = state(&app);
    let Some((mut asset, media, control, final_path)) = ({
        let inner = st.downloads.lock();
        inner.tasks.iter().find(|e| e.snap.id == id).map(|e| (e.asset.clone(), e.media.clone(), e.control.clone(), PathBuf::from(&e.snap.file_path)))
    }) else {
        return;
    };

    let part = part_path(&final_path);
    let referer = providers::referer_for(&media.platform);
    let cookie = st.settings.read().ok().and_then(|s| s.cookie(&media.platform).map(String::from));
    let mut re_resolved = false;
    let mut net_attempts = 0;

    let outcome = loop {
        if let Some(dir) = final_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                break Err(DlError::Io(format!("无法创建目录 {}：{e}", dir.display())));
            }
        }

        let mut last = Instant::now();
        let mut last_bytes = 0u64;
        let app2 = app.clone();
        let st2 = st.clone();
        let res = download_to_file(&st.dl_client, &asset.url, referer, cookie.as_deref(), &part, &control, |received, total| {
            let elapsed = last.elapsed();
            if elapsed < Duration::from_millis(250) && Some(received) != total {
                return;
            }
            let speed = ((received.saturating_sub(last_bytes)) as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
            last = Instant::now();
            last_bytes = received;
            let snap = {
                let mut inner = st2.downloads.lock();
                inner.tasks.iter_mut().find(|e| e.snap.id == id).map(|e| {
                    e.snap.received = received;
                    e.snap.total = total;
                    e.snap.speed = speed;
                    e.snap.clone()
                })
            };
            if let Some(s) = snap {
                let _ = app2.emit(EVT_PROGRESS, s);
            }
        })
        .await;

        match res {
            Err(DlError::Status(code)) if matches!(code, 403 | 404 | 410) && !re_resolved && !media.source_url.is_empty() => {
                // 直链过期：重新解析拿新地址
                re_resolved = true;
                set_note(&app, id, Some("直链已过期，正在重新解析…"));
                let settings = st.settings.read().map(|s| s.clone()).unwrap_or_default();
                match providers::resolve_url(&st.client, &settings, &media.source_url).await {
                    Ok(fresh) => match fresh.assets.iter().find(|a| a.id == asset.id) {
                        Some(a) => {
                            asset = a.clone();
                            let _ = tokio::fs::remove_file(&part).await;
                            let mut inner = st.downloads.lock();
                            if let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) {
                                e.asset = asset.clone();
                                e.snap.note = None;
                            }
                        }
                        None => break Err(DlError::Other("重新解析后找不到这个资源".into())),
                    },
                    Err(e) => break Err(DlError::Other(format!("直链已过期，重新解析失败：{e}"))),
                }
            }
            Err(DlError::Net(_)) if net_attempts < MAX_NET_RETRIES => {
                net_attempts += 1;
                set_note(&app, id, Some(&format!("网络中断，第 {net_attempts} 次重试…")));
                tokio::time::sleep(Duration::from_secs(1 << net_attempts)).await;
                if control.load(Ordering::SeqCst) != CTRL_RUN {
                    break Err(if control.load(Ordering::SeqCst) == CTRL_PAUSE { DlError::Paused } else { DlError::Canceled });
                }
            }
            other => break other,
        }
    };

    let mut notify: Option<(String, String)> = None;
    {
        let mut inner = st.downloads.lock();
        let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) else {
            // 任务已被移除
            if matches!(outcome, Err(DlError::Canceled)) {
                let _ = std::fs::remove_file(&part);
            }
            drop(inner);
            schedule(&app);
            return;
        };
        e.snap.speed = 0;
        e.snap.note = None;
        match outcome {
            Ok(size) => {
                let dest = if final_path.exists() { naming::unique_path(final_path.clone(), &|p: &Path| p.exists()) } else { final_path.clone() };
                match std::fs::rename(&part, &dest) {
                    Ok(()) => {
                        e.snap.file_path = dest.to_string_lossy().into_owned();
                        e.snap.status = TaskStatus::Done;
                        e.snap.received = size;
                        e.snap.total = Some(size);
                        e.snap.finished_at = Some(db::now());
                        let _ = st.db.record_download(&NewDownload {
                            platform: &e.snap.platform,
                            media_id: &e.snap.media_id,
                            asset_id: &e.snap.asset_id,
                            title: &e.snap.title,
                            author: &e.snap.author,
                            cover: e.snap.cover.as_deref(),
                            path: &e.snap.file_path,
                            size: size as i64,
                        });
                        notify = Some((e.snap.title.clone(), e.snap.asset_label.clone()));
                    }
                    Err(err) => {
                        e.snap.status = TaskStatus::Failed;
                        e.snap.error = Some(format!("保存文件失败：{err}"));
                    }
                }
            }
            Err(DlError::Paused) => e.snap.status = TaskStatus::Paused,
            Err(DlError::Canceled) => {
                e.snap.status = TaskStatus::Canceled;
                e.snap.received = 0;
                let _ = std::fs::remove_file(&part);
            }
            Err(err) => {
                e.snap.status = TaskStatus::Failed;
                e.snap.error = Some(err.to_string());
            }
        }
    }

    if let Some((title, label)) = notify {
        let enabled = st.settings.read().map(|s| s.notify_on_complete).unwrap_or(false);
        if enabled && !st.downloads.has_active() {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title("下载完成").body(format!("{title}（{label}）")).show();
        }
    }
    schedule(&app);
}

fn set_note(app: &AppHandle, id: u64, note: Option<&str>) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        if let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) {
            e.snap.note = note.map(String::from);
        }
    }
    emit_all(app);
}

pub fn pause(app: &AppHandle, id: u64) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        if let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) {
            match e.snap.status {
                TaskStatus::Running => e.control.store(CTRL_PAUSE, Ordering::SeqCst),
                TaskStatus::Queued => e.snap.status = TaskStatus::Paused,
                _ => {}
            }
        }
    }
    emit_all(app);
}

/// 继续暂停的任务，或重试失败 / 已取消的任务。
pub fn resume(app: &AppHandle, id: u64) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        if let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) {
            if matches!(e.snap.status, TaskStatus::Paused | TaskStatus::Failed | TaskStatus::Canceled) {
                e.snap.status = TaskStatus::Queued;
                e.snap.error = None;
            }
        }
    }
    schedule(app);
}

pub fn cancel(app: &AppHandle, id: u64) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        if let Some(e) = inner.tasks.iter_mut().find(|e| e.snap.id == id) {
            match e.snap.status {
                TaskStatus::Running => e.control.store(CTRL_CANCEL, Ordering::SeqCst),
                TaskStatus::Queued | TaskStatus::Paused | TaskStatus::Failed => {
                    e.snap.status = TaskStatus::Canceled;
                    e.snap.received = 0;
                    let _ = std::fs::remove_file(part_path(Path::new(&e.snap.file_path)));
                }
                _ => {}
            }
        }
    }
    emit_all(app);
}

pub fn remove(app: &AppHandle, id: u64) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        if let Some(pos) = inner.tasks.iter().position(|e| e.snap.id == id) {
            let e = inner.tasks.remove(pos);
            if e.snap.status == TaskStatus::Running {
                e.control.store(CTRL_CANCEL, Ordering::SeqCst);
            } else if e.snap.status != TaskStatus::Done {
                let _ = std::fs::remove_file(part_path(Path::new(&e.snap.file_path)));
            }
        }
    }
    schedule(app);
}

pub fn clear_finished(app: &AppHandle) {
    let st = state(app);
    st.downloads.lock().tasks.retain(|e| !matches!(e.snap.status, TaskStatus::Done | TaskStatus::Canceled));
    emit_all(app);
}

pub fn pause_all(app: &AppHandle) {
    let ids: Vec<u64> =
        state(app).downloads.snapshots().into_iter().filter(|s| matches!(s.status, TaskStatus::Running | TaskStatus::Queued)).map(|s| s.id).collect();
    for id in ids {
        pause(app, id);
    }
}

pub fn resume_all(app: &AppHandle) {
    let st = state(app);
    {
        let mut inner = st.downloads.lock();
        for e in inner.tasks.iter_mut().filter(|e| matches!(e.snap.status, TaskStatus::Paused | TaskStatus::Failed)) {
            e.snap.status = TaskStatus::Queued;
            e.snap.error = None;
        }
    }
    schedule(app);
}

pub fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

#[derive(Debug, thiserror::Error)]
pub enum DlError {
    #[error("已暂停")]
    Paused,
    #[error("已取消")]
    Canceled,
    #[error("服务器返回 {0}")]
    Status(u16),
    #[error("网络错误：{0}")]
    Net(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

/// 流式下载到 `part`。已有部分内容时发送 Range 请求续传；服务器不支持续传时从头下载。
pub async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
    cookie: Option<&str>,
    part: &Path,
    control: &AtomicU8,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, DlError> {
    let existing = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if !referer.is_empty() {
        req = req.header("Referer", referer);
    }
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    if existing > 0 {
        req = req.header("Range", format!("bytes={existing}-"));
    }
    let resp = req.send().await.map_err(|e| DlError::Net(e.to_string()))?;
    let status = resp.status().as_u16();

    if status == 416 && existing > 0 {
        on_progress(existing, Some(existing));
        return Ok(existing);
    }
    let (mut received, append) = match status {
        206 => (existing, true),
        200..=299 => (0, false),
        _ => return Err(DlError::Status(status)),
    };
    let total = resp.content_length().map(|len| len + received);

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .await
        .map_err(|e| DlError::Io(format!("无法写入文件 {}：{e}", part.display())))?;

    on_progress(received, total);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match control.load(Ordering::SeqCst) {
            CTRL_PAUSE => {
                let _ = file.flush().await;
                return Err(DlError::Paused);
            }
            CTRL_CANCEL => return Err(DlError::Canceled),
            _ => {}
        }
        let chunk = chunk.map_err(|e| DlError::Net(e.to_string()))?;
        file.write_all(&chunk).await.map_err(|e| DlError::Io(format!("写入失败：{e}")))?;
        received += chunk.len() as u64;
        on_progress(received, total);
    }
    file.flush().await.map_err(|e| DlError::Io(format!("写入失败：{e}")))?;
    if let Some(t) = total {
        if received < t {
            return Err(DlError::Net(format!("连接提前结束（{received}/{t} 字节）")));
        }
    }
    Ok(received)
}

pub fn build_download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(providers::MOBILE_UA)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("failed to build download client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 极简 HTTP 服务器：`/file` 支持 Range，`/forbidden` 返回 403。
    async fn serve(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let resp: Vec<u8> = if req.starts_with("GET /forbidden") {
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    } else {
                        let range_start = req
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("range: bytes="))
                            .and_then(|l| l.split('=').nth(1))
                            .and_then(|r| r.trim_end_matches('-').trim_end_matches(|c: char| c == '-' || c.is_whitespace()).parse::<usize>().ok());
                        match range_start {
                            Some(s) => {
                                let part = &body[s..];
                                let mut v = format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                                    part.len(),
                                    s,
                                    body.len() - 1,
                                    body.len()
                                )
                                .into_bytes();
                                v.extend_from_slice(part);
                                v
                            }
                            None => {
                                let mut v = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                                v.extend_from_slice(body);
                                v
                            }
                        }
                    };
                    let _ = sock.write_all(&resp).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clearclip-dl-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("f.bin.part")
    }

    #[tokio::test]
    async fn downloads_full_file() {
        let base = serve(b"hello world").await;
        let part = tmp("full");
        let ctrl = AtomicU8::new(CTRL_RUN);
        let mut last = (0, None);
        let n = download_to_file(&build_download_client(), &format!("{base}/file"), "https://ref/", None, &part, &ctrl, |r, t| last = (r, t)).await.unwrap();
        assert_eq!(n, 11);
        assert_eq!(last, (11, Some(11)));
        assert_eq!(std::fs::read(&part).unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn resumes_partial_file_with_range() {
        let base = serve(b"hello world").await;
        let part = tmp("resume");
        std::fs::write(&part, b"hello").unwrap();
        let ctrl = AtomicU8::new(CTRL_RUN);
        let n = download_to_file(&build_download_client(), &format!("{base}/file"), "", None, &part, &ctrl, |_, _| {}).await.unwrap();
        assert_eq!(n, 11);
        assert_eq!(std::fs::read(&part).unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn reports_http_status() {
        let base = serve(b"x").await;
        let part = tmp("403");
        let ctrl = AtomicU8::new(CTRL_RUN);
        let err = download_to_file(&build_download_client(), &format!("{base}/forbidden"), "", None, &part, &ctrl, |_, _| {}).await.unwrap_err();
        assert!(matches!(err, DlError::Status(403)));
    }

    #[tokio::test]
    async fn cancel_flag_stops_download() {
        let base = serve(b"0123456789").await;
        let part = tmp("cancel");
        let ctrl = AtomicU8::new(CTRL_CANCEL);
        let err = download_to_file(&build_download_client(), &format!("{base}/file"), "", None, &part, &ctrl, |_, _| {}).await.unwrap_err();
        assert!(matches!(err, DlError::Canceled));
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(part_path(Path::new("/a/b.mp4")), PathBuf::from("/a/b.mp4.part"));
    }
}
