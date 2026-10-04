//! 各平台解析器。每个平台实现 [`Provider`]，新增平台时只需加一个文件并在 [`all`] 里注册。

pub mod bilibili;
pub mod douyin;
pub mod kuaishou;
pub mod remote;
pub mod weibo;
pub mod xiaohongshu;

use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde::Serialize;
use url::Url;

use crate::model::{AppError, AppResult, MediaInfo};
use crate::settings::{ParseMode, Settings};

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";
pub const MOBILE_UA: &str =
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";

/// 解析时共享的上下文。
pub struct Ctx<'a> {
    pub client: &'a reqwest::Client,
    pub settings: &'a Settings,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// 判断链接是否属于本平台。
    fn matches(&self, url: &Url) -> bool;
    /// 下载该平台资源时使用的 Referer。
    fn referer(&self) -> &'static str;
    /// 内置登录窗口打开的地址，也用于读取登录后的 Cookie。
    fn login_url(&self) -> &'static str;
    async fn resolve(&self, ctx: &Ctx<'_>, url: &str) -> AppResult<MediaInfo>;
}

pub fn all() -> &'static [&'static dyn Provider] {
    static ALL: [&dyn Provider; 5] = [&douyin::Douyin, &kuaishou::Kuaishou, &xiaohongshu::Xiaohongshu, &bilibili::Bilibili, &weibo::Weibo];
    &ALL
}

pub fn by_id(id: &str) -> Option<&'static dyn Provider> {
    all().iter().copied().find(|p| p.id() == id)
}

pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(MOBILE_UA)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .gzip(true)
        .build()
        .expect("failed to build http client")
}

static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[A-Za-z0-9\-._~:/?#\[\]@!$&'*+,;=%]+"#).unwrap());

/// 从整段分享文案里提取所有链接（去掉尾部标点）。
pub fn extract_urls(text: &str) -> Vec<String> {
    URL_RE.find_iter(text).map(|m| m.as_str().trim_end_matches(|c: char| ".,;:!?'\")]".contains(c)).to_string()).filter(|u| Url::parse(u).is_ok()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedLink {
    pub url: String,
    pub platform: String,
    pub platform_name: String,
}

/// 找出文本里所有受支持平台的链接，按出现顺序去重。
pub fn detect_links(text: &str) -> Vec<DetectedLink> {
    let mut out: Vec<DetectedLink> = Vec::new();
    for raw in extract_urls(text) {
        let Ok(parsed) = Url::parse(&raw) else { continue };
        if let Some(p) = all().iter().find(|p| p.matches(&parsed)) {
            if !out.iter().any(|d| d.url == raw) {
                out.push(DetectedLink { url: raw, platform: p.id().into(), platform_name: p.name().into() });
            }
        }
    }
    out
}

/// 解析一段文本中的第一个链接，按设置决定本地 / 远程策略。
pub async fn resolve_text(client: &reqwest::Client, settings: &Settings, text: &str) -> AppResult<MediaInfo> {
    let urls = extract_urls(text);
    let Some(first) = urls.first() else {
        return Err(AppError::msg("没有找到链接。请粘贴分享文案或链接。"));
    };
    // 优先选受支持平台的链接，避免文案中混有其他网址。
    let url = urls.iter().find(|u| Url::parse(u).map(|p| all().iter().any(|pr| pr.matches(&p))).unwrap_or(false)).unwrap_or(first).clone();
    resolve_url(client, settings, &url).await
}

/// 旧版 PHP 接口（远程 API）只支持这些平台。
pub const REMOTE_PLATFORMS: &[&str] = &["douyin", "kuaishou"];

/// 远程 API 不支持的平台始终走本地解析；无法识别平台的链接交给远程接口判断。
pub fn remote_allowed(provider: Option<&dyn Provider>) -> bool {
    provider.map_or(true, |p| REMOTE_PLATFORMS.contains(&p.id()))
}

pub async fn resolve_url(client: &reqwest::Client, settings: &Settings, url: &str) -> AppResult<MediaInfo> {
    let ctx = Ctx { client, settings };
    let has_remote = !settings.remote_endpoint.is_empty();
    let parsed = Url::parse(url).map_err(|_| AppError::msg("链接格式不正确。"))?;
    let provider = all().iter().find(|p| p.matches(&parsed));
    let remote_ok = remote_allowed(provider.copied());

    if settings.parse_mode == ParseMode::Remote && remote_ok {
        if !has_remote {
            return Err(AppError::msg("当前为远程解析模式，但没有填写远程 API 地址。请在设置中填写，或改为本地解析。"));
        }
        return remote::resolve(&ctx, url).await;
    }

    let local = match provider {
        Some(p) => p.resolve(&ctx, url).await,
        None => Err(AppError::msg("暂不支持这个平台。目前支持抖音、快手、小红书、B站、微博。")),
    };

    match local {
        Ok(info) => Ok(info),
        Err(e) if settings.parse_mode != ParseMode::Local && has_remote && remote_ok && provider.is_some() => {
            remote::resolve(&ctx, url).await.map_err(|re| AppError::msg(format!("本地解析失败：{e}；远程解析也失败：{re}")))
        }
        Err(e) => Err(e),
    }
}

/// 下载某个平台资源时使用的 Referer。
pub fn referer_for(platform: &str) -> &'static str {
    by_id(platform).map(|p| p.referer()).unwrap_or("")
}

// ---------- 供各平台复用的小工具 ----------

/// 从 HTML 中截取 `marker = {...}` 形式的 JSON 对象（按括号配对，忽略字符串内的括号）。
pub(crate) fn extract_json_after(html: &str, marker: &str) -> Option<serde_json::Value> {
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let brace = rest.find('{')?;
    let body = &rest[brace..];
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let raw = &body[..=i];
                    // 有的页面会把 undefined 写进对象里
                    return serde_json::from_str(raw).ok().or_else(|| serde_json::from_str(&raw.replace(":undefined", ":null")).ok());
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn str_at<'a>(v: &'a serde_json::Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(|x| x.as_str()).filter(|s| !s.is_empty())
}

pub(crate) fn u64_at(v: &serde_json::Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)).or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

pub(crate) fn u32_at(v: &serde_json::Value, ptr: &str) -> Option<u32> {
    u64_at(v, ptr).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_from_douyin_share_text() {
        let text = "7.43 复制打开抖音，看看【山野厨房的作品】秋天第一锅板栗焖鸡 https://v.douyin.com/ehHpu7V/ a@B.Ok 02/18 :0pm";
        assert_eq!(extract_urls(text), vec!["https://v.douyin.com/ehHpu7V/"]);
    }

    #[test]
    fn strips_trailing_punctuation() {
        assert_eq!(extract_urls("看这个：https://v.kuaishou.com/abc12, 不错"), vec!["https://v.kuaishou.com/abc12"]);
    }

    #[test]
    fn detects_supported_platforms_only() {
        let text = "https://example.com/x https://v.douyin.com/ehHpu7V/ https://www.kuaishou.com/short-video/3xabc https://v.douyin.com/ehHpu7V/";
        let links = detect_links(text);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].platform, "douyin");
        assert_eq!(links[1].platform, "kuaishou");
    }

    #[test]
    fn extract_json_handles_braces_in_strings() {
        let html = r#"<script>window.X = {"a":"}{","b":{"c":1}};</script>"#;
        let v = extract_json_after(html, "window.X").unwrap();
        assert_eq!(v["a"], "}{");
        assert_eq!(v["b"]["c"], 1);
    }

    #[tokio::test]
    async fn remote_mode_without_endpoint_is_an_error() {
        let s = Settings { parse_mode: ParseMode::Remote, ..Settings::default() };
        let err = resolve_text(&build_client(), &s, "https://v.douyin.com/x/").await.unwrap_err();
        assert!(err.to_string().contains("远程 API"));
    }

    #[test]
    fn remote_api_only_for_legacy_platforms() {
        assert!(remote_allowed(by_id("douyin")));
        assert!(remote_allowed(by_id("kuaishou")));
        assert!(!remote_allowed(by_id("bilibili")));
        assert!(!remote_allowed(by_id("xiaohongshu")));
        assert!(!remote_allowed(by_id("weibo")));
        assert!(remote_allowed(None));
    }

    #[tokio::test]
    async fn text_without_url_is_an_error() {
        let err = resolve_text(&build_client(), &Settings::default(), "没有链接").await.unwrap_err();
        assert!(err.to_string().contains("没有找到链接"));
    }
}
