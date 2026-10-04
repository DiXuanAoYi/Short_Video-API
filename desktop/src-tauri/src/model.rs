use serde::{Deserialize, Serialize};

/// 解析结果里的内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Images,
}

/// 单个可下载资源的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Video,
    Image,
    Audio,
    Cover,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    /// 在同一条作品内唯一，例如 `video`、`image-3`、`music`、`cover`。
    pub id: String,
    pub kind: AssetKind,
    pub url: String,
    pub label: String,
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 图集中的序号（从 0 开始），其他资源为 `None`。
    pub index: Option<usize>,
}

impl Asset {
    pub fn video(url: String, width: Option<u32>, height: Option<u32>) -> Self {
        Asset { id: "video".into(), kind: AssetKind::Video, url, label: "视频 MP4".into(), ext: "mp4".into(), width, height, index: None }
    }

    pub fn image(index: usize, url: String, width: Option<u32>, height: Option<u32>) -> Self {
        let ext = guess_image_ext(&url);
        Asset { id: format!("image-{index}"), kind: AssetKind::Image, url, label: format!("图片 {}", index + 1), ext, width, height, index: Some(index) }
    }

    pub fn audio(url: String) -> Self {
        let ext = if url.contains(".m4a") { "m4a" } else { "mp3" };
        Asset { id: "music".into(), kind: AssetKind::Audio, url, label: "背景音乐".into(), ext: ext.into(), width: None, height: None, index: None }
    }

    pub fn cover(url: String) -> Self {
        let ext = guess_image_ext(&url);
        Asset { id: "cover".into(), kind: AssetKind::Cover, url, label: "封面图".into(), ext, width: None, height: None, index: None }
    }
}

fn guess_image_ext(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    if lower.contains("format/png") {
        "png".into()
    } else if lower.contains("format/webp") || path.contains(".webp") {
        "webp".into()
    } else if path.contains(".png") {
        "png".into()
    } else if path.contains(".gif") {
        "gif".into()
    } else if path.contains(".heic") {
        "heic".into()
    } else {
        "jpg".into()
    }
}

/// 一条作品的统一解析结果，所有平台都转换成这个结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    /// 平台标识：`douyin` / `kuaishou` / `remote`。
    pub platform: String,
    pub platform_name: String,
    /// 平台内作品 ID。
    pub id: String,
    /// 用户粘贴的原始链接，用于直链过期后重新解析。
    pub source_url: String,
    pub title: String,
    pub author: String,
    pub cover: Option<String>,
    pub duration_ms: Option<u64>,
    pub kind: MediaKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 发布时间，Unix 秒。
    pub published_at: Option<i64>,
    pub assets: Vec<Asset>,
}

impl MediaInfo {
    /// 默认勾选的资源：视频作品选视频，图集选全部图片。
    pub fn default_asset_ids(&self) -> Vec<String> {
        self.assets
            .iter()
            .filter(|a| match self.kind {
                MediaKind::Video => a.kind == AssetKind::Video,
                MediaKind::Images => a.kind == AssetKind::Image,
            })
            .map(|a| a.id.clone())
            .collect()
    }
}

pub use crate::error::{AppError, AppResult, ErrorKind};
