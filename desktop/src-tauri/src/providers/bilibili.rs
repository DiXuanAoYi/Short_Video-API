//! B站：从链接取 BV / av 号，调用 `x/web-interface/view` 取稿件信息，
//! 再用 `x/player/playurl`（html5 平台，音视频合一的 MP4）取播放地址。
//!
//! 未登录时清晰度通常为 480P / 720P；在设置中登录 B站后会请求 1080P。
//! 多 P 稿件按链接里的 `?p=` 选择分 P，默认第 1 P。

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use url::Url;

use super::{str_at, u32_at, u64_at, Ctx, Provider, DESKTOP_UA};
use crate::model::{AppError, AppResult, Asset, MediaInfo, MediaKind};

pub struct Bilibili;

static BV_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(BV[0-9A-Za-z]{10})").unwrap());
static AV_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)/av(\d+)").unwrap());
static PAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?&]p=(\d+)").unwrap());

const REFERER: &str = "https://www.bilibili.com/";

#[derive(Debug, PartialEq)]
pub enum VideoId {
    Bv(String),
    Av(u64),
}

impl VideoId {
    fn query(&self) -> String {
        match self {
            VideoId::Bv(b) => format!("bvid={b}"),
            VideoId::Av(a) => format!("aid={a}"),
        }
    }
}

#[async_trait]
impl Provider for Bilibili {
    fn id(&self) -> &'static str {
        "bilibili"
    }

    fn name(&self) -> &'static str {
        "B站"
    }

    fn matches(&self, url: &Url) -> bool {
        url.host_str().is_some_and(|h| h == "b23.tv" || h.ends_with(".b23.tv") || h == "bilibili.com" || h.ends_with(".bilibili.com") || h == "bili2233.cn")
    }

    fn referer(&self) -> &'static str {
        REFERER
    }

    fn login_url(&self) -> &'static str {
        "https://www.bilibili.com/"
    }

    async fn account_status(&self, ctx: &Ctx<'_>) -> AppResult<Option<super::AccountStatus>> {
        let api = "https://api.bilibili.com/x/web-interface/nav";
        let v = get_json(ctx, api, ctx.cookie(api).as_deref()).await?;
        Ok(Some(parse_nav(&v)))
    }

    async fn resolve(&self, ctx: &Ctx<'_>, url: &str) -> AppResult<MediaInfo> {
        let cookie = ctx.cookie("https://api.bilibili.com/");
        let (vid, page) = match video_id_from_url(url) {
            Some(v) => (v, page_from_url(url)),
            None => {
                // b23.tv 短链
                let resp = ctx.get(url).header("User-Agent", DESKTOP_UA).send().await?;
                let final_url = resp.url().to_string();
                let v = video_id_from_url(&final_url).ok_or_else(|| AppError::msg("没能从链接里识别出 B站视频号（BV/av），目前只支持视频稿件。"))?;
                (v, page_from_url(&final_url))
            }
        };

        let view = get_json(ctx, &format!("https://api.bilibili.com/x/web-interface/view?{}", vid.query()), cookie.as_deref()).await?;
        let data = api_data(&view)?;
        let pages = data.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
        let index = page.saturating_sub(1).min(pages.len().saturating_sub(1));
        let cid = pages
            .get(index)
            .and_then(|p| u64_at(p, "/cid"))
            .or_else(|| u64_at(data, "/cid"))
            .ok_or_else(|| AppError::parser("B站稿件缺少 cid，无法获取播放地址。"))?;

        let qn = if cookie.is_some() { 80 } else { 64 };
        let play_api =
            format!("https://api.bilibili.com/x/player/playurl?{}&cid={cid}&qn={qn}&fnval=1&fnver=0&fourk=0&platform=html5&high_quality=1", vid.query());
        let play = get_json(ctx, &play_api, cookie.as_deref()).await?;
        let play_data = api_data(&play)?;
        parse_view(data, play_data, index, url)
    }
}

async fn get_json(ctx: &Ctx<'_>, api: &str, cookie: Option<&str>) -> AppResult<Value> {
    let mut req = ctx.get(api).header("User-Agent", DESKTOP_UA).header("Referer", REFERER);
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(AppError::from_status(resp.status().as_u16(), "B站接口").context_suffix("，可能触发了风控，请稍后再试或在设置中登录 B站。"));
    }
    let text = resp.text().await?;
    ctx.record("bilibili", "api", api, &text);
    Ok(serde_json::from_str(&text)?)
}

/// B站接口统一格式 `{code, message, data}`，code 非 0 表示失败。
pub fn api_data(v: &Value) -> AppResult<&Value> {
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = str_at(v, "/message").unwrap_or("未知错误");
        let text = format!("B站：{msg}（{code}）");
        return Err(match code {
            -101 | -403 | -10403 => AppError::need_login(text),
            -404 | 62002 | 62004 => AppError::not_found(text),
            -352 | -412 | -509 => AppError::new(crate::error::ErrorKind::RateLimited, text),
            _ => AppError::classify(text),
        });
    }
    v.get("data").filter(|d| d.is_object()).ok_or_else(|| AppError::msg("B站接口没有返回数据。"))
}

/// 解析 `x/web-interface/nav`：未登录时 code 为 -101。
pub fn parse_nav(v: &Value) -> super::AccountStatus {
    let data = v.get("data");
    let logged_in = data.and_then(|d| d.get("isLogin")).and_then(Value::as_bool).unwrap_or(false);
    let vip = data.filter(|d| u64_at(d, "/vipStatus") == Some(1)).map(|d| str_at(d, "/vip_label/text").unwrap_or("大会员").to_string());
    super::AccountStatus { logged_in, user_name: data.and_then(|d| str_at(d, "/uname")).map(String::from), vip }
}

pub fn video_id_from_url(url: &str) -> Option<VideoId> {
    if let Some(c) = BV_RE.captures(url) {
        return Some(VideoId::Bv(c[1].to_string()));
    }
    AV_RE.captures(url).and_then(|c| c[1].parse().ok()).map(VideoId::Av)
}

fn page_from_url(url: &str) -> usize {
    PAGE_RE.captures(url).and_then(|c| c[1].parse().ok()).filter(|p| *p > 0).unwrap_or(1)
}

/// 合并稿件信息和播放地址。`index` 是分 P 下标（从 0 开始）。
pub fn parse_view(data: &Value, play: &Value, index: usize, source_url: &str) -> AppResult<MediaInfo> {
    let bvid = str_at(data, "/bvid").map(String::from).or_else(|| u64_at(data, "/aid").map(|a| format!("av{a}"))).unwrap_or_default();
    let pages = data.get("pages").and_then(Value::as_array);
    let page = pages.and_then(|p| p.get(index));
    let multi = pages.is_some_and(|p| p.len() > 1);

    let mut title = str_at(data, "/title").unwrap_or("B站视频").trim().to_string();
    if multi {
        if let Some(part) = page.and_then(|p| str_at(p, "/part")) {
            title = format!("{title} P{} {part}", index + 1);
        }
    }
    let author = str_at(data, "/owner/name").unwrap_or("未知UP主").to_string();
    let cover = str_at(data, "/pic").map(|u| u.replace("http://", "https://"));
    let duration_ms = page.and_then(|p| u64_at(p, "/duration")).or_else(|| u64_at(data, "/duration")).map(|s| s * 1000);
    let dim = page.and_then(|p| p.get("dimension")).or_else(|| data.get("dimension"));
    let (mut width, mut height) = (dim.and_then(|d| u32_at(d, "/width")), dim.and_then(|d| u32_at(d, "/height")));
    // rotate=1 表示竖屏，宽高需要交换
    if dim.and_then(|d| u64_at(d, "/rotate")) == Some(1) {
        std::mem::swap(&mut width, &mut height);
    }

    let video = str_at(play, "/durl/0/url")
        .or_else(|| str_at(play, "/durl/0/backup_url/0"))
        .ok_or_else(|| AppError::need_login("B站没有返回可直接下载的 MP4 地址。该视频可能需要登录、大会员或仅支持分段播放。"))?;

    let mut assets = vec![Asset::video(video.to_string(), width, height)];
    if let Some(c) = &cover {
        assets.push(Asset::cover(c.clone()));
    }
    let id = if multi { format!("{bvid}-p{}", index + 1) } else { bvid };

    Ok(MediaInfo {
        platform: "bilibili".into(),
        platform_name: "B站".into(),
        id,
        source_url: source_url.to_string(),
        title,
        author,
        cover,
        duration_ms,
        kind: MediaKind::Video,
        width,
        height,
        published_at: u64_at(data, "/pubdate").map(|t| t as i64),
        assets,
        entries: vec![],
        series: None,
        extractor: Some("native".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = include_str!("../../tests/fixtures/bili_view.json");
    const PLAY: &str = include_str!("../../tests/fixtures/bili_playurl.json");

    #[test]
    fn video_ids() {
        assert_eq!(video_id_from_url("https://www.bilibili.com/video/BV1xx411c7mD/?spm=1"), Some(VideoId::Bv("BV1xx411c7mD".into())));
        assert_eq!(video_id_from_url("https://m.bilibili.com/video/av170001"), Some(VideoId::Av(170001)));
        assert_eq!(video_id_from_url("https://b23.tv/AbC123"), None);
        assert_eq!(page_from_url("https://www.bilibili.com/video/BV1xx411c7mD?p=3"), 3);
        assert_eq!(page_from_url("https://www.bilibili.com/video/BV1xx411c7mD"), 1);
    }

    #[test]
    fn parses_second_page_of_multi_part_video() {
        let view: Value = serde_json::from_str(VIEW).unwrap();
        let play: Value = serde_json::from_str(PLAY).unwrap();
        let info = parse_view(api_data(&view).unwrap(), api_data(&play).unwrap(), 1, "x").unwrap();
        assert_eq!(info.id, "BV1xx411c7mD-p2");
        assert_eq!(info.title, "Rust 入门教程 P2 第二集：所有权");
        assert_eq!(info.author, "编程老王");
        assert_eq!(info.duration_ms, Some(600_000));
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        assert!(info.assets[0].url.starts_with("https://upos-sz-mirrorcos.bilivideo.com/"));
        assert_eq!(info.cover.as_deref(), Some("https://i0.hdslb.com/bfs/archive/cover.jpg"));
    }

    #[test]
    fn nav_status() {
        let v: Value =
            serde_json::from_str(r#"{"code":0,"data":{"isLogin":true,"uname":"编程老王","vipStatus":1,"vip_label":{"text":"年度大会员"}}}"#).unwrap();
        let s = parse_nav(&v);
        assert!(s.logged_in);
        assert_eq!(s.user_name.as_deref(), Some("编程老王"));
        assert_eq!(s.vip.as_deref(), Some("年度大会员"));
        let v: Value = serde_json::from_str(r#"{"code":-101,"message":"账号未登录","data":{"isLogin":false}}"#).unwrap();
        assert!(!parse_nav(&v).logged_in);
    }

    #[test]
    fn api_error_surfaces_message() {
        let v: Value = serde_json::from_str(r#"{"code":-404,"message":"啥都木有","data":null}"#).unwrap();
        let err = api_data(&v).unwrap_err().to_string();
        assert!(err.contains("啥都木有") && err.contains("-404"), "{err}");
    }

    #[test]
    fn missing_durl_is_clear_error() {
        let view: Value = serde_json::from_str(VIEW).unwrap();
        let play: Value = serde_json::from_str(r#"{"quality":64}"#).unwrap();
        assert!(parse_view(api_data(&view).unwrap(), &play, 0, "x").unwrap_err().to_string().contains("MP4"));
    }
}
