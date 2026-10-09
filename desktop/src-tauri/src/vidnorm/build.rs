//! 把规格 + 素材信息 + 分析结果 + ffmpeg 能力翻译成 ffmpeg 参数。纯函数（只会往临时目录写 HDR 用的 LUT 文件）。

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::media_tools::{s, target_kbps, video_codec_for};
use crate::vidcaps::Caps;

use super::facts::{Facts, Hdr, VideoFacts};
use super::hdr;
use super::spec::{Analysis, NormSpec};

#[derive(Debug, Default)]
pub struct NormPlan {
    /// ffmpeg 参数（不含 `-hide_banner -y` 等公共部分，也不含输出文件）
    pub args: Vec<String>,
    pub ext: &'static str,
    /// 做了哪些处理
    pub notes: Vec<String>,
    /// 做不到或改用了替代做法的说明
    pub warnings: Vec<String>,
    pub temp_files: Vec<PathBuf>,
    /// 素材已经符合规格，不需要生成新文件
    pub unchanged: bool,
    pub video_copy: bool,
    pub out_ms: Option<u64>,
}

/// 视频滤镜图（单个输入 `[0:v:0]`，输出 `[v]`）和相关结论。
#[derive(Debug, Default)]
pub struct VideoGraph {
    /// 没有任何视频处理时为 None
    pub graph: Option<String>,
    pub out_w: u32,
    pub out_h: u32,
    pub out_fps: Option<f64>,
    /// 需要重新编码的原因
    pub reasons: Vec<&'static str>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
    pub temp_files: Vec<PathBuf>,
}

// ---------- 滤镜图工具 ----------

/// 把路径放进滤镜参数：转成正斜杠，再按两层转义（选项层转义 `\ ' :`，滤镜图层转义 `\ ' , ; [ ]`）。
pub fn filter_path(p: &Path) -> String {
    fn esc(s: &str, set: &[char]) -> String {
        let mut out = String::with_capacity(s.len() + 8);
        for c in s.chars() {
            if set.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
    let s = p.to_string_lossy().replace('\\', "/");
    esc(&esc(&s, &['\\', '\'', ':']), &['\\', '\'', ',', ';', '[', ']'])
}

/// 逐步拼出滤镜图：一条主链，必要时分叉再合并。
struct Graph {
    stmts: Vec<String>,
    chain: String,
    fresh: bool,
    n: u32,
}

impl Graph {
    fn new() -> Graph {
        Graph { stmts: vec![], chain: "[0:v:0]".into(), fresh: true, n: 0 }
    }

    fn push(&mut self, f: &str) {
        if !self.fresh {
            self.chain.push(',');
        }
        self.chain.push_str(f);
        self.fresh = false;
    }

    fn is_empty(&self) -> bool {
        self.fresh && self.stmts.is_empty()
    }

    /// 把当前画面和“经过 `branch` 处理后的画面”按强度混合。
    fn mix(&mut self, strength: f64, branch: &str) {
        self.n += 1;
        let (a, b, c) = (format!("m{}a", self.n), format!("m{}b", self.n), format!("m{}c", self.n));
        self.push(&format!("split[{a}][{b}]"));
        self.stmts.push(std::mem::take(&mut self.chain));
        self.stmts.push(format!("[{b}]{branch}[{c}]"));
        self.chain = format!("[{a}][{c}]blend=all_mode=normal:all_opacity={strength:.3}");
        self.fresh = false;
    }

    /// 等比放进 w×h，两侧或上下用同一画面放大后模糊的背景补满。
    fn blur_fill(&mut self, w: u32, h: u32) {
        self.n += 1;
        let (a, b, bg, fg) = (format!("b{}a", self.n), format!("b{}b", self.n), format!("b{}g", self.n), format!("b{}f", self.n));
        self.push(&format!("split[{a}][{b}]"));
        self.stmts.push(std::mem::take(&mut self.chain));
        self.stmts.push(format!("[{a}]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=25:5[{bg}]"));
        self.stmts.push(format!("[{b}]scale={w}:{h}:force_original_aspect_ratio=decrease[{fg}]"));
        self.chain = format!("[{bg}][{fg}]overlay=(W-w)/2:(H-h)/2");
        self.fresh = false;
    }

    fn finish(mut self) -> String {
        self.chain.push_str("[v]");
        self.stmts.push(self.chain);
        self.stmts.join(";")
    }
}

// ---------- 帧率 ----------

/// 常见的标准帧率（分子、分母）。
const STANDARD_FPS: [(u32, u32); 8] = [(24000, 1001), (24, 1), (25, 1), (30000, 1001), (30, 1), (50, 1), (60000, 1001), (60, 1)];

/// 最接近的标准帧率。
pub fn nearest_standard_fps(fps: f64) -> (u32, u32) {
    STANDARD_FPS
        .iter()
        .copied()
        .min_by(|a, b| (f64::from(a.0) / f64::from(a.1) - fps).abs().total_cmp(&(f64::from(b.0) / f64::from(b.1) - fps).abs()))
        .unwrap_or((30, 1))
}

/// 把用户填的帧率写成 ffmpeg 的分数写法：23.976 → 24000/1001，29.97 → 30000/1001，59.94 → 60000/1001。
pub fn fps_fraction(v: f64) -> (u32, u32) {
    for (n, d) in [(24000, 1001), (30000, 1001), (60000, 1001)] {
        if (f64::from(n) / f64::from(d) - v).abs() < 0.015 {
            return (n, d);
        }
    }
    (v.round().max(1.0) as u32, 1)
}

fn frac_f(f: (u32, u32)) -> f64 {
    f64::from(f.0) / f64::from(f.1)
}

fn frac_text(f: (u32, u32)) -> String {
    if f.1 == 1 {
        f.0.to_string()
    } else {
        format!("{}/{}", f.0, f.1)
    }
}

// ---------- 色彩 ----------

/// 把没写明的色彩空间按常规猜：高清用 BT.709，标清用 BT.601（smpte170m）。
fn assumed_matrix(v: &VideoFacts) -> &str {
    match v.matrix.as_deref() {
        Some("bt709") => "bt709",
        Some(m @ ("bt470bg" | "smpte170m" | "fcc" | "smpte240m")) => m,
        Some("bt2020nc" | "bt2020c") => "bt2020nc",
        _ if v.height >= 720 => "bt709",
        _ => "smpte170m",
    }
}

/// 输出文件的色彩标记。高清统一成 BT.709；标清保持 BT.601 不转换；已经转成 BT.709 的（HDR 转 SDR）一定是 BT.709。
fn out_color(v: &VideoFacts, out_h: u32, fix: bool, to_709: bool) -> Option<&'static str> {
    if !fix {
        return None;
    }
    if to_709 || out_h >= 720 || matches!(assumed_matrix(v), "bt709") {
        Some("bt709")
    } else {
        Some("smpte170m")
    }
}

// ---------- 画面尺寸 ----------

enum SizeOp {
    None,
    Filter(String),
    Blur(u32, u32),
}

fn even(v: u32) -> u32 {
    v & !1
}

/// 画面尺寸处理：返回做法和输出尺寸（宽, 高）。输入是裁黑边之后、按显示方向的尺寸。
fn plan_size(spec: &NormSpec, w: u32, h: u32) -> (SizeOp, u32, u32, Option<&'static str>) {
    let keep = || {
        if w % 2 == 1 || h % 2 == 1 {
            (SizeOp::Filter("crop=trunc(iw/2)*2:trunc(ih/2)*2".into()), even(w), even(h), Some("奇数尺寸"))
        } else {
            (SizeOp::None, w, h, None)
        }
    };
    match spec.size.as_str() {
        "keep" => keep(),
        "limit" => {
            let short = w.min(h);
            if short <= spec.short_side {
                return keep();
            }
            let ratio = f64::from(spec.short_side) / f64::from(short);
            let long = even((f64::from(w.max(h)) * ratio).round() as u32);
            if h > w {
                (SizeOp::Filter(format!("scale={}:-2", spec.short_side)), spec.short_side, long, Some("缩小"))
            } else {
                (SizeOp::Filter(format!("scale=-2:{}", spec.short_side)), long, spec.short_side, Some("缩小"))
            }
        }
        mode => {
            let (mut tw, mut th) = (spec.width, spec.height);
            if spec.follow_orientation && w != h && (h > w) != (th > tw) {
                std::mem::swap(&mut tw, &mut th);
            }
            if (w, h) == (tw, th) {
                return (SizeOp::None, w, h, None);
            }
            let same_aspect = (f64::from(w) / f64::from(h) - f64::from(tw) / f64::from(th)).abs() < 0.003;
            let op = if same_aspect {
                SizeOp::Filter(format!("scale={tw}:{th}"))
            } else {
                match mode {
                    "fill" => SizeOp::Filter(format!("scale={tw}:{th}:force_original_aspect_ratio=increase,crop={tw}:{th}")),
                    "fit" => SizeOp::Filter(format!("scale={tw}:{th}:force_original_aspect_ratio=decrease,pad={tw}:{th}:(ow-iw)/2:(oh-ih)/2:black")),
                    _ => SizeOp::Blur(tw, th),
                }
            };
            (op, tw, th, Some("统一尺寸"))
        }
    }
}

// ---------- HDR → SDR ----------

fn tonemap_chain(v: &VideoFacts, caps: &Caps, tmp: &Path, g: &mut VideoGraph) -> Option<String> {
    let tin = match v.hdr {
        Hdr::Pq => "smpte2084",
        Hdr::Hlg => "arib-std-b67",
        Hdr::None => return None,
    };
    if v.dolby_profile == Some(5) {
        g.warnings.push("这是杜比视界 Profile 5（没有 HDR10 兼容层），转成 SDR 后颜色可能偏紫偏绿，建议下载 HDR10 / SDR 版本。".into());
    }
    if caps.can_tonemap() {
        let m = if v.matrix.as_deref() == Some("bt2020c") { "bt2020c" } else { "bt2020nc" };
        g.notes.push(format!("{} 转 SDR（zscale + Hable 色调映射）", v.hdr.label()));
        return Some(format!(
            "zscale=tin={tin}:min={m}:pin=bt2020:rin=tv:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
        ));
    }
    if caps.has_filter("lut3d") {
        let path = tmp.join(format!("hdr-{}.cube", if v.hdr == Hdr::Hlg { "hlg" } else { "pq" }));
        if std::fs::create_dir_all(tmp).and_then(|_| std::fs::write(&path, hdr::cube(v.hdr))).is_err() {
            g.warnings.push("无法写入临时文件，HDR 没有转换。".into());
            return None;
        }
        g.temp_files.push(path.clone());
        g.warnings.push(format!(
            "当前的 ffmpeg 没有 zscale，HDR 转 SDR 使用了程序内置的色调映射（3D LUT），效果接近。{}",
            if caps.gpl { "" } else { "安装完整版 ffmpeg 可以用 zscale。" }
        ));
        g.notes.push(format!("{} 转 SDR（内置 3D LUT 色调映射）", v.hdr.label()));
        return Some(format!(
            "scale=in_color_matrix=bt2020:in_range=tv,format=gbrp16le,lut3d=file={}:interp=tetrahedral,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
            filter_path(&path)
        ));
    }
    g.warnings.push("当前的 ffmpeg 既没有 zscale 也没有 lut3d，无法把 HDR 转成 SDR。".into());
    None
}

// ---------- 视频滤镜图 ----------

/// 生成视频滤镜图。`pix` 是编码器要求的像素格式；`preview` 为真时不做帧率处理（只截一帧）。
pub fn video_graph(spec: &NormSpec, facts: &Facts, an: &Analysis, caps: &Caps, tmp: &Path, pix: &str, preview: bool) -> AppResult<VideoGraph> {
    let v = facts.video.as_ref().ok_or_else(|| AppError::invalid("这个文件里没有视频画面。"))?;
    let mut out = VideoGraph::default();
    let mut g = Graph::new();

    // 分段色彩匹配：放在最前面，滤镜看到的时间戳和分析时一致（后面的帧率转换会改写时间戳）。
    // 校正用 lutyuv，只认 8 位的 YUV 平面格式，其他格式先转成 yuv420p。
    if spec.match_color > 0.0 {
        if let Some(plan) = an.color.as_ref().filter(|p| !p.fixes.is_empty()) {
            if v.hdr != Hdr::None {
                out.warnings.push("HDR 素材要先转成 SDR，分段色彩匹配没有应用。".into());
            } else {
                let planar8 = matches!(v.pix_fmt.as_str(), "yuv420p" | "yuvj420p" | "yuv422p" | "yuvj422p" | "yuv444p" | "yuvj444p" | "yuv440p");
                if !planar8 {
                    g.push("format=yuv420p");
                }
                for fix in &plan.fixes {
                    g.push(&fix.filter());
                }
                out.reasons.push("color_match");
                out.notes.push(format!("分段色彩匹配：{} 个镜头单独校正到和整体一致", plan.fixes.len()));
            }
        }
    }

    // 帧率
    let fps_target: Option<(u32, u32)> = match spec.fps.as_str() {
        "fixed" => Some(fps_fraction(spec.fps_value)),
        "auto" => an.vfr.as_ref().filter(|f| f.variable).map(|f| nearest_standard_fps(if f.median_fps > 0.0 { f.median_fps } else { v.fps.unwrap_or(30.0) })),
        _ => None,
    };
    let variable = an.vfr.as_ref().is_some_and(|f| f.variable);
    if let Some(t) = fps_target {
        let same = !variable && v.fps.is_some_and(|f| (f - frac_f(t)).abs() < 0.02);
        if !same {
            if !preview {
                g.push(&format!("fps={}", frac_text(t)));
            }
            out.out_fps = Some(frac_f(t));
            out.reasons.push("fps");
            out.notes.push(if variable { format!("可变帧率转成固定 {} 帧", frac_text(t)) } else { format!("帧率统一为 {} 帧", frac_text(t)) });
        }
    }
    if out.out_fps.is_none() {
        out.out_fps = v.fps;
    }

    // 去黑边
    let (mut w, mut h) = v.display_size();
    if spec.autocrop {
        if let Some(c) = &an.crop {
            g.push(&format!("crop={}:{}:{}:{}", c.w, c.h, c.x, c.y));
            (w, h) = (c.w, c.h);
            out.reasons.push("crop");
            let (t, b, l, r) = c.margins();
            out.notes.push(format!("去黑边（上 {t} 下 {b} 左 {l} 右 {r}）"));
        }
    }
    // 非方形像素：先拉成方形
    if let Some((a, b)) = v.sar.filter(|(a, b)| a != b) {
        let nw = even((f64::from(w) * f64::from(a) / f64::from(b)).round() as u32).max(2);
        g.push(&format!("scale={nw}:{h},setsar=1"));
        w = nw;
        out.reasons.push("sar");
        out.notes.push("非方形像素改为方形".into());
    }

    // 画面尺寸
    let (op, ow, oh, why) = plan_size(spec, w, h);
    match op {
        SizeOp::None => {}
        SizeOp::Filter(f) => g.push(&f),
        SizeOp::Blur(tw, th) => g.blur_fill(tw, th),
    }
    if let Some(why) = why {
        out.reasons.push("size");
        if why != "奇数尺寸" {
            out.notes.push(format!("{why}：{w}×{h} → {ow}×{oh}"));
        }
    }
    (out.out_w, out.out_h) = (ow, oh);

    // HDR → SDR
    let mut sdr_done = false;
    if spec.hdr && v.hdr != Hdr::None {
        if let Some(c) = tonemap_chain(v, caps, tmp, &mut out) {
            g.push(&c);
            sdr_done = true;
            out.reasons.push("hdr");
        }
    }

    // 色彩：全范围转电视范围、BT.601 转 BT.709
    let assumed = assumed_matrix(v).to_string();
    if spec.fix_color && !sdr_done {
        let hd_out = oh >= 720;
        if v.full_range() {
            g.push("scale=in_range=pc:out_range=tv");
            out.reasons.push("range");
            out.notes.push("全范围色彩转电视范围".into());
        }
        if hd_out && matches!(assumed.as_str(), "bt470bg" | "smpte170m" | "fcc" | "smpte240m") && caps.has_filter("colorspace") {
            g.push(&format!("colorspace=all=bt709:iall={assumed}"));
            out.reasons.push("matrix");
            out.notes.push("BT.601 转 BT.709".into());
        }
    }

    // 自动色阶
    if spec.levels > 0.0 {
        if caps.has_filter("normalize") {
            g.push(&format!("normalize=smoothing=30:independence=0:strength={:.2}", spec.levels));
            out.reasons.push("levels");
            out.notes.push(format!("自动色阶（强度 {:.0}%）", spec.levels * 100.0));
        } else {
            out.warnings.push("当前的 ffmpeg 没有 normalize 滤镜，自动色阶没有执行。".into());
        }
    }

    // LUT
    if let Some(lut) = &spec.lut {
        if !Path::new(lut).is_file() {
            return Err(AppError::invalid(format!("找不到 LUT 文件：{lut}")));
        }
        if caps.has_filter("lut3d") {
            let chain = format!(
                "scale=in_color_matrix=bt709:in_range=tv,format=gbrp16le,lut3d=file={}:interp=tetrahedral,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
                filter_path(Path::new(lut))
            );
            if spec.lut_strength >= 0.995 {
                g.push(&chain);
            } else if spec.lut_strength > 0.0 {
                g.mix(spec.lut_strength, &chain);
            }
            if spec.lut_strength > 0.0 {
                out.reasons.push("lut");
                let name = Path::new(lut).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                out.notes.push(format!("应用 LUT：{name}（强度 {:.0}%）", spec.lut_strength * 100.0));
            }
        } else {
            out.warnings.push("当前的 ffmpeg 没有 lut3d 滤镜，LUT 没有应用。".into());
        }
    }

    // 像素格式：转成 8 位 4:2:0
    let pix_change = v.pix_fmt != "yuv420p" && !(v.pix_fmt == "nv12" && pix == "nv12");
    if pix_change && !out.reasons.contains(&"hdr") {
        out.reasons.push("pix");
        if v.bit_depth > 8 {
            out.notes.push(format!("{} 位色深转 8 位", v.bit_depth));
        }
    }
    if !g.is_empty() || pix_change {
        g.push(&format!("format={pix}"));
        // 写明输出的色彩标记（没有转成 SDR 的 HDR 保持原标记）
        if let Some(tag) = out_color(v, oh, spec.fix_color, sdr_done).filter(|_| v.hdr == Hdr::None || sdr_done) {
            g.push(&format!("setparams=colorspace={tag}:color_primaries={tag}:color_trc={tag}:range=tv"));
        }
        out.graph = Some(g.finish());
    }
    Ok(out)
}

// ---------- 编码 ----------

fn replace_arg(args: &mut [String], key: &str, value: &str) {
    if let Some(i) = args.iter().position(|a| a == key) {
        if let Some(slot) = args.get_mut(i + 1) {
            *slot = value.to_string();
        }
    }
}

/// HEVC 编码参数：(参数, 像素格式)。没有可用的 HEVC 编码器时返回 None。
fn hevc_args(caps: &Caps, level: usize, kbps: u32) -> Option<(Vec<String>, &'static str)> {
    let name = caps.hevc?;
    let q = |a: [&str; 3]| a[level.min(2)].to_string();
    let rate = format!("{}k", (f64::from(kbps) * 0.65) as u32);
    Some(match name {
        "libx265" => {
            (s(&["-c:v", "libx265", "-preset", "medium", "-crf", &q(["27", "24", "21"]), "-tag:v", "hvc1", "-x265-params", "log-level=error"]), "yuv420p")
        }
        "hevc_nvenc" => (s(&["-c:v", name, "-preset", "p5", "-rc", "vbr", "-cq", &q(["33", "29", "25"]), "-b:v", "0", "-tag:v", "hvc1"]), "yuv420p"),
        "hevc_qsv" => (s(&["-c:v", name, "-global_quality", &q(["33", "29", "25"]), "-preset", "medium", "-tag:v", "hvc1"]), "nv12"),
        "hevc_amf" => {
            let qp = q(["30", "26", "22"]);
            (s(&["-c:v", name, "-quality", "quality", "-rc", "cqp", "-qp_i", &qp, "-qp_p", &qp, "-tag:v", "hvc1"]), "yuv420p")
        }
        _ => (s(&["-c:v", name, "-b:v", &rate, "-tag:v", "hvc1", "-allow_sw", "1"]), "yuv420p"),
    })
}

/// 编码器要求的像素格式（决定滤镜链最后的 format）。
fn encoder_pix(spec: &NormSpec, caps: &Caps) -> &'static str {
    let h264_qsv = caps.encoders.h264 == Some("h264_qsv");
    let hevc_qsv = caps.hevc == Some("hevc_qsv");
    if (spec.codec == "hevc" && hevc_qsv) || (spec.codec != "hevc" && h264_qsv) {
        "nv12"
    } else {
        "yuv420p"
    }
}

fn level_of(quality: &str) -> usize {
    match quality {
        "small" => 0,
        "high" => 2,
        _ => 1,
    }
}

/// 音频：能直接放进 MP4 的编码。
fn mp4_audio_ok(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3" | "ac3" | "eac3")
}

/// 视频：能直接放进 MP4 的编码。
fn mp4_video_ok(codec: &str) -> bool {
    matches!(codec, "h264" | "hevc" | "av1" | "vp9" | "mpeg4")
}

pub fn build(input: &Path, spec: &NormSpec, facts: &Facts, an: &Analysis, caps: &Caps, tmp: &Path) -> AppResult<NormPlan> {
    let v = facts.video.as_ref().ok_or_else(|| AppError::invalid("这个文件里没有视频画面，视频规整只适用于视频文件。"))?;
    let level = level_of(&spec.quality);
    let pix = encoder_pix(spec, caps);
    let mut vg = video_graph(spec, facts, an, caps, tmp, pix, false)?;
    let mut plan = NormPlan { out_ms: facts.duration_ms, ext: "mp4", ..Default::default() };
    plan.notes.append(&mut vg.notes);
    plan.warnings.append(&mut vg.warnings);
    plan.temp_files.append(&mut vg.temp_files);

    // 要不要重新编码视频
    let mut reasons = vg.reasons.clone();
    match spec.codec.as_str() {
        "h264" if v.codec != "h264" => reasons.push("codec"),
        "hevc" if v.codec != "hevc" => reasons.push("codec"),
        _ => {}
    }
    if !mp4_video_ok(&v.codec) {
        reasons.push("container");
    }
    // 旋转标记：只复制视频时原样保留，重新编码时 ffmpeg 会自动转正
    reasons.sort_unstable();
    reasons.dedup();
    let reencode = !reasons.is_empty();

    let mut a: Vec<String> = vec![];
    if facts.is_stream_container() {
        // FLV / TS：补全缺失的显示时间戳
        a.extend(s(&["-fflags", "+genpts"]));
    }
    a.extend(s(&["-i", &input.to_string_lossy()]));

    // ---- 视频 ----
    let mut tag_patch = false;
    if reencode {
        let (w, h) = (vg.out_w.max(2), vg.out_h.max(2));
        let color_tag = out_color(v, vg.out_h, spec.fix_color, vg.reasons.contains(&"hdr")).filter(|_| v.hdr == Hdr::None || vg.reasons.contains(&"hdr"));
        // 新版 ffmpeg（2026 年的开发版）不再用 -color_primaries / -color_trc 给输出写标记，只认画面上的标记；
        // 没有别的滤镜时也要走一遍 setparams，否则这两项会被写成 unknown。
        if vg.graph.is_none() {
            if let Some(tag) = color_tag {
                vg.graph = Some(format!("[0:v:0]setparams=colorspace={tag}:color_primaries={tag}:color_trc={tag}:range=tv[v]"));
            }
        }
        let fps = vg.out_fps.or(v.fps).unwrap_or(30.0).clamp(1.0, 240.0);
        let pps = f64::from(w) * f64::from(h) * fps;
        let want_hevc = spec.codec == "hevc";
        let hevc = if want_hevc { hevc_args(caps, level, target_kbps(pps, level)) } else { None };
        if want_hevc && hevc.is_none() {
            plan.warnings.push(format!(
                "当前的 ffmpeg 没有 HEVC 编码器，已改用 H.264。{}",
                if caps.gpl { "" } else { "在“设置 → 组件”里安装完整版 ffmpeg 可以使用 libx265。" }
            ));
        }
        // 重新编码的画质要比“压缩”高一档
        let q_for_codec = match spec.quality.as_str() {
            "small" => "balanced",
            _ => "high",
        };
        match hevc {
            Some((args, _)) => {
                match &vg.graph {
                    Some(g) => a.extend(["-filter_complex".into(), g.clone(), "-map".into(), "[v]".into()]),
                    None => a.extend(s(&["-map", "0:v:0"])),
                }
                a.extend(args);
                plan.notes.push("视频重新编码为 HEVC".into());
            }
            None => {
                let mut vc = video_codec_for(&caps.encoders, q_for_codec, "mp4", Some(pps));
                replace_arg(&mut vc.args, "-crf", ["25", "21", "18"][level]);
                match &vg.graph {
                    Some(g) => a.extend(["-filter_complex".into(), g.clone(), "-map".into(), "[v]".into()]),
                    None => a.extend(s(&["-map", "0:v:0"])),
                }
                a.extend(vc.args);
                plan.ext = vc.ext;
                if let Some(n) = vc.note {
                    plan.warnings.push(n);
                }
                plan.notes.push(format!("视频重新编码（{}）", reasons_text(&reasons)));
            }
        }
        // 帧率：保持原有的可变帧率
        if vg.out_fps == v.fps && an.vfr.as_ref().is_some_and(|f| f.variable) && spec.fps == "keep" {
            a.extend(if caps.has_fps_mode() { s(&["-fps_mode", "vfr"]) } else { s(&["-vsync", "vfr"]) });
        }
        if let Some(tag) = color_tag {
            a.extend(s(&["-colorspace", tag, "-color_primaries", tag, "-color_trc", tag, "-color_range", "tv"]));
        }
        if v.rotation != 0 {
            plan.notes.push(format!("旋转标记 {}° 已转正", v.rotation));
        }
    } else {
        a.extend(s(&["-map", "0:v:0", "-c:v", "copy"]));
        plan.video_copy = true;
        // 色彩信息没写：不动画面，用比特流滤镜补写标记
        if spec.fix_color && v.untagged() && !v.full_range() {
            let (code, name) = if v.height >= 720 { (1, "BT.709") } else { (6, "BT.601") };
            let bsf = match v.codec.as_str() {
                "h264" => Some("h264_metadata"),
                "hevc" => Some("hevc_metadata"),
                _ => None,
            };
            if let Some(b) = bsf {
                a.extend([
                    "-bsf:v".into(),
                    format!("{b}=colour_primaries={code}:transfer_characteristics={code}:matrix_coefficients={code}:video_full_range_flag=0"),
                ]);
                plan.notes.push(format!("补写了色彩标记（{name}），画面没有重新编码"));
                tag_patch = true;
            }
        }
        if v.rotation != 0 {
            plan.notes.push("视频流直接复制，旋转标记原样保留".into());
        }
    }

    // ---- 音频 ----
    let mut audio_changed = false;
    if let Some(au) = &facts.audio {
        a.extend(s(&["-map", "0:a:0?"]));
        let signs = facts.is_stream_container() || an.vfr.as_ref().is_some_and(|f| f.variable);
        let sync = spec.fix_sync && signs;
        let rate_change = spec.audio_rate != 0 && au.sample_rate != spec.audio_rate;
        let ch_change = spec.audio_channels != 0 && au.channels != spec.audio_channels && au.channels != 0;
        let loud = spec.loudness;
        let must = loud.is_some() || sync || rate_change || ch_change || !mp4_audio_ok(&au.codec);
        if must {
            audio_changed = true;
            let mut af: Vec<String> = vec![];
            if sync {
                af.push("aresample=async=1:first_pts=0".into());
                plan.notes.push("修正音画不同步和时间戳跳变".into());
            }
            if let Some(t) = loud {
                af.push(match &an.loudness {
                    Some(m) => format!(
                        "loudnorm=I={t}:TP=-1.5:LRA=11:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:measured_thresh={:.2}:offset={:.2}:linear=true",
                        m.input_i, m.input_tp, m.input_lra, m.input_thresh, m.target_offset
                    ),
                    None => format!("loudnorm=I={t}:TP=-1.5:LRA=11"),
                });
                plan.notes.push(format!("响度标准化到 {t} LUFS（{}）", if an.loudness.is_some() { "两遍测量，保持动态" } else { "单遍" }));
            }
            if !af.is_empty() {
                a.extend(["-af".into(), af.join(",")]);
            }
            // loudnorm 内部按 192 kHz 输出，必须明确指定采样率
            let rate = if spec.audio_rate != 0 {
                spec.audio_rate
            } else if au.sample_rate != 0 {
                au.sample_rate
            } else {
                48000
            };
            if rate_change || loud.is_some() || sync {
                a.extend(["-ar".into(), rate.to_string()]);
            }
            let ch = if spec.audio_channels != 0 { spec.audio_channels } else { au.channels.clamp(1, 2) };
            if ch_change || au.channels > 2 {
                a.extend(["-ac".into(), ch.to_string()]);
            }
            if rate_change {
                plan.notes.push(format!("采样率 {} → {} Hz", au.sample_rate, spec.audio_rate));
            }
            if ch_change {
                plan.notes.push(format!("声道 {} → {}", au.channels, spec.audio_channels));
            }
            a.extend(s(&["-c:a", "aac", "-b:a", if ch == 1 { "128k" } else { "192k" }]));
        } else {
            a.extend(s(&["-c:a", "copy"]));
            if facts.container.contains("mpegts") && au.codec == "aac" {
                a.extend(s(&["-bsf:a", "aac_adtstoasc"]));
            }
        }
    }

    // ---- 是否已经符合规格 ----
    let in_mp4 = facts.container.contains("mp4");
    plan.unchanged = !reencode && !tag_patch && !audio_changed && in_mp4 && plan.ext == "mp4";
    plan.args = a;
    if !reencode && !plan.unchanged && plan.notes.is_empty() {
        plan.notes.push("画面和声音都不需要处理，只重新封装成 MP4".into());
    }
    Ok(plan)
}

fn reasons_text(reasons: &[&str]) -> String {
    reasons
        .iter()
        .map(|r| match *r {
            "fps" => "帧率",
            "crop" => "去黑边",
            "rotation" => "旋转",
            "sar" => "像素比",
            "size" => "尺寸",
            "hdr" => "HDR 转 SDR",
            "range" => "色彩范围",
            "matrix" => "色彩空间",
            "levels" => "色阶",
            "lut" => "LUT",
            "color_match" => "分段色彩匹配",
            "pix" => "像素格式",
            "codec" => "编码",
            "container" => "封装",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::super::facts::parse;
    use super::super::spec::{preset, Crop, Loudness, VfrInfo};
    use super::*;
    use crate::media_tools::Encoders;

    fn caps(zscale: bool, gpl: bool) -> Caps {
        let mut f: Vec<&str> = vec![
            "scale",
            "fps",
            "crop",
            "pad",
            "overlay",
            "boxblur",
            "format",
            "setparams",
            "lut3d",
            "normalize",
            "cropdetect",
            "colorspace",
            "blend",
            "split",
        ];
        if zscale {
            f.extend(["zscale", "tonemap"]);
        }
        Caps {
            gpl,
            filters: f.iter().map(|s| s.to_string()).collect(),
            encoders: Encoders { h264: Some("libx264"), vp9: true, mp3: true, opus: true },
            hevc: gpl.then_some("libx265"),
            major: 6,
            ..Default::default()
        }
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "clearclip-vn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn arg_after<'a>(a: &'a [String], key: &str) -> Option<&'a str> {
        a.iter().position(|x| x == key).and_then(|i| a.get(i + 1)).map(String::as_str)
    }

    const PHONE_HLG: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mov':\n  Duration: 00:00:10.00, start: 0.000000, bitrate: 9000 kb/s\n  Stream #0:0[0x1](und): Video: hevc (Main 10), yuv420p10le(tv, bt2020nc/bt2020/arib-std-b67), 3840x2160, 8800 kb/s, 29.97 fps, 30 tbr, 600 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC), 44100 Hz, stereo, fltp, 123 kb/s (default)\n";
    const PLAIN: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'b.mp4':\n  Duration: 00:00:10.00, start: 0.000000, bitrate: 5000 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080, 4800 kb/s, 30 fps, 30 tbr, 15360 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC), 48000 Hz, stereo, fltp, 128 kb/s (default)\n";

    #[test]
    fn filter_paths_are_escaped_for_two_levels() {
        assert_eq!(filter_path(Path::new("/tmp/a.cube")), "/tmp/a.cube");
        assert_eq!(filter_path(Path::new(r"C:\Users\me\lut.cube")), r"C\\:/Users/me/lut.cube");
        assert_eq!(filter_path(Path::new("/a b/it's,[x];y.cube")), r"/a b/it\\\'s\,\[x\]\;y.cube");
    }

    #[test]
    fn fps_helpers() {
        assert_eq!(nearest_standard_fps(29.9), (30000, 1001));
        assert_eq!(nearest_standard_fps(30.0), (30, 1));
        assert_eq!(nearest_standard_fps(47.0), (50, 1));
        assert_eq!(nearest_standard_fps(120.0), (60, 1));
        assert_eq!(nearest_standard_fps(12.0), (24000, 1001));
        assert_eq!(fps_fraction(29.97), (30000, 1001));
        assert_eq!(fps_fraction(23.976), (24000, 1001));
        assert_eq!(fps_fraction(30.0), (30, 1));
        assert_eq!(fps_fraction(25.0), (25, 1));
        assert_eq!(frac_text((30000, 1001)), "30000/1001");
    }

    #[test]
    fn compliant_file_is_left_alone() {
        let f = parse(PLAIN);
        let p = build(Path::new("/m/b.mp4"), &preset("compat").unwrap(), &f, &Analysis::default(), &caps(true, true), &tmp()).unwrap();
        assert!(p.unchanged && p.video_copy, "{:?}", p);
        assert!(p.args.windows(2).any(|w| w == ["-c:v", "copy"]) && p.args.windows(2).any(|w| w == ["-c:a", "copy"]));
    }

    #[test]
    fn hdr_phone_video_is_tone_mapped_scaled_and_reencoded() {
        let f = parse(PHONE_HLG);
        let spec = preset("compat").unwrap();
        let p = build(Path::new("/m/a.mov"), &spec, &f, &Analysis::default(), &caps(true, true), &tmp()).unwrap();
        assert!(!p.unchanged && !p.video_copy);
        let g = arg_after(&p.args, "-filter_complex").unwrap();
        assert!(g.starts_with("[0:v:0]scale=-2:1080,zscale=tin=arib-std-b67"), "{g}");
        assert!(
            g.contains("tonemap=tonemap=hable") && g.ends_with("format=yuv420p,setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv[v]"),
            "{g}"
        );
        assert_eq!(arg_after(&p.args, "-map"), Some("[v]"));
        assert_eq!(arg_after(&p.args, "-c:v"), Some("libx264"));
        assert_eq!(arg_after(&p.args, "-crf"), Some("21"));
        assert_eq!(arg_after(&p.args, "-color_trc"), Some("bt709"));
        // 音频是 44.1 kHz AAC，“通用兼容”不改采样率 → 直接复制
        assert_eq!(arg_after(&p.args, "-c:a"), Some("copy"));
        assert!(p.notes.iter().any(|n| n.contains("HLG 转 SDR")) && p.notes.iter().any(|n| n.contains("缩小")), "{:?}", p.notes);
    }

    #[test]
    fn hdr_without_zscale_uses_the_builtin_lut() {
        let f = parse(PHONE_HLG);
        let dir = tmp();
        let p = build(Path::new("/m/a.mov"), &preset("compat").unwrap(), &f, &Analysis::default(), &caps(false, false), &dir).unwrap();
        let g = arg_after(&p.args, "-filter_complex").unwrap();
        assert!(g.contains("lut3d=file=") && g.contains("hdr-hlg.cube") && !g.contains("zscale"), "{g}");
        assert!(p.warnings.iter().any(|w| w.contains("没有 zscale")), "{:?}", p.warnings);
        assert_eq!(p.temp_files.len(), 1);
        assert!(std::fs::read_to_string(&p.temp_files[0]).unwrap().contains("LUT_3D_SIZE"));
        // 连 lut3d 也没有：不转换，并说明
        let mut c = caps(false, false);
        c.filters.remove("lut3d");
        let p = build(Path::new("/m/a.mov"), &preset("compat").unwrap(), &f, &Analysis::default(), &c, &dir).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("无法把 HDR 转成 SDR")), "{:?}", p.warnings);
    }

    #[test]
    fn vfr_and_black_bars_with_the_edit_preset() {
        let f = parse("Input #0, mov,mp4, from 'r.mp4':\n  Duration: 00:00:20.00, start: 0.0, bitrate: 3000 kb/s\n  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1080x2400, 3000 kb/s, 27.3 fps, 120 tbr, 90k tbn\n  Stream #0:1: Audio: aac (LC), 44100 Hz, stereo, fltp, 96 kb/s\n");
        let an = Analysis {
            vfr: Some(VfrInfo { variable: true, median_fps: 30.0, min_fps: 5.0, max_fps: 60.0, frames: 240 }),
            crop: Some(Crop { w: 1080, h: 2208, x: 0, y: 96, src_w: 1080, src_h: 2400 }),
            loudness: Some(Loudness { input_i: -24.0, input_tp: -3.0, input_lra: 6.0, input_thresh: -34.0, target_offset: 0.3 }),
            ..Default::default()
        };
        let p = build(Path::new("/m/r.mp4"), &preset("edit").unwrap(), &f, &an, &caps(true, true), &tmp()).unwrap();
        let g = arg_after(&p.args, "-filter_complex").unwrap();
        assert!(g.starts_with("[0:v:0]fps=30,crop=1080:2208:0:96,scale=1080:1920:force_original_aspect_ratio=decrease,pad=1080:1920"), "{g}");
        let af = arg_after(&p.args, "-af").unwrap();
        assert!(af.contains("aresample=async=1") && af.contains("measured_I=-24.00") && af.contains("linear=true"), "{af}");
        assert_eq!((arg_after(&p.args, "-ar"), arg_after(&p.args, "-c:a")), (Some("48000"), Some("aac")));
        assert_eq!(arg_after(&p.args, "-crf"), Some("18"), "高质量档");
        assert!(p.notes.iter().any(|n| n.contains("可变帧率转成固定 30 帧")) && p.notes.iter().any(|n| n.contains("去黑边")));
    }

    #[test]
    fn blur_fill_graph_is_wired_correctly() {
        let f = parse(PLAIN);
        let spec = NormSpec { size: "blur".into(), width: 1080, height: 1920, follow_orientation: false, ..Default::default() };
        let vg = video_graph(&spec, &f, &Analysis::default(), &caps(true, true), &tmp(), "yuv420p", false).unwrap();
        let g = vg.graph.unwrap();
        assert!(g.starts_with("[0:v:0]split[b1a][b1b];[b1a]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,boxblur=25:5[b1g];[b1b]scale=1080:1920:force_original_aspect_ratio=decrease[b1f];[b1g][b1f]overlay=(W-w)/2:(H-h)/2,format=yuv420p"), "{g}");
        assert_eq!((vg.out_w, vg.out_h), (1080, 1920));
        // 比例相同时只是缩放
        let spec = NormSpec { size: "blur".into(), width: 1280, height: 720, ..Default::default() };
        let g = video_graph(&spec, &f, &Analysis::default(), &caps(true, true), &tmp(), "yuv420p", false).unwrap().graph.unwrap();
        assert!(g.starts_with("[0:v:0]scale=1280:720,format"), "{g}");
    }

    #[test]
    fn lut_strength_blends_the_branch() {
        let dir = tmp();
        let lut = dir.join("look.cube");
        std::fs::write(&lut, "LUT_3D_SIZE 2\n").unwrap();
        let f = parse(PLAIN);
        let spec = NormSpec { size: "keep".into(), lut: Some(lut.to_string_lossy().into_owned()), lut_strength: 0.6, ..Default::default() };
        let g = video_graph(&spec, &f, &Analysis::default(), &caps(true, true), &dir, "yuv420p", false).unwrap().graph.unwrap();
        assert!(g.starts_with("[0:v:0]split[m1a][m1b];[m1b]scale=in_color_matrix=bt709"), "{g}");
        assert!(g.contains("[m1a][m1c]blend=all_mode=normal:all_opacity=0.600,format=yuv420p"), "{g}");
        let missing = NormSpec { lut: Some("/nope/x.cube".into()), ..Default::default() };
        assert!(video_graph(&missing, &f, &Analysis::default(), &caps(true, true), &dir, "yuv420p", false).is_err());
    }

    #[test]
    fn live_recording_is_remuxed_with_sync_fixes() {
        let f = parse("Input #0, flv, from 'live.flv':\n  Duration: N/A, start: 0.0, bitrate: N/A\n  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1280x720, 30 fps, 30 tbr, 1k tbn\n  Stream #0:1: Audio: aac (LC), 44100 Hz, stereo, fltp\n");
        let p = build(Path::new("/m/live.flv"), &preset("live").unwrap(), &f, &Analysis::default(), &caps(true, true), &tmp()).unwrap();
        assert!(p.video_copy && !p.unchanged, "视频没问题就不重新编码，但要转封装");
        assert_eq!(&p.args[..2], ["-fflags", "+genpts"]);
        assert!(arg_after(&p.args, "-af").unwrap().contains("aresample=async=1"));
        assert_eq!(arg_after(&p.args, "-c:a"), Some("aac"));
    }

    #[test]
    fn untagged_video_gets_its_tags_patched_without_reencoding() {
        let f = parse("Input #0, mov,mp4, from 'u.mp4':\n  Stream #0:0: Video: h264 (High), yuv420p, 1920x1080, 30 fps, 30 tbr, 15360 tbn\n  Stream #0:1: Audio: aac (LC), 48000 Hz, stereo, fltp\n");
        let p = build(Path::new("/m/u.mp4"), &preset("platform").unwrap(), &f, &Analysis::default(), &caps(true, true), &tmp()).unwrap();
        assert!(p.video_copy && !p.unchanged);
        let bsf = arg_after(&p.args, "-bsf:v").unwrap();
        assert!(bsf.starts_with("h264_metadata=colour_primaries=1:") && bsf.contains("video_full_range_flag=0"), "{bsf}");
    }

    #[test]
    fn full_range_and_bt601_are_converted() {
        let f = parse("Input #0, mov,mp4, from 's.mp4':\n  Stream #0:0: Video: h264 (High), yuvj420p(pc, bt470bg/bt470bg/smpte170m), 1280x720, 30 fps, 30 tbr, 15360 tbn\n");
        let vg = video_graph(&NormSpec { size: "keep".into(), ..Default::default() }, &f, &Analysis::default(), &caps(true, true), &tmp(), "yuv420p", false)
            .unwrap();
        let g = vg.graph.unwrap();
        assert!(g.contains("scale=in_range=pc:out_range=tv,colorspace=all=bt709:iall=bt470bg,format=yuv420p"), "{g}");
        // 标清素材保持 BT.601，不转换
        let sd = parse("Input #0, mov,mp4, from 's.mp4':\n  Stream #0:0: Video: h264 (High), yuv420p(tv, smpte170m/smpte170m/smpte170m), 640x480, 30 fps, 30 tbr, 15360 tbn\n  Stream #0:1: Audio: aac, 48000 Hz, stereo\n");
        let p = build(Path::new("/m/s.mp4"), &NormSpec { size: "keep".into(), ..Default::default() }, &sd, &Analysis::default(), &caps(true, true), &tmp())
            .unwrap();
        assert!(p.unchanged);
    }

    #[test]
    fn encoder_fallbacks_and_hevc() {
        let f = parse(PHONE_HLG);
        let mut c = caps(true, false);
        c.encoders.h264 = Some("h264_qsv");
        let p = build(Path::new("/m/a.mov"), &preset("compat").unwrap(), &f, &Analysis::default(), &c, &tmp()).unwrap();
        assert_eq!(arg_after(&p.args, "-c:v"), Some("h264_qsv"));
        assert!(arg_after(&p.args, "-filter_complex").unwrap().contains("format=nv12,setparams"), "QSV 要 nv12");
        // 没有 HEVC 编码器：回退 H.264 并说明
        let p = build(
            Path::new("/m/a.mov"),
            &NormSpec { codec: "hevc".into(), size: "keep".into(), ..Default::default() },
            &parse(PLAIN),
            &Analysis::default(),
            &caps(true, false),
            &tmp(),
        )
        .unwrap();
        assert_eq!(arg_after(&p.args, "-c:v"), Some("libx264"));
        assert!(p.warnings.iter().any(|w| w.contains("HEVC")), "{:?}", p.warnings);
        // 有 libx265
        let p = build(
            Path::new("/m/a.mov"),
            &NormSpec { codec: "hevc".into(), size: "keep".into(), ..Default::default() },
            &parse(PLAIN),
            &Analysis::default(),
            &caps(true, true),
            &tmp(),
        )
        .unwrap();
        assert_eq!((arg_after(&p.args, "-c:v"), arg_after(&p.args, "-tag:v")), (Some("libx265"), Some("hvc1")));
        // 没有任何 H.264 编码器 → VP9 + MKV
        let mut c = caps(true, false);
        c.encoders = Encoders { h264: None, vp9: true, mp3: false, opus: true };
        let p = build(Path::new("/m/a.mov"), &preset("compat").unwrap(), &f, &Analysis::default(), &c, &tmp()).unwrap();
        assert_eq!(p.ext, "mkv");
        assert!(p.warnings.iter().any(|w| w.contains("H.264")));
    }

    #[test]
    fn audio_only_files_are_rejected() {
        let f = parse("Input #0, mp3, from 'a.mp3':\n  Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 128 kb/s\n");
        assert!(build(Path::new("/m/a.mp3"), &NormSpec::default(), &f, &Analysis::default(), &caps(true, true), &tmp()).is_err());
    }

    #[test]
    fn audio_rules() {
        // 6 声道 → 立体声，采样率统一
        let f = parse("Input #0, matroska,webm, from 'm.mkv':\n  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080, 24 fps, 24 tbr, 1k tbn\n  Stream #0:1: Audio: ac3, 48000 Hz, 5.1(side), fltp, 448 kb/s\n");
        let spec = NormSpec { audio_channels: 2, audio_rate: 48000, ..preset("compat").unwrap() };
        let p = build(Path::new("/m/m.mkv"), &spec, &f, &Analysis::default(), &caps(true, true), &tmp()).unwrap();
        assert_eq!((arg_after(&p.args, "-ac"), arg_after(&p.args, "-c:a")), (Some("2"), Some("aac")));
        // 没有音轨的视频
        let silent = parse(
            "Input #0, mov,mp4, from 'q.mp4':\n  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080, 30 fps, 30 tbr, 15360 tbn\n",
        );
        let p =
            build(Path::new("/m/q.mp4"), &NormSpec { loudness: Some(-16.0), ..Default::default() }, &silent, &Analysis::default(), &caps(true, true), &tmp())
                .unwrap();
        assert!(!p.args.contains(&"-c:a".to_string()) && p.unchanged);
    }
}
