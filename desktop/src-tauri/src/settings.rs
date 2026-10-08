use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    /// 只用本地解析。
    Local,
    /// 只用远程 PHP 接口（`jxindex.php`）。
    Remote,
    /// 先本地解析，失败后改用远程接口。
    #[default]
    LocalThenRemote,
}

/// 网络出口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Route {
    Direct,
    /// 系统代理（Windows / macOS 系统设置，或环境变量）
    #[default]
    System,
    /// 自定义代理，内容为代理 ID
    Proxy(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRule {
    /// 域名后缀，如 `youtube.com`（同时匹配其子域名）
    pub pattern: String,
    pub route: Route,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyDef {
    pub id: String,
    pub name: String,
    /// `http://127.0.0.1:7890`、`socks5://127.0.0.1:1080` 等
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkSettings {
    pub default_route: Route,
    pub rules: Vec<RouteRule>,
    pub proxies: Vec<ProxyDef>,
}

/// 国内平台默认直连（走海外代理时常被拦截或返回不同内容）。
pub const DIRECT_DOMAINS: &[&str] = &[
    "douyin.com",
    "iesdouyin.com",
    "douyinvod.com",
    "douyinpic.com",
    "snssdk.com",
    "amemv.com",
    "kuaishou.com",
    "chenzhongtech.com",
    "gifshow.com",
    "kwaicdn.com",
    "yximgs.com",
    "xiaohongshu.com",
    "xhscdn.com",
    "xhslink.com",
    "bilibili.com",
    "bilivideo.com",
    "bilivideo.cn",
    "hdslb.com",
    "b23.tv",
    "weibo.com",
    "weibo.cn",
    "sinaimg.cn",
    "weibocdn.com",
    "t.cn",
    "douyu.com",
    "douyucdn.cn",
    "huya.com",
    "iqiyi.com",
    "youku.com",
    "acfun.cn",
    "ximalaya.com",
];

/// 海外网站默认走系统代理。
pub const PROXY_DOMAINS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "googlevideo.com",
    "ytimg.com",
    "google.com",
    "pornhub.com",
    "phncdn.com",
    "pixiv.net",
    "pximg.net",
    "twitter.com",
    "x.com",
    "twimg.com",
    "instagram.com",
    "cdninstagram.com",
    "facebook.com",
    "fbcdn.net",
    "twitch.tv",
    "ttvnw.net",
    "tiktok.com",
    "vimeo.com",
    "dailymotion.com",
    "nicovideo.jp",
    "soundcloud.com",
];

impl Default for NetworkSettings {
    fn default() -> Self {
        let rules = DIRECT_DOMAINS
            .iter()
            .map(|d| RouteRule { pattern: d.to_string(), route: Route::Direct })
            .chain(PROXY_DOMAINS.iter().map(|d| RouteRule { pattern: d.to_string(), route: Route::System }))
            .collect();
        NetworkSettings { default_route: Route::System, rules, proxies: vec![] }
    }
}

impl NetworkSettings {
    /// 按最长后缀匹配找出某个主机使用的出口。
    pub fn route_for_host(&self, host: &str) -> Route {
        let host = host.to_ascii_lowercase();
        self.rules
            .iter()
            .filter(|r| {
                let p = r.pattern.trim().trim_start_matches('.').to_ascii_lowercase();
                !p.is_empty() && (host == p || host.ends_with(&format!(".{p}")))
            })
            .max_by_key(|r| r.pattern.len())
            .map(|r| r.route.clone())
            .unwrap_or_else(|| self.default_route.clone())
    }

    pub fn proxy_url(&self, id: &str) -> Option<&str> {
        self.proxies.iter().find(|p| p.id == id).map(|p| p.url.as_str())
    }
}

/// 默认选择哪种清晰度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    /// 最高画质
    #[default]
    Best,
    /// 不超过 1080P 的最高画质
    Max1080,
    /// 省空间：不超过 720P、码率最低
    Small,
    /// 只要音频
    Audio,
}

/// 目标文件已存在时的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// 在文件名后加 (1)、(2)…
    #[default]
    Rename,
    /// 跳过，不下载
    Skip,
    /// 覆盖原文件
    Overwrite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: String,
    pub subfolder_by_platform: bool,
    /// 可用变量：{author} {title} {date} {id} {platform}
    pub filename_template: String,
    pub concurrency: usize,
    pub skip_existing: bool,
    pub watch_clipboard: bool,
    pub auto_download: bool,
    pub notify_on_complete: bool,
    pub parse_mode: ParseMode,
    pub remote_endpoint: String,
    /// 旧版：平台 → Cookie 字符串。启动时迁移到加密的 Cookie 存储后清空。
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub cookies: HashMap<String, String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub cookie_updated_at: HashMap<String, i64>,
    /// `system` / `dark` / `light`
    pub theme: String,
    pub close_to_tray: bool,
    pub shortcut: String,
    pub check_update: bool,
    pub disclaimer_accepted: bool,
    /// 启动时自动继续未完成的任务（否则恢复为“已暂停”）
    pub auto_resume: bool,
    /// 网络错误时的自动重试次数
    pub max_retries: u32,
    /// 第一次重试前的等待秒数，之后每次翻倍
    pub retry_delay_secs: u64,
    /// 取消任务时保留已下载的部分
    pub keep_part_on_cancel: bool,
    /// 保存解析时的原始响应，用于排查问题
    pub record_samples: bool,
    pub network: NetworkSettings,
    /// 大文件分段并行下载的段数（1 表示不分段）
    pub segments: usize,
    /// 文件大于该值（MB）时才分段
    pub segment_min_mb: u64,
    /// 全局限速（KB/s），0 表示不限
    pub speed_limit_kbps: u64,
    /// 每个网站同时下载的任务数
    pub per_site_concurrency: usize,
    /// 解析时对同一网站两次请求的最小间隔（毫秒）
    pub site_request_interval_ms: u64,
    /// 临时文件目录，留空表示与保存位置相同
    pub temp_dir: String,
    pub conflict_policy: ConflictPolicy,
    /// 下载前要求保留的剩余磁盘空间（MB）
    pub disk_reserve_mb: u64,
    pub quality_preset: QualityPreset,
    /// 同等清晰度下优先 H.264（兼容性最好，AV1 / VP9 很多设备播不了）
    pub prefer_h264: bool,
    /// 合并音视频时的封装格式：mp4 / mkv
    pub merge_container: String,
    /// 只下载音频时的格式：mp3 / m4a
    pub audio_format: String,
    /// m3u8 分片并发数
    pub hls_concurrency: usize,
    /// 跳过与正片来源明显不同的 m3u8 分片（常见于插播广告）
    pub hls_skip_ads: bool,
    /// 剧集的命名模板；变量：{series} {episode} {season} {index} {title}
    pub series_template: String,
    /// 内置平台解析失败或不支持时使用 yt-dlp
    pub use_ytdlp: bool,
    /// yt-dlp 也不支持时，尝试在网页里查找视频地址
    pub generic_sniffer: bool,
    /// 下载组件时使用的 GitHub 镜像前缀，如 `https://ghfast.top/`
    pub component_mirrors: Vec<String>,
    /// 剪贴板监听是否识别所有网址；关闭时只识别内置平台和 `clipboard_domains`
    pub clipboard_all_sites: bool,
    /// 剪贴板监听额外识别的网站（域名后缀）
    pub clipboard_domains: Vec<String>,
    /// 手机发链接到电脑
    pub phone: PhoneSettings,
    /// 把标题、作者、封面写入视频 / 音频文件
    pub embed_metadata: bool,
    /// 保存作品信息 JSON（标题、简介、原链接等）
    pub write_info_json: bool,
    /// 生成 Jellyfin / Plex 能识别的 NFO 文件
    pub write_nfo: bool,
    /// 全部下载完成后打开下载文件夹
    pub open_folder_on_done: bool,
    /// 每个任务完成后运行的命令（默认关闭）。文件路径通过环境变量 CLEARCLIP_FILE 传入
    pub post_script_enabled: bool,
    pub post_script: String,
    /// 有下载任务时阻止系统休眠
    pub prevent_sleep: bool,
    /// 订阅与追更（默认关闭，首次开启时说明用途和风险）
    pub subscriptions_enabled: bool,
    /// 开机自动启动（在托盘运行，用于订阅检查）
    pub launch_at_login: bool,
    /// 同时录制的直播间数量上限
    pub live_max_recordings: usize,
    /// 自动删除多少天前的直播录像（0 表示不删除）
    pub live_cleanup_days: u32,
    /// 收到的链接（收件箱）
    pub inbox: InboxSettings,
    /// 字幕偏好语言，靠前的优先（`zh` 同时匹配简体、繁体等变体）
    pub subtitle_langs: Vec<String>,
    /// 解析时包含自动生成和自动翻译的字幕（YouTube）
    pub subtitle_auto: bool,
    /// 自动下载时的字幕处理：off 不下载 / file 单独保存字幕文件 / embed 内嵌到视频 / burn 烧录进画面
    pub subtitle_mode: String,
    /// VTT 字幕转成 SRT（播放器和剪辑软件通用）
    pub subtitle_convert: bool,
    /// B站弹幕 XML 转成 ASS（播放器可直接显示滚动弹幕）
    pub danmaku_ass: bool,
    /// 下载完成后用 ffmpeg 检查文件能否正常读取，损坏时自动重新下载一次
    pub verify_downloads: bool,
    /// 媒体库：回收站、整理规则
    pub library: LibrarySettings,
    /// 弹幕转 ASS 时的样式
    pub danmaku: DanmakuStyle,
    /// 默认选中的视频格式预估超过这个大小（MB）时，改选不超过的最高清晰度；0 表示不限
    pub max_size_mb: u64,
    /// 下载失败（网络、文件损坏等）时自动改用更低一档的清晰度重新下载
    pub auto_downgrade: bool,
}

/// 弹幕样式（B站弹幕 XML 转成 ASS 时使用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DanmakuStyle {
    /// 字号（以 1080 高度的画面为准）
    pub font_size: u32,
    /// 不透明度 20–100（%）
    pub opacity: u32,
    /// 滚动弹幕从右到左穿过屏幕的大致秒数，越小越快
    pub scroll_secs: u32,
    /// 弹幕占画面高度的比例（%），避免挡住底部字幕
    pub area: u32,
    /// 字体名；留空使用微软雅黑（没有时播放器会自动换用其他中文字体）
    pub font: String,
}

impl Default for DanmakuStyle {
    fn default() -> Self {
        DanmakuStyle { font_size: 40, opacity: 100, scroll_secs: 7, area: 75, font: String::new() }
    }
}

impl DanmakuStyle {
    pub fn normalize(&mut self) {
        self.font_size = self.font_size.clamp(18, 80);
        self.opacity = self.opacity.clamp(20, 100);
        self.scroll_secs = self.scroll_secs.clamp(3, 20);
        self.area = self.area.clamp(10, 100);
        self.font = self.font.trim().chars().filter(|c| !matches!(c, ',' | '\n' | '\r')).take(60).collect();
    }
}

/// 媒体库设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibrarySettings {
    /// 删除文件时先放进回收站（下载目录里的隐藏文件夹），可以还原
    pub use_trash: bool,
    /// 回收站里的文件保留多少天后自动清空（0 表示不自动清空）
    pub trash_keep_days: u32,
    /// “整理目录”使用的子目录规则；变量：{platform} {author} {year} {month} {day} {date} {kind}
    pub reorganize_template: String,
    /// 下载完成后给没有封面的视频截一张图，并记录时长
    pub probe_new_files: bool,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        LibrarySettings { use_trash: true, trash_keep_days: 30, reorganize_template: "{platform}/{author}".into(), probe_new_files: true }
    }
}

/// 收到的链接的记录与保留。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InboxSettings {
    /// 记录剪贴板识别到的链接（关闭后只记录手机和浏览器扩展发送的链接）
    pub record_clipboard: bool,
    /// 保留多少天（0 表示不按时间清理）
    pub keep_days: u32,
    /// 最多保留多少条（0 表示不限）
    pub max_items: usize,
}

impl Default for InboxSettings {
    fn default() -> Self {
        InboxSettings { record_clipboard: true, keep_days: 30, max_items: 1000 }
    }
}

/// 已配对的手机。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub added_at: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PhoneSettings {
    pub enabled: bool,
    /// 固定端口，让快捷指令里的地址保持不变；0 表示首次开启时随机选择
    pub port: u16,
    /// 访问令牌（写在二维码和快捷指令里）
    pub token: String,
    pub devices: Vec<PairedDevice>,
}

/// 剪贴板默认额外识别的常用网站。
pub const DEFAULT_CLIPBOARD_DOMAINS: &[&str] =
    &["youtube.com", "youtu.be", "pornhub.com", "x.com", "twitter.com", "tiktok.com", "instagram.com", "vimeo.com", "twitch.tv", "acfun.cn", "ixigua.com"];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: String::new(),
            subfolder_by_platform: true,
            filename_template: "{author}_{title}_{date}".into(),
            concurrency: 3,
            skip_existing: true,
            watch_clipboard: true,
            auto_download: false,
            notify_on_complete: true,
            parse_mode: ParseMode::LocalThenRemote,
            remote_endpoint: String::new(),
            cookies: HashMap::new(),
            cookie_updated_at: HashMap::new(),
            theme: "system".into(),
            close_to_tray: true,
            shortcut: "CommandOrControl+Shift+D".into(),
            check_update: true,
            disclaimer_accepted: false,
            auto_resume: false,
            max_retries: 3,
            retry_delay_secs: 2,
            keep_part_on_cancel: false,
            record_samples: false,
            network: NetworkSettings::default(),
            segments: 4,
            segment_min_mb: 8,
            speed_limit_kbps: 0,
            per_site_concurrency: 2,
            site_request_interval_ms: 500,
            temp_dir: String::new(),
            conflict_policy: ConflictPolicy::Rename,
            disk_reserve_mb: 200,
            quality_preset: QualityPreset::Best,
            prefer_h264: true,
            merge_container: "mp4".into(),
            audio_format: "mp3".into(),
            hls_concurrency: 6,
            hls_skip_ads: false,
            series_template: "{series}/第{episode}集".into(),
            use_ytdlp: true,
            generic_sniffer: true,
            component_mirrors: vec![],
            clipboard_all_sites: false,
            clipboard_domains: DEFAULT_CLIPBOARD_DOMAINS.iter().map(|d| d.to_string()).collect(),
            phone: PhoneSettings::default(),
            embed_metadata: true,
            write_info_json: false,
            write_nfo: false,
            open_folder_on_done: false,
            post_script_enabled: false,
            post_script: String::new(),
            prevent_sleep: true,
            subscriptions_enabled: false,
            launch_at_login: false,
            live_max_recordings: 3,
            live_cleanup_days: 0,
            inbox: InboxSettings::default(),
            subtitle_langs: vec!["zh".into(), "en".into()],
            subtitle_auto: true,
            subtitle_mode: "off".into(),
            subtitle_convert: true,
            danmaku_ass: true,
            verify_downloads: true,
            library: LibrarySettings::default(),
            danmaku: DanmakuStyle::default(),
            max_size_mb: 0,
            auto_downgrade: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path, default_download_dir: &Path) -> Settings {
        let mut s: Settings = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        if s.download_dir.trim().is_empty() {
            s.download_dir = default_download_dir.to_string_lossy().into_owned();
        }
        s.normalize();
        s
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.concurrency = self.concurrency.clamp(1, 8);
        self.max_retries = self.max_retries.min(10);
        self.retry_delay_secs = self.retry_delay_secs.clamp(1, 60);
        self.segments = self.segments.clamp(1, 16);
        self.per_site_concurrency = self.per_site_concurrency.clamp(1, 8);
        self.site_request_interval_ms = self.site_request_interval_ms.min(10_000);
        self.temp_dir = self.temp_dir.trim().to_string();
        for r in &mut self.network.rules {
            r.pattern = r.pattern.trim().trim_start_matches('.').to_ascii_lowercase();
        }
        self.network.rules.retain(|r| !r.pattern.is_empty());
        self.hls_concurrency = self.hls_concurrency.clamp(1, 16);
        self.inbox.keep_days = self.inbox.keep_days.min(3650);
        if self.inbox.max_items > 0 {
            self.inbox.max_items = self.inbox.max_items.clamp(50, 100_000);
        }
        self.subtitle_langs = self.subtitle_langs.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(12).collect();
        if !matches!(self.subtitle_mode.as_str(), "off" | "file" | "embed" | "burn") {
            self.subtitle_mode = "off".into();
        }
        if !matches!(self.merge_container.as_str(), "mp4" | "mkv") {
            self.merge_container = "mp4".into();
        }
        if !matches!(self.audio_format.as_str(), "mp3" | "m4a" | "opus" | "flac") {
            self.audio_format = "mp3".into();
        }
        self.series_template = self.series_template.trim().to_string();
        self.clipboard_domains =
            self.clipboard_domains.iter().map(|d| d.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|d| d.contains('.')).collect();
        self.phone.devices.truncate(50);
        self.component_mirrors = self
            .component_mirrors
            .iter()
            .map(|m| m.trim().to_string())
            .filter(|m| m.starts_with("https://"))
            .map(|m| if m.ends_with('/') { m } else { format!("{m}/") })
            .collect();
        if self.filename_template.trim().is_empty() {
            self.filename_template = Settings::default().filename_template;
        }
        self.remote_endpoint = self.remote_endpoint.trim().to_string();
        self.danmaku.normalize();
        self.max_size_mb = self.max_size_mb.min(1_000_000);
        self.library.trash_keep_days = self.library.trash_keep_days.min(3650);
        self.library.reorganize_template = self.library.reorganize_template.trim().to_string();
        if self.library.reorganize_template.is_empty() {
            self.library.reorganize_template = LibrarySettings::default().reorganize_template;
        }
    }

    pub fn download_root(&self) -> PathBuf {
        PathBuf::from(&self.download_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"concurrency": 5}"#).unwrap();
        assert_eq!(s.concurrency, 5);
        assert!(s.watch_clipboard);
        assert_eq!(s.parse_mode, ParseMode::LocalThenRemote);
    }

    #[test]
    fn route_matching_prefers_longest_suffix() {
        let mut n = NetworkSettings::default();
        assert_eq!(n.route_for_host("www.youtube.com"), Route::System);
        assert_eq!(n.route_for_host("upos-sz-mirrorcos.bilivideo.com"), Route::Direct);
        assert_eq!(n.route_for_host("example.org"), Route::System);
        n.rules.push(RouteRule { pattern: "music.youtube.com".into(), route: Route::Proxy("p1".into()) });
        assert_eq!(n.route_for_host("music.youtube.com"), Route::Proxy("p1".into()));
        assert_eq!(n.route_for_host("notyoutube.com"), Route::System, "suffix must match on label boundary");
    }

    #[test]
    fn route_serde_shape() {
        assert_eq!(serde_json::to_string(&Route::Direct).unwrap(), r#"{"kind":"direct"}"#);
        assert_eq!(serde_json::to_string(&Route::Proxy("a".into())).unwrap(), r#"{"kind":"proxy","id":"a"}"#);
    }

    #[test]
    fn normalize_clamps_concurrency() {
        let mut s = Settings { concurrency: 99, ..Settings::default() };
        s.normalize();
        assert_eq!(s.concurrency, 8);
    }
}
