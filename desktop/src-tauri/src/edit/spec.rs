//! 剪辑工程：片段（主轨）、文字、音频轨、输出规格，以及时间线的排布计算。
//!
//! 时间线的约定：主轨上的片段首尾相接；某个片段设了转场时，它的开头和上一个片段的结尾重叠（重叠的长度 = 转场时长），
//! 所以第 i 个片段的开始 = 上一个片段的结束 − 转场时长。文字和音频轨的时间都是时间线上的时间。
//! 前端有一份同样的排布计算（`src/utils/edit.ts` 的 `layout`），改这里的规则时要一起改。

use serde::{Deserialize, Serialize};

pub const MAX_CLIPS: usize = 300;
pub const MAX_TEXTS: usize = 100;
pub const MAX_AUDIO: usize = 20;
/// 片段、文字最短的长度
pub const MIN_MS: u64 = 100;
/// 小于这个长度的转场当作没有（只会闪一下）
pub const MIN_TRANSITION_MS: u64 = 100;

/// 可选的转场（xfade 的名字）。用真实的 ffmpeg 逐个试过，见端到端测试。
pub const TRANSITIONS: &[&str] = &[
    "fade",
    "fadeblack",
    "fadewhite",
    "dissolve",
    "wipeleft",
    "wiperight",
    "wipeup",
    "wipedown",
    "slideleft",
    "slideright",
    "slideup",
    "slidedown",
    "circleopen",
    "circleclose",
    "radial",
    "pixelize",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    #[default]
    Video,
    /// 静态图片：`out_ms` 就是显示的时长
    Image,
}

/// 这个片段开头的转场（和上一个片段混合）。第一个片段的转场没有意义，会被忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transition {
    pub kind: String,
    pub duration_ms: u64,
}

impl Default for Transition {
    fn default() -> Self {
        Transition { kind: "fade".into(), duration_ms: 500 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Clip {
    /// 界面里的编号，只用来在错误提示里区分
    pub id: u32,
    pub path: String,
    pub kind: ClipKind,
    /// 取素材的哪一段（毫秒）。图片：`in_ms` 为 0，`out_ms` 是显示时长
    pub in_ms: u64,
    pub out_ms: u64,
    /// 播放速度 0.25–4
    pub speed: f64,
    /// 音量 0–4（1 = 原样）
    pub volume: f64,
    pub mute: bool,
    /// 画面淡入 / 淡出（从黑色），声音同步淡入 / 淡出
    pub fade_in_ms: u64,
    pub fade_out_ms: u64,
    /// 顺时针旋转 0 / 90 / 180 / 270
    pub rotate: u16,
    pub flip_h: bool,
    pub flip_v: bool,
    /// 调色：亮度 -1–1（0 = 原样），对比度 0–3（1 = 原样），饱和度 0–3（1 = 原样）。
    /// 用 `lutyuv` 实现（精简版 ffmpeg 没有需要 GPL 的 `eq`）
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub transition: Option<Transition>,
}

impl Default for Clip {
    fn default() -> Self {
        Clip {
            id: 0,
            path: String::new(),
            kind: ClipKind::Video,
            in_ms: 0,
            out_ms: 0,
            speed: 1.0,
            volume: 1.0,
            mute: false,
            fade_in_ms: 0,
            fade_out_ms: 0,
            rotate: 0,
            flip_h: false,
            flip_v: false,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            transition: None,
        }
    }
}

impl Clip {
    /// 在时间线上占的时长（毫秒）。
    pub fn duration_ms(&self) -> u64 {
        let span = self.out_ms.saturating_sub(self.in_ms);
        match self.kind {
            ClipKind::Image => span,
            ClipKind::Video => (span as f64 / self.speed.max(0.01)).round() as u64,
        }
    }

    pub fn needs_tone(&self) -> bool {
        self.brightness.abs() > 1e-6 || (self.contrast - 1.0).abs() > 1e-6 || (self.saturation - 1.0).abs() > 1e-6
    }
}

/// 叠在画面上的文字。位置是文字中心在画面里的比例（0–1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TextItem {
    pub id: u32,
    pub text: String,
    /// 时间线上的出现和消失时间
    pub start_ms: u64,
    pub end_ms: u64,
    pub x: f64,
    pub y: f64,
    /// 字号，占输出画面高度的百分比
    pub size: f64,
    /// `#RRGGBB`
    pub color: String,
    pub opacity: f64,
    pub outline: bool,
    pub outline_color: String,
    /// 文字后面的底色块
    pub boxed: bool,
    pub box_color: String,
    pub box_opacity: f64,
    /// 字体文件（ttf / otf / ttc）；留空用系统里找到的中文字体
    pub font: Option<String>,
}

impl Default for TextItem {
    fn default() -> Self {
        TextItem {
            id: 0,
            text: String::new(),
            start_ms: 0,
            end_ms: 3000,
            x: 0.5,
            y: 0.85,
            size: 6.0,
            color: "#FFFFFF".into(),
            opacity: 1.0,
            outline: true,
            outline_color: "#000000".into(),
            boxed: false,
            box_color: "#000000".into(),
            box_opacity: 0.5,
            font: None,
        }
    }
}

/// 配乐、旁白、音效等：混在主轨声音上面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioTrack {
    pub id: u32,
    pub path: String,
    /// 在时间线上从哪里开始
    pub start_ms: u64,
    /// 取素材的哪一段；`out_ms` 为空表示到素材结尾
    pub in_ms: u64,
    pub out_ms: Option<u64>,
    pub volume: f64,
    pub fade_in_ms: u64,
    pub fade_out_ms: u64,
    /// 不够长时重复播放，直到视频结束
    pub looped: bool,
    /// 主轨有声音（人声）时自动把这条压低
    pub duck: bool,
}

impl Default for AudioTrack {
    fn default() -> Self {
        AudioTrack { id: 0, path: String::new(), start_ms: 0, in_ms: 0, out_ms: None, volume: 1.0, fade_in_ms: 0, fade_out_ms: 0, looped: false, duck: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputSpec {
    /// 0 = 跟随第一个视频片段（取偶数）
    pub width: u32,
    pub height: u32,
    /// 0 = 跟随第一个视频片段
    pub fps: f64,
    /// 片段和输出画面比例不同时：contain 留黑边 / cover 裁切填满 / blur 模糊背景
    pub fit: String,
    /// small / balanced / high
    pub quality: String,
    /// h264 / hevc
    pub codec: String,
    /// mp4 / mkv
    pub format: String,
}

impl Default for OutputSpec {
    fn default() -> Self {
        OutputSpec { width: 0, height: 0, fps: 0.0, fit: "contain".into(), quality: "balanced".into(), codec: "h264".into(), format: "mp4".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    /// 输出文件名（不含扩展名）；留空沿用第一个片段的名字
    pub title: String,
    pub clips: Vec<Clip>,
    pub texts: Vec<TextItem>,
    pub audio: Vec<AudioTrack>,
    pub out: OutputSpec,
}

/// 一个片段在时间线上的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub start_ms: u64,
    pub dur_ms: u64,
    /// 开头和上一个片段重叠多少（转场时长，已经按能放下的长度收紧）
    pub overlap_ms: u64,
}

impl Placed {
    pub fn end_ms(&self) -> u64 {
        self.start_ms + self.dur_ms
    }
}

/// 排布主轨。转场时长按“放得下”收紧：不超过前后两个片段，并且一个片段开头和结尾的转场加起来不超过它自己的长度。
pub fn layout(clips: &[Clip]) -> Vec<Placed> {
    let d: Vec<u64> = clips.iter().map(Clip::duration_ms).collect();
    let mut out: Vec<Placed> = Vec::with_capacity(clips.len());
    let mut prev_overlap = 0u64;
    for (i, c) in clips.iter().enumerate() {
        let overlap = if i == 0 {
            0
        } else {
            let want = c.transition.as_ref().map_or(0, |t| t.duration_ms);
            let fit = want.min(d[i - 1].saturating_sub(prev_overlap)).min(d[i]);
            if fit < MIN_TRANSITION_MS {
                0
            } else {
                fit
            }
        };
        let start = if i == 0 { 0 } else { out[i - 1].end_ms() - overlap };
        out.push(Placed { start_ms: start, dur_ms: d[i], overlap_ms: overlap });
        prev_overlap = overlap;
    }
    out
}

/// 时间线总长（毫秒）。
pub fn total_ms(clips: &[Clip]) -> u64 {
    layout(clips).last().map_or(0, Placed::end_ms)
}

fn hex_color(s: &str, fallback: &str) -> String {
    let t = s.trim().trim_start_matches('#');
    if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("#{}", t.to_ascii_uppercase())
    } else {
        fallback.to_string()
    }
}

fn clamp(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Project {
    /// 校验并整理：超出范围的数值收紧，结构性的错误（没有片段、素材路径为空、起止颠倒）返回提示。
    pub fn checked(mut self) -> Result<Project, String> {
        if self.clips.is_empty() {
            return Err("时间线上还没有片段，请先添加视频或图片。".into());
        }
        if self.clips.len() > MAX_CLIPS {
            return Err(format!("片段太多了（最多 {MAX_CLIPS} 个）。"));
        }
        if self.texts.len() > MAX_TEXTS {
            return Err(format!("文字太多了（最多 {MAX_TEXTS} 条）。"));
        }
        if self.audio.len() > MAX_AUDIO {
            return Err(format!("音频轨太多了（最多 {MAX_AUDIO} 条）。"));
        }
        for (i, c) in self.clips.iter_mut().enumerate() {
            let n = i + 1;
            if c.path.trim().is_empty() {
                return Err(format!("第 {n} 个片段没有素材文件。"));
            }
            match c.kind {
                ClipKind::Image => {
                    c.in_ms = 0;
                    c.speed = 1.0;
                    c.out_ms = c.out_ms.max(MIN_MS);
                }
                ClipKind::Video => {
                    if c.out_ms <= c.in_ms || c.out_ms - c.in_ms < MIN_MS {
                        return Err(format!("第 {n} 个片段的结束时间要比开始时间晚（至少 {MIN_MS} 毫秒）。"));
                    }
                    c.speed = clamp(c.speed, 0.25, 4.0, 1.0);
                }
            }
            c.volume = clamp(c.volume, 0.0, 4.0, 1.0);
            c.brightness = clamp(c.brightness, -1.0, 1.0, 0.0);
            c.contrast = clamp(c.contrast, 0.0, 3.0, 1.0);
            c.saturation = clamp(c.saturation, 0.0, 3.0, 1.0);
            if !matches!(c.rotate, 0 | 90 | 180 | 270) {
                c.rotate = (c.rotate % 360) / 90 * 90;
            }
            let d = c.duration_ms();
            c.fade_in_ms = c.fade_in_ms.min(d);
            c.fade_out_ms = c.fade_out_ms.min(d.saturating_sub(c.fade_in_ms));
            if let Some(t) = &mut c.transition {
                if !TRANSITIONS.contains(&t.kind.as_str()) {
                    t.kind = "fade".into();
                }
                if i == 0 || t.duration_ms < MIN_TRANSITION_MS {
                    c.transition = None;
                }
            }
        }
        self.texts.retain(|t| !t.text.trim().is_empty());
        for t in &mut self.texts {
            if t.end_ms < t.start_ms + MIN_MS {
                t.end_ms = t.start_ms + MIN_MS;
            }
            t.x = clamp(t.x, 0.0, 1.0, 0.5);
            t.y = clamp(t.y, 0.0, 1.0, 0.85);
            t.size = clamp(t.size, 1.0, 40.0, 6.0);
            t.opacity = clamp(t.opacity, 0.0, 1.0, 1.0);
            t.box_opacity = clamp(t.box_opacity, 0.0, 1.0, 0.5);
            t.color = hex_color(&t.color, "#FFFFFF");
            t.outline_color = hex_color(&t.outline_color, "#000000");
            t.box_color = hex_color(&t.box_color, "#000000");
            t.font = t.font.take().filter(|f| !f.trim().is_empty());
        }
        for (i, a) in self.audio.iter_mut().enumerate() {
            if a.path.trim().is_empty() {
                return Err(format!("第 {} 条音频轨没有素材文件。", i + 1));
            }
            if let Some(o) = a.out_ms {
                if o <= a.in_ms {
                    return Err(format!("第 {} 条音频轨的结束时间要比开始时间晚。", i + 1));
                }
            }
            a.volume = clamp(a.volume, 0.0, 4.0, 1.0);
        }
        let o = &mut self.out;
        if (o.width != 0 && !(16..=7680).contains(&o.width)) || (o.height != 0 && !(16..=7680).contains(&o.height)) || (o.width == 0) != (o.height == 0) {
            return Err("输出尺寸不对：宽和高都要在 16–7680 之间（或者都留空，跟随第一个片段）。".into());
        }
        o.fps = clamp(o.fps, 0.0, 120.0, 0.0);
        if o.fps != 0.0 && o.fps < 5.0 {
            return Err("输出帧率太低了（至少 5 帧；留空则跟随第一个片段）。".into());
        }
        if !matches!(o.fit.as_str(), "contain" | "cover" | "blur") {
            o.fit = "contain".into();
        }
        if !matches!(o.quality.as_str(), "small" | "balanced" | "high") {
            o.quality = "balanced".into();
        }
        if !matches!(o.codec.as_str(), "h264" | "hevc") {
            o.codec = "h264".into();
        }
        if !matches!(o.format.as_str(), "mp4" | "mkv") {
            o.format = "mp4".into();
        }
        Ok(self)
    }

    /// 所有用到的素材文件（去重，保持顺序）。
    pub fn media_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let paths = self.clips.iter().map(|c| &c.path).chain(self.audio.iter().map(|a| &a.path)).chain(self.texts.iter().filter_map(|t| t.font.as_ref()));
        for p in paths {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: u32, dur: u64) -> Clip {
        Clip { id, path: format!("/m/{id}.mp4"), in_ms: 0, out_ms: dur, ..Default::default() }
    }

    fn with_t(mut c: Clip, ms: u64) -> Clip {
        c.transition = Some(Transition { kind: "fade".into(), duration_ms: ms });
        c
    }

    #[test]
    fn clips_follow_each_other_and_transitions_overlap() {
        let clips = vec![clip(1, 4000), with_t(clip(2, 3000), 1000), clip(3, 2000)];
        let l = layout(&clips);
        assert_eq!((l[0].start_ms, l[0].end_ms()), (0, 4000));
        assert_eq!((l[1].start_ms, l[1].end_ms(), l[1].overlap_ms), (3000, 6000, 1000));
        assert_eq!((l[2].start_ms, l[2].end_ms(), l[2].overlap_ms), (6000, 8000, 0));
        assert_eq!(total_ms(&clips), 8000);
        assert_eq!(total_ms(&[]), 0);
    }

    #[test]
    fn speed_changes_how_long_a_clip_plays() {
        let mut c = clip(1, 6000);
        c.speed = 2.0;
        assert_eq!(c.duration_ms(), 3000);
        c.speed = 0.5;
        assert_eq!(c.duration_ms(), 12000);
        // 图片的时长就是 out_ms，不受速度影响
        let img = Clip { kind: ClipKind::Image, out_ms: 2500, speed: 4.0, ..Default::default() };
        assert_eq!(img.duration_ms(), 2500);
    }

    #[test]
    fn transitions_shrink_to_what_fits() {
        // 第 2 个片段只有 1 秒，请求 3 秒的转场：缩到 1 秒；第 3 个片段开头的转场再占掉第 2 个片段剩下的部分
        let clips = vec![clip(1, 5000), with_t(clip(2, 1000), 3000), with_t(clip(3, 5000), 2000)];
        let l = layout(&clips);
        assert_eq!(l[1].overlap_ms, 1000);
        // 第 2 个片段 1 秒已经全被开头的转场占了，第 3 个片段的转场放不下，当作硬切
        assert_eq!(l[2].overlap_ms, 0);
        assert_eq!(l[2].start_ms, l[1].end_ms());
        // 一个片段开头和结尾的转场加起来不超过它自己
        let clips = vec![clip(1, 5000), with_t(clip(2, 2000), 1500), with_t(clip(3, 5000), 1500)];
        let l = layout(&clips);
        assert_eq!((l[1].overlap_ms, l[2].overlap_ms), (1500, 500));
        assert!(l[1].overlap_ms + l[2].overlap_ms <= l[1].dur_ms);
        // 第一个片段的转场没有意义；太短的转场当作没有
        let clips = vec![with_t(clip(1, 3000), 800), with_t(clip(2, 3000), 50)];
        let l = layout(&clips);
        assert_eq!((l[0].overlap_ms, l[1].overlap_ms, l[1].start_ms), (0, 0, 3000));
    }

    #[test]
    fn checked_tightens_numbers_and_rejects_structural_mistakes() {
        assert!(Project::default().checked().unwrap_err().contains("还没有片段"));
        let mut p = Project { clips: vec![clip(1, 3000)], ..Default::default() };
        p.clips[0].path.clear();
        assert!(p.clone().checked().unwrap_err().contains("第 1 个片段没有素材"));
        p.clips[0].path = "/m/1.mp4".into();
        p.clips[0].out_ms = 0;
        assert!(p.clone().checked().unwrap_err().contains("结束时间要比开始时间晚"));

        let mut p = Project { clips: vec![clip(1, 3000), with_t(clip(2, 3000), 800)], ..Default::default() };
        p.clips[0].speed = 99.0;
        p.clips[0].volume = f64::NAN;
        p.clips[0].fade_in_ms = 10_000;
        p.clips[0].fade_out_ms = 10_000;
        p.clips[0].rotate = 450;
        p.clips[1].transition.as_mut().unwrap().kind = "nonsense".into();
        p.texts = vec![
            TextItem { text: "  ".into(), ..Default::default() },
            TextItem { text: "你好".into(), start_ms: 1000, end_ms: 900, color: "red".into(), x: 7.0, size: 500.0, ..Default::default() },
        ];
        p.out.fit = "weird".into();
        p.out.format = "avi".into();
        let q = p.checked().unwrap();
        assert_eq!(q.clips[0].speed, 4.0);
        assert_eq!(q.clips[0].volume, 1.0);
        // 淡入 + 淡出不超过片段自己的长度（速度 4 倍，750 毫秒）
        assert!(q.clips[0].fade_in_ms + q.clips[0].fade_out_ms <= q.clips[0].duration_ms());
        assert_eq!(q.clips[0].rotate, 90);
        assert_eq!(q.clips[1].transition.as_ref().unwrap().kind, "fade");
        assert_eq!(q.texts.len(), 1, "空文字被丢掉");
        let t = &q.texts[0];
        assert_eq!((t.end_ms, t.color.as_str(), t.x, t.size), (1100, "#FFFFFF", 1.0, 40.0));
        assert_eq!((q.out.fit.as_str(), q.out.format.as_str()), ("contain", "mp4"));
    }

    #[test]
    fn output_size_must_be_both_or_neither() {
        let mut p = Project { clips: vec![clip(1, 3000)], ..Default::default() };
        p.out.width = 1280;
        assert!(p.clone().checked().unwrap_err().contains("输出尺寸"));
        p.out.height = 720;
        assert!(p.clone().checked().is_ok());
        p.out.fps = 2.0;
        assert!(p.checked().unwrap_err().contains("帧率"));
    }

    #[test]
    fn media_paths_are_unique_and_include_fonts_and_music() {
        let mut p = Project { clips: vec![clip(1, 3000), clip(1, 3000)], ..Default::default() };
        p.audio = vec![AudioTrack { path: "/m/a.mp3".into(), ..Default::default() }];
        p.texts = vec![TextItem { text: "x".into(), font: Some("/f/x.ttf".into()), ..Default::default() }];
        assert_eq!(p.media_paths(), vec!["/m/1.mp4", "/m/a.mp3", "/f/x.ttf"]);
    }

    #[test]
    fn projects_round_trip_through_json_with_camel_case() {
        let mut p = Project { title: "旅行".into(), clips: vec![with_t(clip(2, 3000), 400)], ..Default::default() };
        p.clips[0].fade_in_ms = 200;
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"inMs\"") && j.contains("\"fadeInMs\"") && j.contains("\"durationMs\""), "{j}");
        let back: Project = serde_json::from_str(&j).unwrap();
        assert_eq!(back, p);
        // 缺少的字段用默认值（旧版本存的工程文件也能打开）
        let min: Project = serde_json::from_str(r#"{"clips":[{"path":"/a.mp4","outMs":1000}]}"#).unwrap();
        assert_eq!((min.clips[0].speed, min.clips[0].volume, min.out.fit.as_str()), (1.0, 1.0, "contain"));
    }

    /// 前端（界面测试里导出的真实请求）发来的 JSON 能被完整读进来并通过校验。
    #[test]
    fn a_job_sent_by_the_editor_ui_is_understood() {
        use crate::media_tools::{ToolJob, ToolOp};
        let job: ToolJob = serde_json::from_str(include_str!("fixtures/frontend-job.json")).unwrap();
        assert!(job.inputs.is_empty() && job.output_dir.is_none());
        let ToolOp::Edit { project } = job.op else { panic!("应该是剪辑任务") };
        let p = project.checked().unwrap();
        assert_eq!((p.clips.len(), p.texts.len(), p.audio.len()), (3, 1, 1));
        assert_eq!(p.clips[1].transition.as_ref().map(|t| (t.kind.as_str(), t.duration_ms)), Some(("fade", 500)));
        assert_eq!((p.clips[1].in_ms, p.clips[1].out_ms), (1500, 4000));
        assert_eq!(p.audio[0].start_ms, 5000);
        assert_eq!(p.texts[0].text, "你好 Hello");
        // 1.5 + (2.5 - 0.5 转场重叠) + 5.003 = 8.503 秒，和界面显示的总长一致
        assert_eq!(total_ms(&p.clips), 8503);
        assert!((p.clips[0].volume - 1.05).abs() < 1e-9);
        assert_eq!(p.media_paths(), vec!["/media/red.webm", "/media/green.webm", "/media/music.ogg"]);
    }
}
