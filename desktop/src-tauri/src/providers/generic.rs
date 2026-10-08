//! 通用网页嗅探：内置解析器和 yt-dlp 都不支持时，从网页里找视频地址
//! （直链、og:video、`<video>` / `<source>` 标签、脚本里的 mp4 / m3u8 地址）。

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use super::{Ctx, DESKTOP_UA};
use crate::model::{AppError, AppResult, Asset, AssetKind, MediaInfo, MediaKind, Protocol};

static META_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<meta\s[^>]*>"#).unwrap());
static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)([a-z:_-]+)\s*=\s*("([^"]*)"|'([^']*)')"#).unwrap());
static TAG_SRC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<(?:video|source)\s[^>]*?src\s*=\s*["']([^"']+)["']"#).unwrap());
static MEDIA_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)https?:(?:\\?/){2}(?:[^\s"'<>()\\]|\\/)+?\.(?:mp4|m3u8|webm|m4v|mov)(?:\?[^\s"'<>\\]*)?"#).unwrap());
static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).unwrap());

fn attrs(tag: &str) -> Vec<(String, String)> {
    ATTR_RE.captures_iter(tag).map(|c| (c[1].to_ascii_lowercase(), c.get(3).or(c.get(4)).map(|m| m.as_str()).unwrap_or("").to_string())).collect()
}

/// `<meta property|name="key" content="...">` 的内容。
fn meta(html: &str, keys: &[&str]) -> Vec<String> {
    let mut out = vec![];
    for m in META_RE.find_iter(html) {
        let a = attrs(m.as_str());
        let key = a.iter().find(|(k, _)| k == "property" || k == "name").map(|(_, v)| v.to_ascii_lowercase());
        if key.is_some_and(|k| keys.contains(&k.as_str())) {
            if let Some((_, v)) = a.iter().find(|(k, _)| k == "content") {
                out.push(html_unescape(v));
            }
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

fn ext_of(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    ["mp4", "m3u8", "webm", "m4v", "mov", "mp3", "m4a", "jpg", "jpeg", "png", "gif", "webp"].into_iter().find(|e| path.ends_with(&format!(".{e}")))
}

/// 从网页中找出候选视频地址（按出现顺序去重，已转为绝对地址）。
pub fn sniff(html: &str, base: &Url) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    let mut push = |raw: &str| {
        let raw = html_unescape(&raw.replace("\\/", "/").replace("\\u002F", "/"));
        if raw.starts_with("blob:") || raw.starts_with("data:") {
            return;
        }
        if let Ok(u) = base.join(raw.trim()) {
            let s = u.to_string();
            if matches!(u.scheme(), "http" | "https") && !found.contains(&s) {
                found.push(s);
            }
        }
    };
    for v in meta(html, &["og:video:secure_url", "og:video:url", "og:video", "twitter:player:stream"]) {
        push(&v);
    }
    for c in TAG_SRC_RE.captures_iter(html) {
        push(&c[1]);
    }
    for m in MEDIA_URL_RE.find_iter(html) {
        push(m.as_str());
    }
    // 只保留真正的媒体地址（og:video 有时是播放器页面）
    found.retain(|u| matches!(ext_of(u), Some("mp4" | "m3u8" | "webm" | "m4v" | "mov")));
    found
}

fn asset_for(i: usize, url: &str, referer: &str) -> Asset {
    let ext = ext_of(url).unwrap_or("mp4");
    let hls = ext == "m3u8";
    let mut a = Asset::base(format!("video-{i}"), AssetKind::Video, url.to_string(), "", if hls { "mp4" } else { ext });
    a.protocol = if hls { Protocol::Hls } else { Protocol::Http };
    a.headers = vec![("Referer".into(), referer.to_string()), ("User-Agent".into(), DESKTOP_UA.into())];
    let host = Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
    a.label = format!("{} {}（{host}）", if hls { "视频流" } else { "视频" }, if hls { "M3U8".to_string() } else { ext.to_uppercase() });
    a
}

pub async fn resolve(ctx: &Ctx<'_>, url: &str) -> AppResult<MediaInfo> {
    let base = Url::parse(url).map_err(|_| AppError::invalid("链接格式不正确。"))?;
    let host = base.host_str().unwrap_or("").to_string();
    let site = crate::cookies::registrable_domain(&host);
    let mut info = MediaInfo {
        platform: site.clone(),
        platform_name: if site.is_empty() { "网页".into() } else { site.clone() },
        id: String::new(),
        source_url: url.to_string(),
        title: String::new(),
        author: String::new(),
        cover: None,
        duration_ms: None,
        kind: MediaKind::Video,
        width: None,
        height: None,
        published_at: None,
        assets: vec![],
        entries: vec![],
        series: None,
        chapters: vec![],
        extractor: Some("generic".into()),
    };
    let name_from_path = || base.path_segments().and_then(|mut s| s.rfind(|x| !x.is_empty())).map(|s| s.to_string()).unwrap_or_else(|| host.clone());

    // 链接本身就是媒体文件
    if let Some(e @ ("mp4" | "m3u8" | "webm" | "m4v" | "mov")) = ext_of(url) {
        info.title = name_from_path().trim_end_matches(&format!(".{e}")).to_string();
        info.id = info.title.clone();
        info.assets.push(asset_for(0, url, &format!("{}://{host}/", base.scheme())));
        return Ok(info);
    }

    let mut req = ctx.get(url).header("User-Agent", DESKTOP_UA);
    if let Some(c) = ctx.cookie(url) {
        req = req.header("Cookie", c);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::from_status(status.as_u16(), "网页打不开"));
    }
    let ctype = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let final_url = resp.url().clone();
    if ctype.starts_with("video/") || ctype.contains("mpegurl") {
        info.title = name_from_path();
        info.id = info.title.clone();
        let mut a = asset_for(0, final_url.as_str(), url);
        if ctype.contains("mpegurl") {
            a.protocol = Protocol::Hls;
            a.ext = "mp4".into();
        }
        info.assets.push(a);
        return Ok(info);
    }
    if !ctype.is_empty() && !ctype.contains("html") && !ctype.contains("text") {
        return Err(AppError::unsupported("这个链接不是网页也不是视频文件。"));
    }
    let html = resp.text().await?;
    ctx.record("generic", "page", url, &html);

    info.title = meta(&html, &["og:title", "twitter:title"])
        .into_iter()
        .next()
        .or_else(|| TITLE_RE.captures(&html).map(|c| html_unescape(c[1].trim())))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(name_from_path);
    info.id = final_url.path().trim_matches('/').replace('/', "_");
    info.cover =
        meta(&html, &["og:image", "og:image:secure_url", "twitter:image"]).into_iter().next().and_then(|c| final_url.join(&c).ok()).map(|u| u.to_string());

    let urls = sniff(&html, &final_url);
    if urls.is_empty() {
        return Err(AppError::unsupported("暂不支持这个网站：网页里没有找到视频地址。可能需要登录，或视频由脚本动态加载。"));
    }
    info.assets = urls.iter().take(12).enumerate().map(|(i, u)| asset_for(i, u, final_url.as_str())).collect();
    if let Some(c) = info.cover.clone() {
        info.assets.push(Asset::cover(c));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_patterns() {
        let html = r#"<html><head><title>Clip &amp; more</title>
            <meta property="og:video" content="https://cdn.example.com/og.mp4?sig=1&amp;t=2">
            <meta property="og:video:url" content="https://www.example.com/player">
            </head><body>
            <video controls src="/media/inline.webm"></video>
            <video><source src='rel/stream.m3u8' type="application/x-mpegURL"></video>
            <video src="blob:https://x/1"></video>
            <script>var p = {"url":"https:\/\/cdn2.example.com\/a\/b.mp4","dup":"https://cdn.example.com/og.mp4?sig=1&t=2"};</script>
            </body></html>"#;
        let base = Url::parse("https://www.example.com/watch/1").unwrap();
        let urls = sniff(html, &base);
        assert_eq!(
            urls,
            vec![
                "https://cdn.example.com/og.mp4?sig=1&t=2",
                "https://www.example.com/media/inline.webm",
                "https://www.example.com/watch/rel/stream.m3u8",
                "https://cdn2.example.com/a/b.mp4",
            ]
        );
        let a = asset_for(2, &urls[2], base.as_str());
        assert_eq!(a.protocol, Protocol::Hls);
        assert_eq!(a.ext, "mp4");
    }

    #[test]
    fn meta_reads_name_and_property() {
        let html = r#"<meta name="twitter:title" content="T1"><meta content="I" property="og:image">"#;
        assert_eq!(meta(html, &["twitter:title"]), vec!["T1"]);
        assert_eq!(meta(html, &["og:image"]), vec!["I"]);
    }
}
