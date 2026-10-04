//! 下载队列：任务持久化、并发控制、暂停 / 继续、断点续传（带一致性校验）、失败重试、直链过期重新解析。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use crate::db::{self, JobRow, NewDownload};
use crate::model::{AppResult, Asset, AssetKind, MediaInfo};
use crate::{naming, providers, AppState};

const CTRL_RUN: u8 = 0;
const CTRL_PAUSE: u8 = 1;
const CTRL_CANCEL: u8 = 2;
/// 下载中进度写入数据库的最小间隔
const PERSIST_EVERY: Duration = Duration::from_secs(2);
/// 留给 ".part" 和重名序号的路径长度余量后，路径的上限
const MAX_PATH_LEN: usize = 240;

pub const EVT_TASKS: &str = "tasks://updated";
pub const EVT_PROGRESS: &str = "tasks://progress";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Canceled,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Paused => "paused",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Canceled => "canceled",
        }
    }

    fn parse(s: &str) -> TaskStatus {
        match s {
            "queued" => TaskStatus::Queued,
            "running" => TaskStatus::Running,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            "canceled" => TaskStatus::Canceled,
            _ => TaskStatus::Paused,
        }
    }
}

/// 续传所需的服务器信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub total: Option<u64>,
    /// 服务器是否支持 Range；None 表示还不知道
    pub resumable: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: i64,
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
    pub error_kind: Option<crate::error::ErrorKind>,
    pub note: Option<String>,
    pub resumable: Option<bool>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

struct Entry {
    snap: TaskSnapshot,
    asset: Asset,
    media: Arc<MediaInfo>,
    ctrl: watch::Sender<u8>,
    meta: ResumeMeta,
}

#[derive(Default)]
pub struct DownloadManager {
    inner: Mutex<Vec<Entry>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueResult {
    pub tasks: Vec<TaskSnapshot>,
    /// 之前已下载过、文件仍在而跳过的数量
    pub already_downloaded: usize,
    /// 已经在队列里（等待、下载中、暂停、失败）而跳过的数量
    pub already_queued: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPart {
    pub path: String,
    pub size: u64,
    pub modified: i64,
}

impl DownloadManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        self.lock().iter().map(|e| e.snap.clone()).collect()
    }

    pub fn has_active(&self) -> bool {
        self.lock().iter().any(|e| matches!(e.snap.status, TaskStatus::Running | TaskStatus::Queued))
    }

    fn part_paths(&self) -> Vec<PathBuf> {
        self.lock().iter().filter(|e| e.snap.status != TaskStatus::Done).map(|e| part_path(Path::new(&e.snap.file_path))).collect()
    }
}

fn state(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

fn emit_all(app: &AppHandle) {
    let _ = app.emit(EVT_TASKS, state(app).downloads.snapshots());
}

fn persist(st: &AppState, e: &Entry) {
    let meta = serde_json::to_string(&e.meta).unwrap_or_else(|_| "{}".into());
    if let Err(err) = st.db.update_job(
        e.snap.id,
        e.snap.status.as_str(),
        e.snap.received as i64,
        e.snap.total.map(|t| t as i64),
        e.snap.error.as_deref(),
        &e.snap.file_path,
        &meta,
        e.snap.finished_at,
    ) {
        log::warn!("persist job {} failed: {err}", e.snap.id);
    }
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// 启动时从数据库恢复任务：未完成的任务恢复为“已暂停”（或按设置自动继续），进度以 .part 文件实际大小为准。
pub fn restore(app: &AppHandle) {
    let st = state(app);
    let auto = st.settings().auto_resume;
    let rows = match st.db.load_jobs() {
        Ok(r) => r,
        Err(e) => {
            log::error!("load jobs failed: {e}");
            return;
        }
    };
    let mut restored = 0;
    {
        let mut list = st.downloads.lock();
        for row in rows {
            let (Ok(media), Ok(asset)) = (serde_json::from_str::<MediaInfo>(&row.media_json), serde_json::from_str::<Asset>(&row.asset_json)) else {
                log::warn!("skip unreadable job {}", row.id);
                continue;
            };
            let meta: ResumeMeta = serde_json::from_str(&row.meta_json).unwrap_or_default();
            let mut status = TaskStatus::parse(&row.status);
            let mut received = row.received.max(0) as u64;
            if matches!(status, TaskStatus::Running | TaskStatus::Queued) {
                status = if auto { TaskStatus::Queued } else { TaskStatus::Paused };
            }
            if status != TaskStatus::Done {
                received = file_len(&part_path(Path::new(&row.file_path)));
            }
            let (ctrl, _) = watch::channel(CTRL_RUN);
            let mut e = Entry { snap: snapshot_for(row.id, &media, &asset, &row.file_path, status, row.created_at), asset, media: Arc::new(media), ctrl, meta };
            e.snap.received = received;
            e.snap.total = row.total.map(|t| t as u64).or(e.meta.total);
            e.snap.resumable = e.meta.resumable;
            e.snap.error = row.error;
            e.snap.finished_at = row.finished_at;
            if status == TaskStatus::Paused && received > 0 {
                e.snap.note = Some(format!("上次下载到 {}，继续时从这里开始", human_bytes(received)));
            }
            persist(&st, &e);
            list.push(e);
            restored += 1;
        }
    }
    log::info!("restored {restored} download tasks");
    schedule(app);
}

fn snapshot_for(id: i64, media: &MediaInfo, asset: &Asset, path: &str, status: TaskStatus, created_at: i64) -> TaskSnapshot {
    TaskSnapshot {
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
        file_path: path.to_string(),
        status,
        received: 0,
        total: None,
        speed: 0,
        error: None,
        error_kind: None,
        note: None,
        resumable: None,
        created_at,
        finished_at: None,
    }
}

/// 把作品中选中的资源加入队列。已在队列中或之前已下载过的资源会跳过。
pub fn enqueue(app: &AppHandle, media: MediaInfo, asset_ids: &[String]) -> AppResult<EnqueueResult> {
    let st = state(app);
    let settings = st.settings();
    let media = Arc::new(media);
    let dir = naming::target_dir(&settings.download_root(), &media, settings.subfolder_by_platform);
    let base = naming::render_base(&settings.filename_template, &media);
    let media_json = serde_json::to_string(&*media)?;

    let mut result = EnqueueResult { tasks: vec![], already_downloaded: 0, already_queued: 0 };
    {
        let mut list = st.downloads.lock();
        for asset in media.assets.iter().filter(|a| asset_ids.contains(&a.id)) {
            let in_queue = list.iter().any(|e| {
                e.snap.platform == media.platform
                    && e.snap.media_id == media.id
                    && e.snap.asset_id == asset.id
                    && matches!(e.snap.status, TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused | TaskStatus::Failed)
            });
            if in_queue {
                result.already_queued += 1;
                continue;
            }
            if settings.skip_existing && st.db.existing_download(&media.platform, &media.id, &asset.id)?.is_some() {
                result.already_downloaded += 1;
                continue;
            }
            let wanted = naming::fit_path(&dir, &base, asset, MAX_PATH_LEN);
            let taken: Vec<String> = list.iter().filter(|e| e.snap.status != TaskStatus::Canceled).map(|e| e.snap.file_path.clone()).collect();
            let path = naming::unique_path(wanted, &|p: &Path| p.exists() || part_path(p).exists() || taken.iter().any(|t| Path::new(t) == p));
            let path_str = path.to_string_lossy().into_owned();
            let now = db::now();
            let id = st.db.insert_job(&JobRow {
                id: 0,
                media_json: media_json.clone(),
                asset_json: serde_json::to_string(asset)?,
                file_path: path_str.clone(),
                status: "queued".into(),
                received: 0,
                total: None,
                error: None,
                meta_json: "{}".into(),
                created_at: now,
                finished_at: None,
            })?;
            let (ctrl, _) = watch::channel(CTRL_RUN);
            let snap = snapshot_for(id, &media, asset, &path_str, TaskStatus::Queued, now);
            result.tasks.push(snap.clone());
            list.push(Entry { snap, asset: asset.clone(), media: media.clone(), ctrl, meta: ResumeMeta::default() });
        }
    }
    schedule(app);
    Ok(result)
}

/// 按并发数启动等待中的任务。
pub fn schedule(app: &AppHandle) {
    let st = state(app);
    let concurrency = st.settings().concurrency;
    let mut to_start = Vec::new();
    {
        let mut list = st.downloads.lock();
        let mut running = list.iter().filter(|e| e.snap.status == TaskStatus::Running).count();
        for e in list.iter_mut() {
            if running >= concurrency {
                break;
            }
            if e.snap.status == TaskStatus::Queued {
                e.snap.status = TaskStatus::Running;
                e.snap.error = None;
                e.snap.error_kind = None;
                let _ = e.ctrl.send(CTRL_RUN);
                running += 1;
                to_start.push(e.snap.id);
                persist(&st, e);
            }
        }
    }
    for id in to_start {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_task(app, id).await });
    }
    emit_all(app);
}

fn with_entry<R>(st: &AppState, id: i64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
    let mut list = st.downloads.lock();
    list.iter_mut().find(|e| e.snap.id == id).map(f)
}

async fn run_task(app: AppHandle, id: i64) {
    let st = state(&app);
    let Some((mut asset, media, mut rx, final_path, mut meta)) =
        with_entry(&st, id, |e| (e.asset.clone(), e.media.clone(), e.ctrl.subscribe(), PathBuf::from(&e.snap.file_path), e.meta.clone()))
    else {
        return;
    };
    let settings = st.settings();
    let part = part_path(&final_path);
    let referer = providers::referer_for(&media.platform);
    let mut re_resolved = false;
    let mut net_attempts = 0u32;
    let start_len = file_len(&part);
    if start_len > 0 {
        set_note(&app, id, Some(&format!("从 {} 处继续下载", human_bytes(start_len))));
    }

    let outcome = loop {
        if let Some(dir) = final_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                break Err(DlError::Io(format!("无法创建目录 {}：{e}", dir.display())));
            }
        }
        // Cookie 按域名匹配：只发给它所属的网站，不会发给 CDN
        let cookie = st.cookies.header_for(&asset.url);
        let req = HttpRequest { url: &asset.url, referer, cookie: cookie.as_deref() };

        let mut baseline: Option<(Instant, u64)> = None;
        let mut last_persist = Instant::now();
        let st2 = st.clone();
        let app2 = app.clone();
        let res = download_to_file(&st.dl_client, &req, &part, &mut meta, &mut rx, |received, total, meta_now| {
            let now = Instant::now();
            let (t0, b0) = *baseline.get_or_insert((now, received));
            let elapsed = now.duration_since(t0);
            if elapsed < Duration::from_millis(250) && Some(received) != total {
                return;
            }
            let speed = (received.saturating_sub(b0) as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
            baseline = Some((now, received));
            let save = last_persist.elapsed() >= PERSIST_EVERY;
            if save {
                last_persist = now;
            }
            let snap = with_entry(&st2, id, |e| {
                e.snap.received = received;
                e.snap.total = total;
                e.snap.speed = speed;
                e.snap.resumable = meta_now.resumable;
                e.meta = meta_now.clone();
                if save {
                    persist(&st2, e);
                }
                e.snap.clone()
            });
            if let Some(s) = snap {
                let _ = app2.emit(EVT_PROGRESS, s);
            }
        })
        .await;
        with_entry(&st, id, |e| e.meta = meta.clone());

        match res {
            Err(DlError::Status(code)) if matches!(code, 403 | 404 | 410) && !re_resolved && !media.source_url.is_empty() => {
                re_resolved = true;
                set_note(&app, id, Some("下载地址已过期，正在重新解析…"));
                let ctx = st.parse_ctx(&settings);
                match providers::resolve_url(&ctx, &media.source_url).await {
                    Ok(fresh) => match fresh.assets.iter().find(|a| a.id == asset.id) {
                        Some(a) => {
                            asset = a.clone();
                            let kept = keep_part_after_refresh(&st.dl_client, &asset, referer, &st, &part, &mut meta).await;
                            let note = if kept {
                                format!("已获取新地址，从 {} 处继续", human_bytes(file_len(&part)))
                            } else {
                                "已获取新地址，文件已变化，从头下载".to_string()
                            };
                            let asset_json = serde_json::to_string(&asset).unwrap_or_default();
                            let _ = st.db.update_job_asset(id, &asset_json);
                            with_entry(&st, id, |e| {
                                e.asset = asset.clone();
                                e.meta = meta.clone();
                            });
                            set_note(&app, id, Some(&note));
                        }
                        None => break Err(DlError::Other("重新解析后找不到这个资源".into())),
                    },
                    Err(e) => break Err(DlError::Other(format!("下载地址已过期，重新解析失败：{e}"))),
                }
            }
            Err(DlError::Net(msg)) if net_attempts < settings.max_retries => {
                net_attempts += 1;
                log::info!("job {id} network error, retry {net_attempts}: {msg}");
                set_note(&app, id, Some(&format!("网络中断，第 {net_attempts} 次重试…")));
                let delay = Duration::from_secs(settings.retry_delay_secs.saturating_mul(1 << (net_attempts - 1).min(5)));
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    c = wait_ctrl(&mut rx) => break Err(if c == CTRL_PAUSE { DlError::Paused } else { DlError::Canceled }),
                }
            }
            other => break other,
        }
    };

    finish(&app, &st, id, outcome, &part, &final_path, settings.keep_part_on_cancel, settings.notify_on_complete);
}

#[allow(clippy::too_many_arguments)]
fn finish(app: &AppHandle, st: &AppState, id: i64, outcome: Result<u64, DlError>, part: &Path, final_path: &Path, keep_part: bool, notify_enabled: bool) {
    let mut notify: Option<(String, String)> = None;
    let found = with_entry(st, id, |e| {
        e.snap.speed = 0;
        e.snap.note = None;
        match outcome {
            Ok(size) => {
                let dest = if final_path.exists() { naming::unique_path(final_path.to_path_buf(), &|p: &Path| p.exists()) } else { final_path.to_path_buf() };
                match std::fs::rename(part, &dest) {
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
                        e.snap.error_kind = Some(crate::error::ErrorKind::Disk);
                    }
                }
            }
            Err(DlError::Paused) => e.snap.status = TaskStatus::Paused,
            Err(DlError::Canceled) => {
                e.snap.status = TaskStatus::Canceled;
                if !keep_part {
                    e.snap.received = 0;
                    let _ = std::fs::remove_file(part);
                }
            }
            Err(err) => {
                log::warn!("job {} failed: {err}", e.snap.id);
                e.snap.status = TaskStatus::Failed;
                e.snap.error_kind = Some(err.kind());
                e.snap.error = Some(err.to_string());
            }
        }
        persist(st, e);
    });
    if found.is_none() {
        // 任务已被移除：下载中被移除时清理残留
        let _ = std::fs::remove_file(part);
    }
    if let Some((title, label)) = notify {
        if notify_enabled && !st.downloads.has_active() {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title("下载完成").body(format!("{title}（{label}）")).show();
        }
    }
    schedule(app);
}

/// 地址更新后判断能否保留已下载部分：新地址的文件总大小与记录一致才保留。
async fn keep_part_after_refresh(client: &reqwest::Client, asset: &Asset, referer: &str, st: &AppState, part: &Path, meta: &mut ResumeMeta) -> bool {
    let have = file_len(part);
    let cookie = st.cookies.header_for(&asset.url);
    let req = HttpRequest { url: &asset.url, referer, cookie: cookie.as_deref() };
    let fresh_total = probe_total(client, &req).await;
    let keep = have > 0 && meta.total.is_some() && fresh_total == meta.total;
    if keep {
        // 新地址的 ETag 与旧 CDN 不同，续传时只依赖大小和 Content-Range 校验
        meta.etag = None;
        meta.last_modified = None;
    } else {
        let _ = std::fs::remove_file(part);
        *meta = ResumeMeta::default();
    }
    keep
}

fn set_note(app: &AppHandle, id: i64, note: Option<&str>) {
    let st = state(app);
    with_entry(&st, id, |e| e.snap.note = note.map(String::from));
    emit_all(app);
}

pub fn pause(app: &AppHandle, id: i64) {
    let st = state(app);
    with_entry(&st, id, |e| match e.snap.status {
        TaskStatus::Running => {
            let _ = e.ctrl.send(CTRL_PAUSE);
        }
        TaskStatus::Queued => {
            e.snap.status = TaskStatus::Paused;
            persist(&st, e);
        }
        _ => {}
    });
    emit_all(app);
}

/// 继续暂停的任务，或重试失败 / 已取消的任务。
pub fn resume(app: &AppHandle, id: i64) {
    let st = state(app);
    with_entry(&st, id, |e| {
        if matches!(e.snap.status, TaskStatus::Paused | TaskStatus::Failed | TaskStatus::Canceled) {
            e.snap.status = TaskStatus::Queued;
            e.snap.error = None;
            e.snap.error_kind = None;
            persist(&st, e);
        }
    });
    schedule(app);
}

pub fn cancel(app: &AppHandle, id: i64) {
    let st = state(app);
    let keep = st.settings().keep_part_on_cancel;
    with_entry(&st, id, |e| match e.snap.status {
        TaskStatus::Running => {
            let _ = e.ctrl.send(CTRL_CANCEL);
        }
        TaskStatus::Queued | TaskStatus::Paused | TaskStatus::Failed => {
            e.snap.status = TaskStatus::Canceled;
            if !keep {
                e.snap.received = 0;
                let _ = std::fs::remove_file(part_path(Path::new(&e.snap.file_path)));
            }
            persist(&st, e);
        }
        _ => {}
    });
    emit_all(app);
}

pub fn remove(app: &AppHandle, id: i64) {
    let st = state(app);
    {
        let mut list = st.downloads.lock();
        if let Some(pos) = list.iter().position(|e| e.snap.id == id) {
            let e = list.remove(pos);
            if e.snap.status == TaskStatus::Running {
                let _ = e.ctrl.send(CTRL_CANCEL);
            } else if e.snap.status != TaskStatus::Done {
                let _ = std::fs::remove_file(part_path(Path::new(&e.snap.file_path)));
            }
            let _ = st.db.delete_job(id);
        }
    }
    schedule(app);
}

pub fn clear_finished(app: &AppHandle) {
    let st = state(app);
    st.downloads.lock().retain(|e| !matches!(e.snap.status, TaskStatus::Done | TaskStatus::Canceled));
    let _ = st.db.delete_jobs_with_status(&["done", "canceled"]);
    emit_all(app);
}

pub fn pause_all(app: &AppHandle) {
    let ids: Vec<i64> =
        state(app).downloads.snapshots().into_iter().filter(|s| matches!(s.status, TaskStatus::Running | TaskStatus::Queued)).map(|s| s.id).collect();
    for id in ids {
        pause(app, id);
    }
}

pub fn resume_all(app: &AppHandle) {
    let st = state(app);
    {
        let mut list = st.downloads.lock();
        for e in list.iter_mut().filter(|e| matches!(e.snap.status, TaskStatus::Paused | TaskStatus::Failed)) {
            e.snap.status = TaskStatus::Queued;
            e.snap.error = None;
            e.snap.error_kind = None;
            persist(&st, e);
        }
    }
    schedule(app);
}

/// 下载目录里不属于任何任务的 .part 文件（例如旧版本崩溃后留下的）。
pub fn orphan_parts(app: &AppHandle) -> Vec<OrphanPart> {
    let st = state(app);
    let known = st.downloads.part_paths();
    let mut out = Vec::new();
    collect_parts(&st.settings().download_root(), 0, &known, &mut out);
    out
}

fn collect_parts(dir: &Path, depth: usize, known: &[PathBuf], out: &mut Vec<OrphanPart>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(md) = entry.metadata() else { continue };
        if md.is_dir() {
            collect_parts(&p, depth + 1, known, out);
        } else if p.extension().is_some_and(|x| x == "part") && !known.contains(&p) {
            let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            out.push(OrphanPart { path: p.to_string_lossy().into_owned(), size: md.len(), modified });
        }
    }
}

/// 删除指定的孤立 .part 文件；只接受当前确实是孤立文件的路径。
pub fn delete_orphans(app: &AppHandle, paths: &[String]) -> usize {
    let orphans = orphan_parts(app);
    paths.iter().filter(|p| orphans.iter().any(|o| &o.path == *p)).filter(|p| std::fs::remove_file(p).is_ok()).count()
}

pub fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn human_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if n as f64 >= MB {
        format!("{:.1} MB", n as f64 / MB)
    } else {
        format!("{:.0} KB", (n as f64 / 1024.0).max(1.0))
    }
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

impl DlError {
    pub fn kind(&self) -> crate::error::ErrorKind {
        use crate::error::ErrorKind;
        match self {
            DlError::Status(code) => crate::error::AppError::from_status(*code, "").kind,
            DlError::Net(_) => ErrorKind::Network,
            DlError::Io(_) => ErrorKind::Disk,
            _ => ErrorKind::Other,
        }
    }
}

pub struct HttpRequest<'a> {
    pub url: &'a str,
    pub referer: &'a str,
    pub cookie: Option<&'a str>,
}

impl HttpRequest<'_> {
    fn build(&self, client: &reqwest::Client) -> reqwest::RequestBuilder {
        let mut req = client.get(self.url);
        if !self.referer.is_empty() {
            req = req.header("Referer", self.referer);
        }
        if let Some(c) = self.cookie {
            req = req.header("Cookie", c);
        }
        req
    }
}

/// 等待控制信号变为暂停或取消，返回该信号。
async fn wait_ctrl(rx: &mut watch::Receiver<u8>) -> u8 {
    loop {
        let v = *rx.borrow_and_update();
        if v != CTRL_RUN {
            return v;
        }
        if rx.changed().await.is_err() {
            // 发送端已释放（任务被移除），视为取消
            return CTRL_CANCEL;
        }
    }
}

/// 解析 `Content-Range: bytes 100-199/1000`，返回 (起点, 总大小)。
pub fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    let total = total.trim().parse().ok();
    Some((start, total))
}

fn header_str(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

/// 用 1 字节的 Range 请求获取文件总大小。
pub async fn probe_total(client: &reqwest::Client, req: &HttpRequest<'_>) -> Option<u64> {
    let resp = req.build(client).header("Range", "bytes=0-0").send().await.ok()?;
    match resp.status().as_u16() {
        206 => header_str(&resp, "content-range").and_then(|v| parse_content_range(&v)).and_then(|(_, t)| t),
        200 => resp.content_length(),
        _ => None,
    }
}

/// 流式下载到 `part`，支持断点续传：
/// - 已有部分内容时发送 `Range`，并用 `If-Range`（ETag / Last-Modified）确认服务器上的文件没变；
/// - 收到 206 时核对 `Content-Range` 起点与总大小，不一致就从头下载；
/// - 服务器返回 200（不支持续传或文件已变化）时从头下载；
/// - 暂停 / 取消立即生效，不等待下一块数据。
pub async fn download_to_file(
    client: &reqwest::Client,
    req: &HttpRequest<'_>,
    part: &Path,
    meta: &mut ResumeMeta,
    ctrl: &mut watch::Receiver<u8>,
    mut on_progress: impl FnMut(u64, Option<u64>, &ResumeMeta),
) -> Result<u64, DlError> {
    let ctrl_err = |c: u8| if c == CTRL_PAUSE { DlError::Paused } else { DlError::Canceled };
    for attempt in 0..2 {
        let existing = if attempt == 0 { file_len(part) } else { 0 };
        if attempt > 0 {
            let _ = tokio::fs::remove_file(part).await;
        }
        let mut rb = req.build(client);
        if existing > 0 {
            rb = rb.header("Range", format!("bytes={existing}-"));
            if let Some(v) = meta.etag.as_deref().or(meta.last_modified.as_deref()) {
                rb = rb.header("If-Range", v);
            }
        }
        let resp = tokio::select! {
            r = rb.send() => r.map_err(|e| DlError::Net(e.without_url().to_string()))?,
            c = wait_ctrl(ctrl) => return Err(ctrl_err(c)),
        };
        let status = resp.status().as_u16();

        if status == 416 && existing > 0 {
            if meta.total == Some(existing) {
                on_progress(existing, Some(existing), meta);
                return Ok(existing);
            }
            *meta = ResumeMeta::default();
            continue;
        }

        let (start, append) = match status {
            206 => {
                let cr = header_str(&resp, "content-range").and_then(|v| parse_content_range(&v));
                match cr {
                    Some((s, total)) if s == existing && (meta.total.is_none() || total.is_none() || total == meta.total) => {
                        if total.is_some() {
                            meta.total = total;
                        }
                        (existing, true)
                    }
                    _ => {
                        log::info!("content-range mismatch, restarting download from zero");
                        *meta = ResumeMeta::default();
                        continue;
                    }
                }
            }
            200..=299 => {
                if existing > 0 {
                    log::info!("server returned {status} for a range request; restarting from zero");
                }
                meta.total = resp.content_length();
                (0, false)
            }
            _ => return Err(DlError::Status(status)),
        };

        let accepts_ranges = header_str(&resp, "accept-ranges").is_some_and(|v| v.contains("bytes"));
        meta.resumable = Some(status == 206 || accepts_ranges);
        if let Some(etag) = header_str(&resp, "etag").filter(|e| !e.starts_with("W/")) {
            meta.etag = Some(etag);
        }
        if let Some(lm) = header_str(&resp, "last-modified") {
            meta.last_modified = Some(lm);
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .await
            .map_err(|e| DlError::Io(format!("无法写入文件 {}：{e}", part.display())))?;

        let mut received = start;
        let total = meta.total;
        on_progress(received, total, meta);
        let mut stream = resp.bytes_stream();
        loop {
            let next = tokio::select! {
                n = stream.next() => n,
                c = wait_ctrl(ctrl) => {
                    let _ = file.flush().await;
                    return Err(ctrl_err(c));
                }
            };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| DlError::Net(e.without_url().to_string()))?;
            file.write_all(&chunk).await.map_err(|e| DlError::Io(format!("写入失败：{e}")))?;
            received += chunk.len() as u64;
            on_progress(received, total, meta);
        }
        file.flush().await.map_err(|e| DlError::Io(format!("写入失败：{e}")))?;
        if let Some(t) = total {
            if received < t {
                return Err(DlError::Net(format!("连接提前结束（{received}/{t} 字节）")));
            }
        }
        return Ok(received);
    }
    Err(DlError::Other("服务器的续传响应不一致，请稍后重试".into()))
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

    const BODY: &[u8] = b"0123456789abcdefghij";

    fn header<'a>(req: &'a str, name: &str) -> Option<&'a str> {
        req.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    /// 模拟服务器。路径：
    /// /file（ETag "v1"，支持 Range 与 If-Range）、/changed（ETag "v2"，If-Range 不匹配时返回整份）、
    /// /badrange（206 但起点错误）、/norange（忽略 Range）、/stall（发送一半后挂起）、/forbidden（403）
    async fn serve() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let range = header(&req, "range").and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
                    let if_range = header(&req, "if-range").map(String::from);
                    let etag = if path == "/changed" { "\"v2\"" } else { "\"v1\"" };
                    let full = |extra: &str| {
                        let mut v = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {etag}\r\nAccept-Ranges: bytes\r\n{extra}Connection: close\r\n\r\n",
                            BODY.len()
                        )
                        .into_bytes();
                        v.extend_from_slice(BODY);
                        v
                    };
                    let partial = |s: usize| {
                        let part = &BODY[s..];
                        let mut v = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nETag: {etag}\r\nConnection: close\r\n\r\n",
                            part.len(),
                            s,
                            BODY.len() - 1,
                            BODY.len()
                        )
                        .into_bytes();
                        v.extend_from_slice(part);
                        v
                    };
                    let resp: Vec<u8> = match path.as_str() {
                        "/forbidden" => b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                        "/norange" => b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n0123456789abcdefghij".to_vec(),
                        "/badrange" => match range {
                            Some(_) => {
                                let mut v =
                                    b"HTTP/1.1 206 Partial Content\r\nContent-Length: 20\r\nContent-Range: bytes 0-19/20\r\nConnection: close\r\n\r\n".to_vec();
                                v.extend_from_slice(BODY);
                                v
                            }
                            None => full(""),
                        },
                        "/stall" => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", BODY.len());
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(&BODY[..5]).await;
                            tokio::time::sleep(Duration::from_secs(20)).await;
                            return;
                        }
                        _ => match range {
                            Some(s) if s >= BODY.len() => b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                            Some(s) if if_range.as_deref().is_none_or(|v| v == etag) => partial(s),
                            _ => full(""),
                        },
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
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("f.bin.part")
    }

    async fn dl(url: &str, part: &Path, meta: &mut ResumeMeta) -> Result<u64, DlError> {
        let (_tx, mut rx) = watch::channel(CTRL_RUN);
        let req = HttpRequest { url, referer: "", cookie: None };
        download_to_file(&build_download_client(), &req, part, meta, &mut rx, |_, _, _| {}).await
    }

    #[tokio::test]
    async fn downloads_full_file_and_records_meta() {
        let base = serve().await;
        let part = tmp("full");
        let mut meta = ResumeMeta::default();
        assert_eq!(dl(&format!("{base}/file"), &part, &mut meta).await.unwrap(), 20);
        assert_eq!(std::fs::read(&part).unwrap(), BODY);
        assert_eq!(meta.etag.as_deref(), Some("\"v1\""));
        assert_eq!(meta.total, Some(20));
        assert_eq!(meta.resumable, Some(true));
    }

    #[tokio::test]
    async fn resumes_with_matching_etag() {
        let base = serve().await;
        let part = tmp("resume");
        std::fs::write(&part, &BODY[..8]).unwrap();
        let mut meta = ResumeMeta { etag: Some("\"v1\"".into()), total: Some(20), ..Default::default() };
        assert_eq!(dl(&format!("{base}/file"), &part, &mut meta).await.unwrap(), 20);
        assert_eq!(std::fs::read(&part).unwrap(), BODY);
    }

    #[tokio::test]
    async fn restarts_when_file_changed_on_server() {
        let base = serve().await;
        let part = tmp("changed");
        std::fs::write(&part, b"XXXXXXXX").unwrap();
        let mut meta = ResumeMeta { etag: Some("\"v1\"".into()), total: Some(20), ..Default::default() };
        assert_eq!(dl(&format!("{base}/changed"), &part, &mut meta).await.unwrap(), 20);
        assert_eq!(std::fs::read(&part).unwrap(), BODY, "stale bytes must be discarded");
        assert_eq!(meta.etag.as_deref(), Some("\"v2\""));
    }

    #[tokio::test]
    async fn restarts_on_content_range_mismatch() {
        let base = serve().await;
        let part = tmp("badrange");
        std::fs::write(&part, b"XXXXXXXX").unwrap();
        let mut meta = ResumeMeta::default();
        assert_eq!(dl(&format!("{base}/badrange"), &part, &mut meta).await.unwrap(), 20);
        assert_eq!(std::fs::read(&part).unwrap(), BODY);
    }

    #[tokio::test]
    async fn restarts_when_server_ignores_range() {
        let base = serve().await;
        let part = tmp("norange");
        std::fs::write(&part, b"XXXXXXXX").unwrap();
        let mut meta = ResumeMeta::default();
        assert_eq!(dl(&format!("{base}/norange"), &part, &mut meta).await.unwrap(), 20);
        assert_eq!(std::fs::read(&part).unwrap(), BODY);
        assert_eq!(meta.resumable, Some(false));
    }

    #[tokio::test]
    async fn complete_file_with_416_is_done() {
        let base = serve().await;
        let part = tmp("416");
        std::fs::write(&part, BODY).unwrap();
        let mut meta = ResumeMeta { total: Some(20), ..Default::default() };
        assert_eq!(dl(&format!("{base}/file"), &part, &mut meta).await.unwrap(), 20);
    }

    #[tokio::test]
    async fn reports_http_status() {
        let base = serve().await;
        let part = tmp("403");
        let err = dl(&format!("{base}/forbidden"), &part, &mut ResumeMeta::default()).await.unwrap_err();
        assert!(matches!(err, DlError::Status(403)));
        assert_eq!(err.kind(), crate::error::ErrorKind::NeedLogin);
    }

    #[tokio::test]
    async fn pause_takes_effect_while_stream_is_stalled() {
        let base = serve().await;
        let part = tmp("stall");
        let (tx, mut rx) = watch::channel(CTRL_RUN);
        let url = format!("{base}/stall");
        let task = tokio::spawn(async move {
            let req = HttpRequest { url: &url, referer: "", cookie: None };
            let mut meta = ResumeMeta::default();
            download_to_file(&build_download_client(), &req, &part, &mut meta, &mut rx, |_, _, _| {}).await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t0 = Instant::now();
        tx.send(CTRL_PAUSE).unwrap();
        let res = tokio::time::timeout(Duration::from_secs(2), task).await.expect("pause must not wait for the read timeout").unwrap();
        assert!(matches!(res, Err(DlError::Paused)));
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn probe_total_reads_content_range() {
        let base = serve().await;
        let url = format!("{base}/file");
        let req = HttpRequest { url: &url, referer: "", cookie: None };
        assert_eq!(probe_total(&build_download_client(), &req).await, Some(20));
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range("bytes 100-199/1000"), Some((100, Some(1000))));
        assert_eq!(parse_content_range("bytes 0-0/*"), Some((0, None)));
        assert_eq!(parse_content_range("items 1-2/3"), None);
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(part_path(Path::new("/a/b.mp4")), PathBuf::from("/a/b.mp4.part"));
    }

    #[test]
    fn status_roundtrip() {
        for s in [TaskStatus::Queued, TaskStatus::Running, TaskStatus::Paused, TaskStatus::Done, TaskStatus::Failed, TaskStatus::Canceled] {
            assert_eq!(TaskStatus::parse(s.as_str()), s);
        }
    }
}
