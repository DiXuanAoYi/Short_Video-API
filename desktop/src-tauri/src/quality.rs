//! 清晰度预设：解析后按设置给视频格式排序（第一个即默认选中），自动下载时决定下载哪些资源。

use crate::download::PostOptions;
use crate::model::{Asset, AssetKind, MediaInfo, MediaKind};
use crate::settings::{QualityPreset, Settings};

fn is_h264(a: &Asset) -> bool {
    a.vcodec.as_deref().is_some_and(|c| {
        let c = c.to_ascii_lowercase();
        c.starts_with("avc") || c.starts_with("h264")
    })
}

/// 视频格式的排序键（越大越优先）。
fn rank(a: &Asset, preset: QualityPreset, prefer_h264: bool) -> (bool, i64, bool, i64) {
    let side = a.short_side().unwrap_or(0) as i64;
    let h264 = prefer_h264 && is_h264(a);
    let bitrate = a.bitrate.unwrap_or(0) as i64;
    match preset {
        // 超过 1080P 的排到后面
        QualityPreset::Max1080 => (side <= 1080, side, h264, bitrate),
        // 不超过 720P 里分辨率最低（但不低于 360P）、码率最低的
        QualityPreset::Small => (side <= 720 && (side >= 360 || side == 0), -side, h264, -bitrate),
        QualityPreset::Best | QualityPreset::Audio => (true, side, h264, bitrate),
    }
}

/// 按预设给视频格式排序，其他资源保持原有顺序并排在视频之后。
pub fn sort_videos(info: &mut MediaInfo, settings: &Settings) {
    let (preset, h264) = (settings.quality_preset, settings.prefer_h264);
    let mut videos: Vec<Asset> = info.assets.iter().filter(|a| a.kind == AssetKind::Video).cloned().collect();
    if videos.len() < 2 {
        return;
    }
    // 稳定排序：同等条件下保留平台给出的顺序
    videos.sort_by_key(|a| std::cmp::Reverse(rank(a, preset, h264)));
    let others = info.assets.iter().filter(|a| a.kind != AssetKind::Video).cloned();
    info.assets = videos.into_iter().chain(others).collect();
}

/// 自动下载（剪贴板、播放列表、订阅）时下载哪些资源，以及后处理选项。
pub fn auto_selection(info: &MediaInfo, settings: &Settings) -> (Vec<String>, PostOptions) {
    let mut post = PostOptions::default();
    if settings.quality_preset == QualityPreset::Audio && info.kind == MediaKind::Video {
        // 优先下载单独的音频轨；没有时下载视频再提取音频
        if let Some(a) = info.assets.iter().filter(|a| a.kind == AssetKind::Audio).max_by_key(|a| a.bitrate.unwrap_or(0)) {
            if a.ext != settings.audio_format {
                post.extract_audio = Some(settings.audio_format.clone());
            }
            return (vec![a.id.clone()], post);
        }
        post.extract_audio = Some(settings.audio_format.clone());
    }
    (info.default_asset_ids(), post)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, side: u32, codec: &str, br: u64) -> Asset {
        let mut a = Asset::base(id, AssetKind::Video, String::new(), id, "mp4");
        a.width = Some(side * 16 / 9);
        a.height = Some(side);
        a.vcodec = Some(codec.into());
        a.bitrate = Some(br);
        a
    }

    fn info() -> MediaInfo {
        let mut audio = Asset::base("a", AssetKind::Audio, String::new(), "a", "m4a");
        audio.bitrate = Some(128);
        let assets = vec![
            v("v360", 360, "avc1", 500),
            Asset::cover("c.jpg".into()),
            v("v2160", 2160, "vp9", 9000),
            v("v1080v", 1080, "vp9", 2000),
            v("v1080h", 1080, "avc1", 3000),
            v("v720", 720, "avc1", 1500),
            audio,
        ];
        MediaInfo {
            platform: "x".into(),
            platform_name: "x".into(),
            id: "1".into(),
            source_url: String::new(),
            title: "t".into(),
            author: String::new(),
            cover: None,
            duration_ms: None,
            kind: MediaKind::Video,
            width: None,
            height: None,
            published_at: None,
            assets,
            entries: vec![],
            series: None,
            extractor: None,
        }
    }

    fn first(preset: QualityPreset, h264: bool) -> String {
        let mut i = info();
        let s = Settings { quality_preset: preset, prefer_h264: h264, ..Settings::default() };
        sort_videos(&mut i, &s);
        assert_eq!(i.assets.len(), 7);
        i.assets[0].id.clone()
    }

    #[test]
    fn presets() {
        assert_eq!(first(QualityPreset::Best, true), "v2160");
        assert_eq!(first(QualityPreset::Max1080, true), "v1080h");
        assert_eq!(first(QualityPreset::Max1080, false), "v1080h", "higher bitrate wins without codec preference");
        assert_eq!(first(QualityPreset::Small, true), "v360");
    }

    #[test]
    fn audio_preset_selects_audio_track() {
        let s = Settings { quality_preset: QualityPreset::Audio, audio_format: "mp3".into(), ..Settings::default() };
        let (ids, post) = auto_selection(&info(), &s);
        assert_eq!(ids, vec!["a"]);
        assert_eq!(post.extract_audio.as_deref(), Some("mp3"));
        let s = Settings { audio_format: "m4a".into(), ..s };
        assert_eq!(auto_selection(&info(), &s).1.extract_audio, None);
        let (ids, post) = auto_selection(&info(), &Settings::default());
        assert_eq!(ids, vec!["v360"], "unsorted info keeps the provider order");
        assert_eq!(post, PostOptions::default());
    }
}
