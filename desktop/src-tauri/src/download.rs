//! 下载队列：任务持久化、并发与按网站限流、暂停 / 继续、断点续传、失败重试、直链过期重新解析、
//! 多输入任务（音视频分离时先分别下载再合并）、临时目录与文件冲突策略。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

use crate::db::{self, JobRow, NewDownload};
use crate::engine::http::{self as http_engine, HttpOptions, HttpRequest};
use crate::engine::{wait_ctrl, DlError, ResumeMeta, CTRL_CANCEL, CTRL_PAUSE, CTRL_RUN};
use crate::error::ErrorKind;
use crate::model::{AppResult, Asset, AssetKind, MediaInfo, Protocol};
use crate::settings::{ConflictPolicy, Settings};
use crate::{naming, postprocess, providers, AppState};

/// 下载中进度写入数据库的最小间隔
const PERSIST_EVERY: Duration = Duration::from_secs(2);
/// 留给 ".part" 和重名序号的路径长度余量后，路径的上限
const MAX_PATH_LEN: usize = 240;

pub const EVT_TASKS: &str = "tasks://updated";
pub const EVT_PROGRESS: &str = "tasks://progress";

pub use crate::engine::http::build_download_client;

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

/// 后处理选项（阶段 3、4 逐步实现）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PostOptions {
    /// 提取音频为 mp3 / m4a
    pub extract_audio: Option<String>,
    /// 写入封面和标题等元数据
    pub embed_metadata: bool,
}

/// 一个下载任务的内容：一个或多个输入（音视频分离时为视频轨 + 音频轨），完成后合并为一个文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSpec {
    pub inputs: Vec<Asset>,
    #[serde(default)]
    pub post: PostOptions,
}

impl JobSpec {
    fn primary(&self) -> &Asset {
        &self.inputs[0]
    }
}

/// 每个输入各自的续传信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobMeta {
    pub inputs: Vec<ResumeMeta>,
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
    pub error_kind: Option<ErrorKind>,
    pub note: Option<String>,
    pub resumable: Option<bool>,
    /// 当前步骤：download / merge / post
    pub step: Option<String>,
    /// 输入数量（大于 1 时需要合并）
    pub inputs: usize,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

struct Entry {
    snap: TaskSnapshot,
    spec: JobSpec,
    media: Arc<MediaInfo>,
    ctrl: watch::Sender<u8>,
    meta: JobMeta,
}

#[derive(Default)]
pub struct DownloadManager {
    inner: Mutex<Vec<Entry>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueResult {
    pub tasks: Vec<TaskSnapshot>,
    /// 之前已下载过（或目标文件已存在且策略为跳过）而跳过的数量
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

/// 第 `i` 个输入的临时文件路径：设置了临时目录时放在临时目录，否则与目标文件同目录。
pub fn input_part_path(settings: &Settings, job_id: i64, final_path: &Path, i: usize, count: usize) -> PathBuf {
    if !settings.temp_dir.is_empty() {
        return PathBuf::from(&settings.temp_dir).join(format!("clearclip-{job_id}-{i}.part"));
    }
    if count == 1 {
        part_path(final_path)
    } else {
        let mut s = final_path.as_os_str().to_owned();
        s.push(format!(".f{i}.part"));
        PathBuf::from(s)
    }
}

fn parse_spec(json: &str) -> Option<JobSpec> {
    serde_json::from_str::<JobSpec>(json)
        .ok()
        .filter(|s| !s.inputs.is_empty())
        .or_else(|| serde_json::from_str::<Asset>(json).ok().map(|a| JobSpec { inputs: vec![a], post: PostOptions::default() }))
}

fn parse_meta(json: &str) -> JobMeta {
    serde_json::from_str::<JobMeta>(json)
        .ok()
        .filter(|m| !m.inputs.is_empty())
        .or_else(|| serde_json::from_str::<ResumeMeta>(json).ok().map(|m| JobMeta { inputs: vec![m] }))
        .unwrap_or_default()
}

/// 已下载的字节数：分段下载的文件是预分配的，按各段记录的进度计算；单线程下载按临时文件大小计算。
fn received_on_disk(settings: &Settings, id: i64, path: &str, inputs: usize, meta: &JobMeta) -> u64 {
    (0..inputs)
        .map(|i| match meta.inputs.get(i).filter(|m| !m.segments.is_empty()) {
            Some(m) => m.segmented_received(),
            None => file_len(&input_part_path(settings, id, Path::new(path), i, inputs)),
        })
        .sum()
}

/// 启动时从数据库恢复任务：未完成的任务恢复为“已暂停”（或按设置自动继续），进度以临时文件实际大小为准。
pub fn restore(app: &AppHandle) {
    let st = state(app);
    let settings = st.settings();
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
            let (Ok(media), Some(spec)) = (serde_json::from_str::<MediaInfo>(&row.media_json), parse_spec(&row.asset_json)) else {
                log::warn!("skip unreadable job {}", row.id);
                continue;
            };
            let meta = parse_meta(&row.meta_json);
            let mut status = TaskStatus::parse(&row.status);
            if matches!(status, TaskStatus::Running | TaskStatus::Queued) {
                status = if settings.auto_resume { TaskStatus::Queued } else { TaskStatus::Paused };
            }
            let received = if status == TaskStatus::Done {
                row.received.max(0) as u64
            } else {
                received_on_disk(&settings, row.id, &row.file_path, spec.inputs.len(), &meta)
            };
            let (ctrl, _) = watch::channel(CTRL_RUN);
            let mut snap = snapshot_for(row.id, &media, &spec, &row.file_path, status, row.created_at);
            snap.received = received;
            snap.total = row.total.map(|t| t as u64);
            snap.resumable = meta.inputs.first().and_then(|m| m.resumable);
            snap.error = row.error;
            snap.finished_at = row.finished_at;
            if status == TaskStatus::Paused && received > 0 {
                snap.note = Some(format!("上次下载到 {}，继续时从这里开始", human_bytes(received)));
            }
            let e = Entry { snap, spec, media: Arc::new(media), ctrl, meta };
            persist(&st, &e);
            list.push(e);
            restored += 1;
        }
    }
    log::info!("restored {restored} download tasks");
    schedule(app);
}

fn snapshot_for(id: i64, media: &MediaInfo, spec: &JobSpec, path: &str, status: TaskStatus, created_at: i64) -> TaskSnapshot {
    let a = spec.primary();
    let label = if spec.inputs.len() > 1 { format!("{}（音视频合并）", a.label) } else { a.label.clone() };
    TaskSnapshot {
        id,
        platform: media.platform.clone(),
        platform_name: media.platform_name.clone(),
        media_id: media.id.clone(),
        title: media.title.clone(),
        author: media.author.clone(),
        cover: media.cover.clone(),
        asset_id: a.id.clone(),
        asset_label: label,
        asset_kind: a.kind,
        file_path: path.to_string(),
        status,
        received: 0,
        total: None,
        speed: 0,
        error: None,
        error_kind: None,
        note: None,
        resumable: None,
        step: None,
        inputs: spec.inputs.len(),
        created_at,
        finished_at: None,
    }
}

/// 根据选中的资源 ID 组成任务：无声视频轨自动带上它的音频轨，合并成一个任务。
pub fn build_specs(media: &MediaInfo, asset_ids: &[String]) -> Vec<JobSpec> {
    media
        .assets
        .iter()
        .filter(|a| asset_ids.contains(&a.id))
        .map(|a| {
            let mut inputs = vec![a.clone()];
            if a.kind == AssetKind::Video && a.has_audio == Some(false) {
                if let Some(audio) = a.pair_audio.as_deref().and_then(|id| media.asset(id)) {
                    inputs.push(audio.clone());
                }
            }
            JobSpec { inputs, post: PostOptions::default() }
        })
        .collect()
}

/// 把作品中选中的资源加入队列。已在队列中、之前已下载过的资源会跳过。
pub fn enqueue(app: &AppHandle, media: MediaInfo, asset_ids: &[String]) -> AppResult<EnqueueResult> {
    enqueue_with(app, media, asset_ids, PostOptions::default())
}

pub fn enqueue_with(app: &AppHandle, media: MediaInfo, asset_ids: &[String], post: PostOptions) -> AppResult<EnqueueResult> {
    let st = state(app);
    let settings = st.settings();
    let media = Arc::new(media);
    let dir = naming::target_dir(&settings.download_root(), &media, settings.subfolder_by_platform);
    let base = naming::render_base(&settings.filename_template, &media);
    let media_json = serde_json::to_string(&*media)?;

    let mut result = EnqueueResult { tasks: vec![], already_downloaded: 0, already_queued: 0 };
    {
        let mut list = st.downloads.lock();
        for mut spec in build_specs(&media, asset_ids) {
            spec.post = post.clone();
            let primary = spec.primary().clone();
            let in_queue = list.iter().any(|e| {
                e.snap.platform == media.platform
                    && e.snap.media_id == media.id
                    && e.snap.asset_id == primary.id
                    && matches!(e.snap.status, TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused | TaskStatus::Failed)
            });
            if in_queue {
                result.already_queued += 1;
                continue;
            }
            if settings.skip_existing && st.db.existing_download(&media.platform, &media.id, &primary.id)?.is_some() {
                result.already_downloaded += 1;
                continue;
            }
            let mut out_asset = primary.clone();
            if let Some(fmt) = &spec.post.extract_audio {
                out_asset.ext = fmt.clone();
            }
            let wanted = naming::fit_path(&dir, &base, &out_asset, MAX_PATH_LEN);
            let path = match settings.conflict_policy {
                ConflictPolicy::Skip if wanted.exists() => {
                    result.already_downloaded += 1;
                    continue;
                }
                ConflictPolicy::Overwrite => wanted,
                _ => {
                    let taken: Vec<String> = list.iter().filter(|e| e.snap.status != TaskStatus::Canceled).map(|e| e.snap.file_path.clone()).collect();
                    naming::unique_path(wanted, &|p: &Path| p.exists() || part_path(p).exists() || taken.iter().any(|t| Path::new(t) == p))
                }
            };
            let path_str = path.to_string_lossy().into_owned();
            let now = db::now();
            let id = st.db.insert_job(&JobRow {
                id: 0,
                media_json: media_json.clone(),
                asset_json: serde_json::to_string(&spec)?,
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
            let snap = snapshot_for(id, &media, &spec, &path_str, TaskStatus::Queued, now);
            result.tasks.push(snap.clone());
            let meta = JobMeta { inputs: vec![ResumeMeta::default(); spec.inputs.len()] };
            list.push(Entry { snap, spec, media: media.clone(), ctrl, meta });
        }
    }
    schedule(app);
    Ok(result)
}

/// 每个网站同时下载的上限；带登录账号时更保守（减半，至少 1）。
fn site_limit(st: &AppState, settings: &Settings, platform: &str) -> usize {
    let base = settings.per_site_concurrency.max(1);
    if st.cookies.has_account(platform) {
        (base / 2).max(1)
    } else {
        base
    }
}

/// 按全局并发数和按网站限流启动等待中的任务。
pub fn schedule(app: &AppHandle) {
    let st = state(app);
    let settings = st.settings();
    let mut to_start = Vec::new();
    {
        let mut list = st.downloads.lock();
        let mut running: Vec<String> = list.iter().filter(|e| e.snap.status == TaskStatus::Running).map(|e| e.snap.platform.clone()).collect();
        for i in 0..list.len() {
            if running.len() >= settings.concurrency {
                break;
            }
            if list[i].snap.status != TaskStatus::Queued {
                continue;
            }
            let platform = list[i].snap.platform.clone();
            if running.iter().filter(|p| **p == platform).count() >= site_limit(&st, &settings, &platform) {
                continue;
            }
            let e = &mut list[i];
            e.snap.status = TaskStatus::Running;
            e.snap.error = None;
            e.snap.error_kind = None;
            let _ = e.ctrl.send(CTRL_RUN);
            running.push(platform);
            to_start.push(e.snap.id);
            persist(&st, e);
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

fn build_request(st: &AppState, media: &MediaInfo, asset: &Asset) -> HttpRequest {
    let mut req = HttpRequest::new(&asset.url);
    let has = |k: &str| asset.headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(k));
    if !has("referer") {
        req = req.header("Referer", providers::referer_for(&media.platform));
    }
    // Cookie 按域名匹配：只发给它所属的网站，不会发给 CDN
    if let Some(c) = st.cookies.header_for(&asset.url) {
        req = req.header("Cookie", c);
    }
    req.headers.extend(asset.headers.iter().cloned());
    req
}

async fn run_task(app: AppHandle, id: i64) {
    let st = state(&app);
    let Some((mut spec, media, mut rx, final_path, mut meta)) =
        with_entry(&st, id, |e| (e.spec.clone(), e.media.clone(), e.ctrl.subscribe(), PathBuf::from(&e.snap.file_path), e.meta.clone()))
    else {
        return;
    };
    let settings = st.settings();
    let count = spec.inputs.len();
    meta.inputs.resize(count, ResumeMeta::default());
    let parts: Vec<PathBuf> = (0..count).map(|i| input_part_path(&settings, id, &final_path, i, count)).collect();
    let opts = HttpOptions {
        segments: settings.segments,
        min_segment_size: settings.segment_min_mb * 1024 * 1024,
        speed_limit_kbps: settings.speed_limit_kbps,
        disk_reserve: settings.disk_reserve_mb * 1024 * 1024,
        net: Some(st.net.clone()),
    };
    let start_len: u64 = (0..count).map(|i| if meta.inputs[i].segments.is_empty() { file_len(&parts[i]) } else { meta.inputs[i].segmented_received() }).sum();
    if start_len > 0 {
        set_note(&app, id, Some(&format!("从 {} 处继续下载", human_bytes(start_len))));
    }

    let mut outcome: Result<u64, DlError> = Ok(0);
    for i in 0..count {
        if spec.inputs[i].protocol != Protocol::Http {
            outcome = Err(DlError::Other("这种格式需要外部组件下载（yt-dlp / m3u8）".into()));
            break;
        }
        // 已完成的输入（合并前中断）直接跳过
        if let Some(t) = meta.inputs[i].total {
            if meta.inputs[i].segments.is_empty() && file_len(&parts[i]) == t {
                continue;
            }
        }
        set_step(&st, id, "download");
        let done_before: u64 = (0..i).map(|k| meta.inputs[k].total.unwrap_or_else(|| file_len(&parts[k]))).sum();
        outcome = download_input(&app, &st, &settings, id, &media, &mut spec, i, &parts[i], &mut meta, &mut rx, &opts, done_before).await;
        if outcome.is_err() {
            break;
        }
    }

    let final_part = if count > 1 { part_path(&final_path) } else { parts[0].clone() };
    let outcome = match outcome {
        Ok(_) if count > 1 => {
            set_step(&st, id, "merge");
            set_note(&app, id, Some("正在合并音视频…"));
            match postprocess::merge(&st, &parts, &final_part, &final_path).await {
                Ok(()) => {
                    for p in &parts {
                        let _ = std::fs::remove_file(p);
                    }
                    Ok(file_len(&final_part))
                }
                Err(e) => Err(DlError::Other(e.message)),
            }
        }
        other => other,
    };
    finish(&app, &st, id, outcome, &final_part, &final_path, &parts, &settings);
}

#[allow(clippy::too_many_arguments)]
async fn download_input(
    app: &AppHandle,
    st: &Arc<AppState>,
    settings: &Settings,
    id: i64,
    media: &MediaInfo,
    spec: &mut JobSpec,
    i: usize,
    part: &Path,
    meta: &mut JobMeta,
    rx: &mut watch::Receiver<u8>,
    opts: &HttpOptions,
    done_before: u64,
) -> Result<u64, DlError> {
    let mut re_resolved = false;
    let mut net_attempts = 0u32;
    loop {
        if let Some(dir) = part.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                return Err(DlError::Io(format!("无法创建目录 {}：{e}", dir.display())));
            }
        }
        let asset = spec.inputs[i].clone();
        let req = build_request(st, media, &asset);
        let client = match st.net.clients_for(&settings.network, &asset.url) {
            Ok(c) => c.download,
            Err(e) => return Err(DlError::Other(e.message)),
        };
        let mut baseline: Option<(Instant, u64)> = None;
        let mut last_persist = Instant::now();
        let st2 = st.clone();
        let app2 = app.clone();
        let res = http_engine::download(&client, &req, part, &mut meta.inputs[i], rx, opts, |received, total, meta_now| {
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
                e.snap.received = done_before + received;
                e.snap.total = total.map(|t| done_before + t);
                e.snap.speed = speed;
                e.snap.resumable = meta_now.resumable;
                if e.meta.inputs.len() > i {
                    e.meta.inputs[i] = meta_now.clone();
                }
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
        let m = meta.clone();
        with_entry(st, id, |e| e.meta = m);

        match res {
            Err(DlError::Status(code)) if matches!(code, 403 | 404 | 410) && !re_resolved && !media.source_url.is_empty() => {
                re_resolved = true;
                set_note(app, id, Some("下载地址已过期，正在重新解析…"));
                let ctx = st.parse_ctx(settings);
                let fresh = providers::resolve_url(&ctx, &media.source_url).await.map_err(|e| DlError::Other(format!("下载地址已过期，重新解析失败：{e}")))?;
                for k in 0..spec.inputs.len() {
                    if let Some(a) = fresh.asset(&spec.inputs[k].id) {
                        spec.inputs[k] = a.clone();
                    } else if k == i {
                        return Err(DlError::Other("重新解析后找不到这个资源".into()));
                    }
                }
                let kept = keep_part_after_refresh(st, settings, media, &spec.inputs[i], part, &mut meta.inputs[i]).await;
                let note = if kept {
                    format!("已获取新地址，从 {} 处继续", human_bytes(file_len(part)))
                } else {
                    "已获取新地址，文件已变化，从头下载".to_string()
                };
                let spec_json = serde_json::to_string(&*spec).unwrap_or_default();
                let _ = st.db.update_job_asset(id, &spec_json);
                let (s2, m2) = (spec.clone(), meta.clone());
                with_entry(st, id, |e| {
                    e.spec = s2;
                    e.meta = m2;
                });
                set_note(app, id, Some(&note));
            }
            Err(DlError::Net(msg)) if net_attempts < settings.max_retries => {
                net_attempts += 1;
                log::info!("job {id} network error, retry {net_attempts}: {msg}");
                set_note(app, id, Some(&format!("网络中断，第 {net_attempts} 次重试…")));
                let delay = Duration::from_secs(settings.retry_delay_secs.saturating_mul(1 << (net_attempts - 1).min(5)));
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    c = wait_ctrl(rx) => return Err(DlError::from_ctrl(c)),
                }
            }
            other => return other,
        }
    }
}

/// 移动到最终位置；跨磁盘时改为复制后删除。
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(app: &AppHandle, st: &AppState, id: i64, outcome: Result<u64, DlError>, part: &Path, final_path: &Path, inputs: &[PathBuf], settings: &Settings) {
    let mut notify: Option<(String, String)> = None;
    let leftovers: Vec<PathBuf> = inputs.iter().cloned().chain(std::iter::once(part.to_path_buf())).collect();
    let found = with_entry(st, id, |e| {
        e.snap.speed = 0;
        e.snap.note = None;
        e.snap.step = None;
        match outcome {
            Ok(size) => {
                let dest = match settings.conflict_policy {
                    ConflictPolicy::Overwrite => {
                        let _ = std::fs::remove_file(final_path);
                        final_path.to_path_buf()
                    }
                    _ if final_path.exists() => naming::unique_path(final_path.to_path_buf(), &|p: &Path| p.exists()),
                    _ => final_path.to_path_buf(),
                };
                if let Some(dir) = dest.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match move_file(part, &dest) {
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
                        e.snap.error_kind = Some(ErrorKind::Disk);
                    }
                }
            }
            Err(DlError::Paused) => e.snap.status = TaskStatus::Paused,
            Err(DlError::Canceled) => {
                e.snap.status = TaskStatus::Canceled;
                if !settings.keep_part_on_cancel {
                    e.snap.received = 0;
                    for p in &leftovers {
                        let _ = std::fs::remove_file(p);
                    }
                    e.meta = JobMeta { inputs: vec![ResumeMeta::default(); e.spec.inputs.len()] };
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
        // 任务已被移除：清理残留
        for p in &leftovers {
            let _ = std::fs::remove_file(p);
        }
    }
    if let Some((title, label)) = notify {
        if settings.notify_on_complete && !st.downloads.has_active() {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title("下载完成").body(format!("{title}（{label}）")).show();
        }
    }
    schedule(app);
}

/// 地址更新后判断能否保留已下载部分：新地址的文件总大小与记录一致才保留。
async fn keep_part_after_refresh(st: &AppState, settings: &Settings, media: &MediaInfo, asset: &Asset, part: &Path, meta: &mut ResumeMeta) -> bool {
    let have = file_len(part);
    let Ok(client) = st.net.clients_for(&settings.network, &asset.url).map(|c| c.download) else { return false };
    let req = build_request(st, media, asset);
    let fresh_total = http_engine::probe_total(&client, &req).await;
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

fn set_step(st: &AppState, id: i64, step: &str) {
    with_entry(st, id, |e| e.snap.step = Some(step.to_string()));
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

fn all_parts(settings: &Settings, e: &Entry) -> Vec<PathBuf> {
    let n = e.spec.inputs.len();
    let fp = Path::new(&e.snap.file_path);
    let mut v: Vec<PathBuf> = (0..n).map(|i| input_part_path(settings, e.snap.id, fp, i, n)).collect();
    v.push(part_path(fp));
    v
}

pub fn cancel(app: &AppHandle, id: i64) {
    let st = state(app);
    let settings = st.settings();
    with_entry(&st, id, |e| match e.snap.status {
        TaskStatus::Running => {
            let _ = e.ctrl.send(CTRL_CANCEL);
        }
        TaskStatus::Queued | TaskStatus::Paused | TaskStatus::Failed => {
            e.snap.status = TaskStatus::Canceled;
            if !settings.keep_part_on_cancel {
                e.snap.received = 0;
                for p in all_parts(&settings, e) {
                    let _ = std::fs::remove_file(p);
                }
                e.meta = JobMeta { inputs: vec![ResumeMeta::default(); e.spec.inputs.len()] };
            }
            persist(&st, e);
        }
        _ => {}
    });
    emit_all(app);
}

pub fn remove(app: &AppHandle, id: i64) {
    let st = state(app);
    let settings = st.settings();
    {
        let mut list = st.downloads.lock();
        if let Some(pos) = list.iter().position(|e| e.snap.id == id) {
            let e = list.remove(pos);
            if e.snap.status == TaskStatus::Running {
                let _ = e.ctrl.send(CTRL_CANCEL);
            } else if e.snap.status != TaskStatus::Done {
                for p in all_parts(&settings, &e) {
                    let _ = std::fs::remove_file(p);
                }
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

/// 下载目录和临时目录里不属于任何任务的 .part 文件。
pub fn orphan_parts(app: &AppHandle) -> Vec<OrphanPart> {
    let st = state(app);
    let settings = st.settings();
    let known: Vec<PathBuf> = st.downloads.lock().iter().filter(|e| e.snap.status != TaskStatus::Done).flat_map(|e| all_parts(&settings, e)).collect();
    let mut out = Vec::new();
    collect_parts(&settings.download_root(), 0, &known, &mut out);
    if !settings.temp_dir.is_empty() {
        collect_parts(Path::new(&settings.temp_dir), 3, &known, &mut out);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaKind;

    fn media() -> MediaInfo {
        let mut v1080 = Asset::video("https://cdn/v1080.m4s".into(), Some(1920), Some(1080));
        v1080.id = "video-1080".into();
        v1080.has_audio = Some(false);
        v1080.pair_audio = Some("audio-best".into());
        let mut audio = Asset::base("audio-best", AssetKind::Audio, "https://cdn/a.m4s".into(), "音频轨", "m4a");
        audio.has_audio = Some(true);
        let v720 = Asset { id: "video-720".into(), ..Asset::video("https://cdn/v720.mp4".into(), Some(1280), Some(720)) };
        MediaInfo {
            platform: "bilibili".into(),
            platform_name: "B站".into(),
            id: "BV1".into(),
            source_url: "x".into(),
            title: "t".into(),
            author: "a".into(),
            cover: None,
            duration_ms: None,
            kind: MediaKind::Video,
            width: None,
            height: None,
            published_at: None,
            assets: vec![v1080, v720, audio],
            entries: vec![],
            series: None,
            extractor: None,
        }
    }

    #[test]
    fn silent_video_is_paired_with_audio() {
        let m = media();
        let specs = build_specs(&m, &["video-1080".into(), "video-720".into()]);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].inputs.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["video-1080", "audio-best"]);
        assert_eq!(specs[1].inputs.len(), 1, "video with audio is downloaded alone");
    }

    #[test]
    fn legacy_rows_parse() {
        let a = Asset::video("u".into(), None, None);
        let spec = parse_spec(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(spec.inputs.len(), 1);
        let meta = parse_meta(r#"{"etag":"e","total":5}"#);
        assert_eq!(meta.inputs[0].etag.as_deref(), Some("e"));
    }

    #[test]
    fn part_paths_for_inputs() {
        let s = Settings::default();
        let f = Path::new("/d/a.mp4");
        assert_eq!(input_part_path(&s, 1, f, 0, 1), PathBuf::from("/d/a.mp4.part"));
        assert_eq!(input_part_path(&s, 1, f, 1, 2), PathBuf::from("/d/a.mp4.f1.part"));
        let t = Settings { temp_dir: "/tmp/cc".into(), ..Settings::default() };
        assert_eq!(input_part_path(&t, 7, f, 0, 1), PathBuf::from("/tmp/cc/clearclip-7-0.part"));
    }

    #[test]
    fn move_file_works_within_same_fs() {
        let dir = std::env::temp_dir().join(format!("clearclip-mv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), b"x").unwrap();
        move_file(&dir.join("a"), &dir.join("b")).unwrap();
        assert!(dir.join("b").exists() && !dir.join("a").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_roundtrip() {
        for s in [TaskStatus::Queued, TaskStatus::Running, TaskStatus::Paused, TaskStatus::Done, TaskStatus::Failed, TaskStatus::Canceled] {
            assert_eq!(TaskStatus::parse(s.as_str()), s);
        }
    }
}
