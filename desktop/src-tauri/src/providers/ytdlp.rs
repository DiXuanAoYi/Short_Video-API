//! 通用解析：交给 yt-dlp（支持上千个网站）。内置解析器不认识的链接走这里。
//!
//! - 解析：`yt-dlp -J --flat-playlist`，把格式列表转换成统一的 [`MediaInfo`]
//! - 下载：普通 HTTP 直链和 m3u8 用内置引擎（分段并行、断点续传），DASH 等其他协议交给 yt-dlp 下载
//! - 代理按网络分流规则传给 yt-dlp；Cookie 写入仅当前用户可读的临时文件，运行结束后把轮换过的 Cookie 合并回存储

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use url::Url;

use super::Ctx;
use crate::cookies::{self, CookieStore};
use crate::model::{quality_label, AppError, AppResult, Asset, AssetKind, ErrorKind, MediaInfo, MediaKind, PlaylistEntry, Protocol, SeriesInfo};
use crate::settings::Route;

/// 解析超时：播放列表较大时 yt-dlp 也需要一些时间
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(120);

/// 临时 Cookie 文件：只有当前用户可读，离开作用域时删除。
pub struct CookieFile {
    pub path: PathBuf,
    site: String,
}

impl CookieFile {
    /// 把某网站默认账号的 Cookie 写成 Netscape 格式；没有 Cookie 时返回 None。
    pub fn create(store: &CookieStore, url: &str) -> Option<CookieFile> {
        let site = Url::parse(url).ok().and_then(|u| cookies::site_for_url(&u))?;
        let list = store.cookies_for_site(&site);
        if list.is_empty() {
            return None;
        }
        let path = std::env::temp_dir().join(format!(
            "clearclip-cookies-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        write_private(&path, cookies::to_netscape(&list).as_bytes()).ok()?;
        Some(CookieFile { path, site })
    }

    /// yt-dlp 运行结束后，把它更新过的 Cookie（如轮换的会话令牌）合并回存储。
    pub fn merge_back(&self, store: &CookieStore) {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return };
        let before: HashMap<(String, String), String> = store.cookies_for_site(&self.site).into_iter().map(|c| ((c.name, c.domain), c.value)).collect();
        let changed: Vec<_> = cookies::parse_netscape(&text)
            .into_iter()
            .filter(|c| cookies::site_for_domain(&c.domain) == self.site)
            .filter(|c| before.get(&(c.name.clone(), c.domain.clone())) != Some(&c.value))
            .collect();
        if !changed.is_empty() {
            log::info!("yt-dlp refreshed {} cookies for {}", changed.len(), self.site);
            let _ = store.merge_into_default(&self.site, changed);
        }
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    crate::secret::restrict_permissions(path);
    Ok(())
}

/// yt-dlp 的公共参数：忽略用户配置、代理、Cookie、ffmpeg 位置。返回命令和临时 Cookie 文件（需保持存活到命令结束）。
pub fn command(ctx: &Ctx<'_>, url: &str) -> AppResult<(tokio::process::Command, Option<CookieFile>)> {
    let bin = ctx.ytdlp.clone().ok_or_else(|| AppError::new(ErrorKind::NeedUpdate, "这个网站需要 yt-dlp 组件。请在“设置 → 组件”中安装 yt-dlp 后重试。"))?;
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["--ignore-config", "--no-warnings", "--no-color", "--socket-timeout", "20"]);
    if let Some(proxy) = proxy_arg(ctx, url) {
        cmd.args(["--proxy", &proxy]);
    }
    if let Some(ff) = &ctx.ffmpeg {
        cmd.arg("--ffmpeg-location").arg(ff);
    }
    let cookie_file = CookieFile::create(ctx.cookies, url);
    if let Some(cf) = &cookie_file {
        cmd.arg("--cookies").arg(&cf.path);
    }
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    Ok((cmd, cookie_file))
}

/// 按分流规则得到 yt-dlp 的 `--proxy` 参数：直连传空字符串（禁止使用环境变量里的代理），系统代理不传。
fn proxy_arg(ctx: &Ctx<'_>, url: &str) -> Option<String> {
    let host = Url::parse(url).ok()?.host_str()?.to_string();
    match ctx.settings.network.route_for_host(&host) {
        Route::Direct => Some(String::new()),
        Route::System => None,
        Route::Proxy(id) => ctx.settings.network.proxy_url(&id).map(String::from),
    }
}

pub async fn resolve(ctx: &Ctx<'_>, url: &str) -> AppResult<MediaInfo> {
    let (mut cmd, cookie_file) = command(ctx, url)?;
    cmd.args(["-J", "--flat-playlist", "--"]).arg(url).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(RESOLVE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| AppError::new(ErrorKind::Network, "yt-dlp 解析超时"))?
        .map_err(|e| AppError::new(ErrorKind::NeedUpdate, format!("无法运行 yt-dlp：{e}")))?;
    if let Some(cf) = &cookie_file {
        cf.merge_back(ctx.cookies);
    }
    if !out.status.success() {
        return Err(classify_stderr(&String::from_utf8_lossy(&out.stderr)));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| AppError::parser(format!("yt-dlp 输出无法解析：{e}")))?;
    ctx.record("ytdlp", "info", url, &String::from_utf8_lossy(&out.stdout));
    from_json(&v, url)
}

/// 把 yt-dlp 的报错转换成带类型的错误，界面据此给出下一步操作。
pub fn classify_stderr(stderr: &str) -> AppError {
    let line = stderr.lines().rev().find(|l| l.contains("ERROR")).unwrap_or_else(|| stderr.lines().last().unwrap_or("")).trim();
    let msg = line.trim_start_matches("ERROR:").trim();
    let lower = msg.to_ascii_lowercase();
    let kind = if lower.contains("unsupported url") {
        ErrorKind::Unsupported
    } else if lower.contains("sign in")
        || lower.contains("login")
        || lower.contains("log in")
        || lower.contains("cookies")
        || lower.contains("members-only")
        || lower.contains("private video")
    {
        ErrorKind::NeedLogin
    } else if lower.contains("drm") {
        ErrorKind::Encrypted
    } else if lower.contains("country") || lower.contains("geo") {
        ErrorKind::GeoBlocked
    } else if lower.contains("429") || lower.contains("too many requests") {
        ErrorKind::RateLimited
    } else if lower.contains("404") || lower.contains("not found") || lower.contains("unavailable") || lower.contains("removed") {
        ErrorKind::NotFound
    } else if lower.contains("timed out") || lower.contains("unable to download") || lower.contains("connection") {
        ErrorKind::Network
    } else if lower.contains("please report this issue") || lower.contains("update") {
        ErrorKind::NeedUpdate
    } else {
        ErrorKind::Other
    };
    let msg = if msg.is_empty() { "yt-dlp 解析失败".to_string() } else { format!("yt-dlp：{msg}") };
    AppError::new(kind, msg)
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty() && *x != "none").map(String::from)
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

fn u(v: &Value, k: &str) -> Option<u32> {
    f(v, k).filter(|x| *x > 0.0).map(|x| x as u32)
}

fn short_codec(c: &str) -> String {
    let c = c.to_ascii_lowercase();
    let name = if c.starts_with("avc") || c.starts_with("h264") {
        "H.264"
    } else if c.starts_with("hev") || c.starts_with("hvc") || c.starts_with("h265") {
        "H.265"
    } else if c.starts_with("av01") || c == "av1" {
        "AV1"
    } else if c.starts_with("vp9") || c.starts_with("vp09") {
        "VP9"
    } else if c.starts_with("mp4a") || c == "aac" {
        "AAC"
    } else if c.starts_with("opus") {
        "Opus"
    } else {
        return c.split('.').next().unwrap_or(&c).to_string();
    };
    name.into()
}

/// 一个格式应使用的下载方式。
fn protocol_of(fmt: &Value) -> Protocol {
    let p = s(fmt, "protocol").unwrap_or_default();
    let has_fragments = fmt.get("fragments").is_some_and(|x| x.as_array().is_some_and(|a| !a.is_empty()));
    match p.as_str() {
        "http" | "https" if !has_fragments => Protocol::Http,
        "m3u8" | "m3u8_native" => Protocol::Hls,
        _ => Protocol::Ytdlp,
    }
}

fn format_asset(fmt: &Value) -> Option<Asset> {
    let format_id = s(fmt, "format_id").unwrap_or_else(|| "best".into());
    let url = s(fmt, "url").unwrap_or_default();
    let ext = s(fmt, "ext").unwrap_or_else(|| "mp4".into());
    let note = s(fmt, "format_note").unwrap_or_default().to_ascii_lowercase();
    if ext == "mhtml" || note.contains("storyboard") || fmt.get("has_drm").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    let vcodec = s(fmt, "vcodec");
    let acodec = s(fmt, "acodec");
    let video_none = fmt.get("vcodec").and_then(|x| x.as_str()) == Some("none");
    let audio_none = fmt.get("acodec").and_then(|x| x.as_str()) == Some("none");
    let (width, height) = (u(fmt, "width"), u(fmt, "height"));
    let is_audio = video_none || (vcodec.is_none() && width.is_none() && height.is_none() && acodec.is_some());
    let kind = if is_audio { AssetKind::Audio } else { AssetKind::Video };
    let mut protocol = protocol_of(fmt);
    if url.is_empty() {
        protocol = Protocol::Ytdlp;
    }

    let bitrate = f(fmt, "tbr").or_else(|| f(fmt, "abr")).or_else(|| f(fmt, "vbr")).map(|x| x as u64).filter(|x| *x > 0);
    let mut a = Asset::base(format!("f-{format_id}"), kind, url, "", ext.clone());
    a.width = width;
    a.height = height;
    a.protocol = protocol;
    a.format_id = Some(format_id);
    a.vcodec = vcodec.clone();
    a.acodec = acodec.clone();
    a.bitrate = bitrate;
    a.filesize = f(fmt, "filesize").or_else(|| f(fmt, "filesize_approx")).map(|x| x as u64).filter(|x| *x > 0);
    a.fps = f(fmt, "fps").map(|x| x as f32).filter(|x| *x > 0.0);
    a.headers = fmt
        .get("http_headers")
        .and_then(|h| h.as_object())
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).filter(|(k, _)| !k.eq_ignore_ascii_case("cookie")).collect())
        .unwrap_or_default();
    if kind == AssetKind::Video {
        a.has_audio = Some(!audio_none);
        a.quality = quality_label(width, height).or_else(|| height.map(|h| format!("{h}P")));
        let mut parts = vec![a.quality.clone().unwrap_or_else(|| s(fmt, "format_note").unwrap_or_else(|| "视频".into()))];
        if let Some(fps) = a.fps.filter(|x| *x > 30.5) {
            parts.push(format!("{}帧", fps.round()));
        }
        if let Some(c) = &vcodec {
            parts.push(short_codec(c));
        }
        parts.push(ext.to_uppercase());
        a.label = parts.join(" · ");
    } else {
        let mut parts = vec!["音频".to_string()];
        if let Some(c) = &acodec {
            parts.push(short_codec(c));
        }
        if let Some(b) = bitrate {
            parts.push(format!("{b}kbps"));
        }
        parts.push(ext.to_uppercase());
        a.label = parts.join(" · ");
    }
    Some(a)
}

/// 给无声视频轨挑一个最合适的音频轨：容器兼容（mp4 配 m4a，webm 配 webm）优先，再按码率。
fn best_audio_for(video: &Asset, audios: &[&Asset]) -> Option<String> {
    let compatible = |a: &Asset| match video.ext.as_str() {
        "mp4" => a.ext == "m4a" || a.ext == "mp4",
        "webm" => a.ext == "webm",
        _ => true,
    };
    audios.iter().max_by_key(|a| (compatible(a), a.bitrate.unwrap_or(0))).map(|a| a.id.clone())
}

fn platform_of(v: &Value, url: &str) -> (String, String) {
    let key = s(v, "extractor_key").or_else(|| s(v, "ie_key")).unwrap_or_else(|| "Generic".into());
    let name = key.trim_end_matches("Tab").trim_end_matches("Playlist").to_string();
    // 通用提取器（网页嵌入、直链等）用域名作平台名
    if name.starts_with("Generic") || name == "HTML5MediaEmbed" {
        let host = Url::parse(url).ok().and_then(|u| u.host_str().map(cookies::registrable_domain)).unwrap_or_default();
        return (host.clone(), if host.is_empty() { "网页".into() } else { host });
    }
    (name.to_ascii_lowercase(), name)
}

/// 把 yt-dlp 的 JSON 转换成统一结构。
pub fn from_json(v: &Value, source_url: &str) -> AppResult<MediaInfo> {
    let (platform, platform_name) = platform_of(v, source_url);
    let id = s(v, "id").unwrap_or_default();
    let title = s(v, "title").or_else(|| s(v, "fulltitle")).unwrap_or_else(|| id.clone());
    let author = s(v, "uploader").or_else(|| s(v, "channel")).or_else(|| s(v, "creator")).or_else(|| s(v, "uploader_id")).unwrap_or_default();
    let cover = s(v, "thumbnail").or_else(|| v.get("thumbnails").and_then(|t| t.as_array()).and_then(|a| a.iter().rev().find_map(|t| s(t, "url"))));
    let mut info = MediaInfo {
        platform,
        platform_name,
        id,
        source_url: source_url.to_string(),
        title,
        author,
        cover: cover.clone(),
        duration_ms: f(v, "duration").map(|d| (d * 1000.0) as u64),
        kind: MediaKind::Video,
        width: u(v, "width"),
        height: u(v, "height"),
        published_at: f(v, "timestamp").map(|t| t as i64),
        assets: vec![],
        entries: vec![],
        series: None,
        extractor: Some(format!("yt-dlp:{}", s(v, "extractor_key").unwrap_or_else(|| "Generic".into()))),
    };
    if let Some(series) = s(v, "series") {
        info.series = Some(SeriesInfo { name: series, season: u(v, "season_number"), episode: u(v, "episode_number") });
    }

    // 条目已经完整解析（带格式列表，如网页里嵌了多个视频、多视频推文）：合并成一个结果，每个条目一个视频
    if let Some(list) = v
        .get("entries")
        .and_then(|e| e.as_array())
        .filter(|a| !a.is_empty() && a.iter().all(|e| e.get("formats").is_some() || s(e, "url").is_some_and(|u| e.get("ext").is_some() && !u.is_empty())))
    {
        let mut assets = vec![];
        for (i, e) in list.iter().enumerate() {
            let Ok(sub) = from_json(e, source_url) else { continue };
            let prefix = format!("e{}-", i + 1);
            let name = s(e, "title").unwrap_or_else(|| format!("第 {} 个", i + 1));
            for mut a in sub.assets.into_iter().filter(|a| a.kind != AssetKind::Cover) {
                a.id = format!("{prefix}{}", a.id);
                a.pair_audio = a.pair_audio.map(|p| format!("{prefix}{p}"));
                a.label = format!("{}. {name} · {}", i + 1, a.label);
                assets.push(a);
            }
        }
        if !assets.is_empty() {
            info.kind = if assets.iter().any(|a| a.kind == AssetKind::Video) { MediaKind::Video } else { MediaKind::Audio };
            info.assets = assets;
            if let Some(c) = cover {
                info.assets.push(Asset::cover(c));
            }
            return Ok(info);
        }
    }

    if s(v, "_type").as_deref() == Some("playlist") || v.get("entries").is_some_and(|e| e.is_array()) {
        info.kind = MediaKind::Playlist;
        info.author = s(v, "uploader").or_else(|| s(v, "channel")).unwrap_or_default();
        let entries = v.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
        info.entries = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let url = s(e, "webpage_url").or_else(|| s(e, "url"))?;
                if !url.starts_with("http") {
                    return None;
                }
                // 去掉 yt-dlp 内部传参用的片段
                let url = url.split("#__youtubedl_smuggle").next().unwrap_or(&url).to_string();
                Some(PlaylistEntry {
                    // 没有 ID 时用链接（序号会随新内容插入而变化，不能用来去重）
                    id: s(e, "id").unwrap_or_else(|| url.clone()),
                    title: s(e, "title").unwrap_or_else(|| format!("第 {} 条", i + 1)),
                    url,
                    duration_ms: f(e, "duration").map(|d| (d * 1000.0) as u64),
                    thumbnail: s(e, "thumbnail")
                        .or_else(|| e.get("thumbnails").and_then(|t| t.as_array()).and_then(|a| a.iter().rev().find_map(|t| s(t, "url")))),
                    index: u(e, "playlist_index").unwrap_or(i as u32 + 1),
                })
            })
            .collect();
        if info.entries.is_empty() {
            return Err(AppError::not_found("这个列表是空的，或者需要登录才能查看。"));
        }
        return Ok(info);
    }

    let formats: Vec<Value> = match v.get("formats").and_then(|x| x.as_array()) {
        Some(a) if !a.is_empty() => a.clone(),
        // 只有单个格式时，格式字段直接在顶层
        _ if s(v, "url").is_some() => vec![v.clone()],
        _ => vec![],
    };
    let mut assets: Vec<Asset> = formats.iter().filter_map(format_asset).collect();
    let audios: Vec<Asset> = assets.iter().filter(|a| a.kind == AssetKind::Audio).cloned().collect();
    let audio_refs: Vec<&Asset> = audios.iter().collect();
    for a in assets.iter_mut().filter(|a| a.kind == AssetKind::Video && a.has_audio == Some(false)) {
        a.pair_audio = best_audio_for(a, &audio_refs);
    }
    // 视频按清晰度、码率从高到低；音频按码率
    assets.sort_by(|a, b| {
        let rank = |x: &Asset| (x.kind == AssetKind::Video, x.short_side().unwrap_or(0), x.has_audio != Some(false), x.bitrate.unwrap_or(0));
        rank(b).cmp(&rank(a))
    });
    if assets.is_empty() {
        return Err(AppError::not_found("没有找到可下载的格式（可能需要登录、受地区限制或有 DRM 保护）。"));
    }
    if !assets.iter().any(|a| a.kind == AssetKind::Video) {
        info.kind = MediaKind::Audio;
    }
    if let Some(c) = cover {
        let mut cv = Asset::cover(c);
        if !["jpg", "png", "webp", "gif"].contains(&cv.ext.as_str()) {
            cv.ext = "jpg".into();
        }
        assets.push(cv);
    }
    info.assets = assets;
    Ok(info)
}

/// yt-dlp 下载进度（来自 `--progress-template`）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: Option<u64>,
}

pub const PROGRESS_PREFIX: &str = "CCPROG";

pub fn progress_template() -> String {
    format!("download:{PROGRESS_PREFIX} %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s")
}

pub fn parse_progress(line: &str) -> Option<Progress> {
    let rest = line.trim().strip_prefix(PROGRESS_PREFIX)?;
    let nums: Vec<Option<f64>> = rest.split_whitespace().map(|x| x.parse::<f64>().ok()).collect();
    let get = |i: usize| nums.get(i).copied().flatten().filter(|x| *x >= 0.0).map(|x| x as u64);
    Some(Progress { downloaded: get(0)?, total: get(1).or(get(2)).filter(|t| *t > 0), speed: get(3) })
}

/// 用 yt-dlp 下载单个格式到 `out`（yt-dlp 自己负责 .part 续传）。暂停或取消时结束进程。
#[allow(clippy::too_many_arguments)]
pub async fn download(
    ctx: &Ctx<'_>,
    page_url: &str,
    format_id: &str,
    ext: &str,
    out: &Path,
    ctrl: &mut tokio::sync::watch::Receiver<u8>,
    rate_limit_kbps: u64,
    mut on_progress: impl FnMut(Progress),
) -> Result<u64, crate::engine::DlError> {
    use crate::engine::{wait_ctrl, DlError};
    let (mut cmd, cookie_file) = command(ctx, page_url).map_err(|e| DlError::Other(e.message))?;
    // yt-dlp 的后处理（ffmpeg）按扩展名判断格式，不能直接输出到 .part 文件
    let target = output_path(out, ext);
    cmd.args(["--newline", "--no-playlist", "--no-mtime", "--continue", "--progress", "--progress-template"])
        .arg(progress_template())
        .args(["-f", format_id, "-o"])
        // 输出路径按原样使用：转义模板里的 %
        .arg(target.to_string_lossy().replace('%', "%%"))
        .arg("--no-download-archive");
    if rate_limit_kbps > 0 {
        cmd.args(["--limit-rate", &format!("{rate_limit_kbps}K")]);
    }
    cmd.arg("--").arg(page_url).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| DlError::Other(format!("无法运行 yt-dlp：{e}")))?;
    let stdout = child.stdout.take().ok_or_else(|| DlError::Other("yt-dlp 无输出".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| DlError::Other("yt-dlp 无输出".into()))?;
    let err_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(l)) = lines.next_line().await {
            tail.push(l);
            if tail.len() > 20 {
                tail.remove(0);
            }
        }
        tail.join("\n")
    });
    let mut lines = BufReader::new(stdout).lines();
    let result = loop {
        tokio::select! {
            l = lines.next_line() => match l {
                Ok(Some(line)) => if let Some(p) = parse_progress(&line) { on_progress(p) },
                _ => break None,
            },
            c = wait_ctrl(ctrl) => {
                let _ = child.kill().await;
                break Some(DlError::from_ctrl(c));
            }
        }
    };
    if let Some(e) = result {
        if let Some(cf) = &cookie_file {
            cf.merge_back(ctx.cookies);
        }
        return Err(e);
    }
    let status = child.wait().await.map_err(|e| DlError::Other(e.to_string()))?;
    let tail = err_task.await.unwrap_or_default();
    if let Some(cf) = &cookie_file {
        cf.merge_back(ctx.cookies);
    }
    if !status.success() {
        let e = classify_stderr(&tail);
        return Err(match e.kind {
            ErrorKind::Network => DlError::Net(e.message),
            _ => DlError::Other(e.message),
        });
    }
    std::fs::rename(&target, out).map_err(|_| DlError::Other("yt-dlp 没有生成文件".into()))?;
    std::fs::metadata(out).map(|m| m.len()).map_err(|e| DlError::Io(e.to_string()))
}

/// 交给 yt-dlp 的输出路径：临时文件名后加上真实扩展名，完成后再改回临时文件名。
pub fn output_path(part: &Path, ext: &str) -> PathBuf {
    let ext = if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) { "mp4" } else { ext };
    let mut s = part.as_os_str().to_owned();
    s.push(format!(".{ext}"));
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "id": "abc", "title": "Test video", "uploader": "Someone", "extractor_key": "Youtube",
            "duration": 61.5, "timestamp": 1700000000, "thumbnail": "https://i.ytimg.com/vi/abc/maxresdefault.webp",
            "series": "Show", "season_number": 1, "episode_number": 3,
            "formats": [
                {"format_id": "sb0", "ext": "mhtml", "format_note": "storyboard", "url": "https://x/sb"},
                {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "abr": 129.5, "url": "https://r/140", "protocol": "https"},
                {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "abr": 140.0, "url": "https://r/251", "protocol": "https"},
                {"format_id": "137", "ext": "mp4", "vcodec": "avc1.640028", "acodec": "none", "width": 1920, "height": 1080, "tbr": 4000.0, "fps": 30, "url": "https://r/137", "protocol": "https", "http_headers": {"User-Agent": "UA", "Cookie": "x=1"}},
                {"format_id": "248", "ext": "webm", "vcodec": "vp9", "acodec": "none", "width": 1920, "height": 1080, "tbr": 2600.0, "url": "https://r/248", "protocol": "https"},
                {"format_id": "18", "ext": "mp4", "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "width": 640, "height": 360, "tbr": 500.0, "url": "https://r/18", "protocol": "https"},
                {"format_id": "hls-720", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a", "width": 1280, "height": 720, "url": "https://r/720.m3u8", "protocol": "m3u8_native"},
                {"format_id": "dash-x", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "width": 854, "height": 480, "url": "https://r/manifest.mpd", "protocol": "http_dash_segments", "fragments": [{"path": "a"}]}
            ]
        })
    }

    #[test]
    fn converts_formats() {
        let info = from_json(&sample(), "https://www.youtube.com/watch?v=abc").unwrap();
        assert_eq!(info.platform, "youtube");
        assert_eq!(info.kind, MediaKind::Video);
        assert_eq!(info.duration_ms, Some(61500));
        assert_eq!(info.series.as_ref().unwrap().episode, Some(3));
        let ids: Vec<&str> = info.assets.iter().map(|a| a.id.as_str()).collect();
        assert!(!ids.contains(&"f-sb0"), "storyboards skipped");
        // 1080P 排在最前，带声音的 720P 在 480P 之前
        assert_eq!(ids[0], "f-137");
        let v137 = info.asset("f-137").unwrap();
        assert_eq!(v137.has_audio, Some(false));
        assert_eq!(v137.pair_audio.as_deref(), Some("f-140"), "mp4 pairs with m4a");
        assert!(v137.label.contains("H.264"), "{}", v137.label);
        assert!(v137.headers.iter().all(|(k, _)| k != "Cookie"), "cookies never copied from yt-dlp headers");
        assert_eq!(info.asset("f-248").unwrap().pair_audio.as_deref(), Some("f-251"), "webm pairs with webm");
        assert_eq!(info.asset("f-hls-720").unwrap().protocol, Protocol::Hls);
        assert_eq!(info.asset("f-dash-x").unwrap().protocol, Protocol::Ytdlp);
        assert_eq!(info.asset("f-18").unwrap().protocol, Protocol::Http);
        assert_eq!(info.asset("f-140").unwrap().kind, AssetKind::Audio);
        assert_eq!(info.assets.last().unwrap().kind, AssetKind::Cover);
        assert_eq!(info.default_asset_ids(), vec!["f-137"]);
    }

    #[test]
    fn converts_playlist() {
        let v = serde_json::json!({"_type": "playlist", "id": "PL1", "title": "List", "extractor_key": "YoutubeTab", "uploader": "Ch",
            "entries": [{"id": "a", "title": "A", "url": "https://www.youtube.com/watch?v=a", "duration": 10},
                        {"id": "b", "title": "B", "url": "https://www.youtube.com/watch?v=b"},
                        {"id": "c", "url": "c-relative"}]});
        let info = from_json(&v, "https://www.youtube.com/playlist?list=PL1").unwrap();
        assert_eq!(info.kind, MediaKind::Playlist);
        assert_eq!(info.platform, "youtube");
        assert_eq!(info.entries.len(), 2);
        assert_eq!(info.entries[1].index, 2);
        assert_eq!(info.entries[0].duration_ms, Some(10000));
    }

    #[test]
    fn resolved_entries_are_flattened() {
        let v = serde_json::json!({"_type": "playlist", "id": "multi", "title": "合集", "extractor_key": "Generic",
            "entries": [
                {"id": "m1", "title": "合集 (1)", "formats": [{"format_id": "0", "url": "http://h/a.mp4", "ext": "mp4", "protocol": "http"}], "webpage_url": "http://h/multi.html"},
                {"id": "m2", "title": "合集 (2)", "formats": [{"format_id": "0", "url": "http://h/b.mp4", "ext": "mp4", "protocol": "http"}], "webpage_url": "http://h/multi.html"}]});
        let info = from_json(&v, "http://h/multi.html").unwrap();
        assert_eq!(info.kind, MediaKind::Video);
        let ids: Vec<&str> = info.assets.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["e1-f-0", "e2-f-0"]);
        assert!(info.assets[1].label.starts_with("2. 合集 (2)"));
    }

    #[test]
    fn single_format_at_top_level() {
        let v = serde_json::json!({"id": "x", "title": "Direct", "extractor_key": "Generic", "url": "https://cdn.example.com/a.mp4", "ext": "mp4", "protocol": "https"});
        let info = from_json(&v, "https://www.example.com/page").unwrap();
        assert_eq!(info.platform, "example.com");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].protocol, Protocol::Http);
    }

    #[test]
    fn stderr_classification() {
        assert_eq!(classify_stderr("ERROR: [youtube] abc: Sign in to confirm your age").kind, ErrorKind::NeedLogin);
        assert_eq!(classify_stderr("ERROR: Unsupported URL: https://x").kind, ErrorKind::Unsupported);
        assert_eq!(classify_stderr("ERROR: [x] The uploader has not made this video available in your country").kind, ErrorKind::GeoBlocked);
        assert_eq!(classify_stderr("ERROR: HTTP Error 429: Too Many Requests").kind, ErrorKind::RateLimited);
        assert!(classify_stderr("WARNING: a\nERROR: boom").message.ends_with("boom"));
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("CCPROG 1024 4096 NA 512.5"), Some(Progress { downloaded: 1024, total: Some(4096), speed: Some(512) }));
        assert_eq!(parse_progress("CCPROG 1024 NA 8192.0 NA"), Some(Progress { downloaded: 1024, total: Some(8192), speed: None }));
        assert_eq!(parse_progress("[download] 10%"), None);
    }

    #[test]
    fn cookie_file_is_private_and_removed() {
        let store = CookieStore::in_memory();
        store.upsert("youtube.com", "a", cookies::parse_header("SID=1; HSID=2", "youtube.com")).unwrap();
        let path;
        {
            let cf = CookieFile::create(&store, "https://www.youtube.com/watch?v=x").unwrap();
            path = cf.path.clone();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("SID\t1"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            }
            // 模拟 yt-dlp 轮换了一个 Cookie
            std::fs::write(&path, text.replace("SID\t1", "SID\t9")).unwrap();
            cf.merge_back(&store);
        }
        assert!(!path.exists());
        assert!(store.header_for("https://www.youtube.com/").unwrap().contains("SID=9"));
        assert!(CookieFile::create(&store, "https://vimeo.com/1").is_none());
    }
}
