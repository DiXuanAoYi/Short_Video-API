//! 用真实的 ffmpeg 跑一遍：生成各种“问题素材”，按规格生成参数并执行，再读回输出检查。
//! 机器上没有 ffmpeg（或缺少需要的编码器）时对应的测试自动跳过。

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::analyze;
use super::build::{build, video_graph, NormPlan};
use super::facts::{self, Facts, Hdr};
use super::spec::{preset, Analysis, NormSpec};
use crate::media_tools::muxer_args;
use crate::postprocess::{base_args, run_ffmpeg_capture};
use crate::vidcaps::{self, Caps};

fn ffmpeg() -> Option<PathBuf> {
    std::process::Command::new("ffmpeg").arg("-version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from("ffmpeg"))
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "clearclip-vn-e2e-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// 生成素材：`ins` 是输入部分，`outs` 是输出部分。失败（缺编码器等）返回 None。
async fn gen(ff: &Path, d: &Path, name: &str, ins: &[&str], outs: &[&str]) -> Option<PathBuf> {
    let out = d.join(name);
    let mut a = strs(&["-hide_banner", "-loglevel", "error", "-y"]);
    a.extend(strs(ins));
    a.extend(strs(outs));
    a.push(out.to_string_lossy().into_owned());
    run_ffmpeg_capture(ff, &a, Duration::from_secs(120)).await.ok().map(|_| out)
}

const SRC: &str = "testsrc2=s=640x360:r=30:d=2";

async fn caps_of(ff: &Path) -> Caps {
    vidcaps::detect(ff).await
}

/// 按任务的流程（读取、分析、生成参数、运行）处理一个文件。
async fn normalize(ff: &Path, input: &Path, spec: &NormSpec, caps: &Caps, d: &Path) -> (PathBuf, NormPlan, Facts) {
    let facts = facts::read(ff, input).await.expect("facts");
    let mut an = Analysis::default();
    if spec.fps == "auto" || spec.fix_sync {
        an.vfr = analyze::detect_vfr(ff, input).await;
    }
    if spec.autocrop {
        an.crop = analyze::detect_crop(ff, input, &facts).await;
    }
    if let (Some(t), Some(_)) = (spec.loudness, &facts.audio) {
        an.loudness = analyze::measure_loudness(ff, input, t).await;
    }
    let plan = build(input, spec, &facts, &an, caps, &d.join("tmp")).expect("build");
    let out = d.join(format!("out.{}", plan.ext));
    let _ = std::fs::remove_file(&out);
    if !plan.unchanged {
        let mut a = base_args();
        a.extend(plan.args.clone());
        a.extend(muxer_args(plan.ext, &out));
        run_ffmpeg_capture(ff, &a, Duration::from_secs(300)).await.unwrap_or_else(|e| panic!("ffmpeg 失败：{e}\n参数：{a:?}"));
    }
    (out, plan, facts)
}

/// 取第一帧缩成 32×32 的灰度（0–255 全范围）。
async fn gray(ff: &Path, file: &Path, pre: &str) -> Vec<u8> {
    let vf = format!("{pre}scale=32:32:flags=area:in_range=tv:out_range=pc,format=gray");
    let a = strs(&["-hide_banner", "-loglevel", "error", "-i", &file.to_string_lossy(), "-vf", &vf, "-frames:v", "1", "-f", "rawvideo", "-"]);
    run_ffmpeg_capture(ff, &a, Duration::from_secs(60)).await.unwrap().0
}

fn mean(v: &[u8]) -> f64 {
    v.iter().map(|b| f64::from(*b)).sum::<f64>() / v.len().max(1) as f64
}

fn spread(v: &[u8]) -> f64 {
    f64::from(*v.iter().max().unwrap_or(&0)) - f64::from(*v.iter().min().unwrap_or(&0))
}

macro_rules! need {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => return,
        }
    };
}

async fn setup(name: &str) -> Option<(PathBuf, PathBuf, Caps)> {
    let ff = ffmpeg()?;
    let caps = caps_of(&ff).await;
    if caps.encoders.h264 != Some("libx264") {
        return None;
    }
    Some((ff, dir(name), caps))
}

async fn hdr_sample(ff: &Path, d: &Path, trc: &str) -> Option<PathBuf> {
    gen(
        ff,
        d,
        "hdr.mp4",
        &["-f", "lavfi", "-i", SRC, "-f", "lavfi", "-i", "sine=d=2"],
        &["-c:v", "libx264", "-pix_fmt", "yuv420p10le", "-color_primaries", "bt2020", "-color_trc", trc, "-colorspace", "bt2020nc", "-c:a", "aac", "-shortest"],
    )
    .await
}

#[tokio::test]
async fn hdr_is_tone_mapped_by_zscale_and_by_the_builtin_lut() {
    let (ff, d, caps) = need!(setup("hdr").await);
    for (trc, kind) in [("smpte2084", Hdr::Pq), ("arib-std-b67", Hdr::Hlg)] {
        let src = need!(hdr_sample(&ff, &d, trc).await);
        let f = facts::read(&ff, &src).await.unwrap();
        assert_eq!(f.video.as_ref().unwrap().hdr, kind);
        let spec = preset("compat").unwrap();

        let mut outs = vec![];
        let mut variants = vec![("zscale", caps.clone())];
        let mut lite = caps.clone();
        lite.filters.remove("zscale");
        lite.filters.remove("tonemap");
        variants.push(("lut", lite));
        for (name, c) in variants {
            if name == "zscale" && !c.can_tonemap() {
                continue;
            }
            let sub = d.join(format!("{trc}-{name}"));
            std::fs::create_dir_all(&sub).unwrap();
            let (out, plan, _) = normalize(&ff, &src, &spec, &c, &sub).await;
            assert!(!plan.unchanged && !plan.video_copy, "{name}");
            let o = facts::read(&ff, &out).await.unwrap();
            let v = o.video.unwrap();
            assert_eq!(
                (v.hdr, v.bit_depth, v.transfer.as_deref(), v.matrix.as_deref(), v.range.as_deref()),
                (Hdr::None, 8, Some("bt709"), Some("bt709"), Some("tv")),
                "{name}: {v:?}"
            );
            assert_eq!((v.width, v.height), (640, 360), "不超过 1080p 不缩放");
            assert!(o.audio.is_some(), "音频保留");
            // 用转成 RGB（超出范围的分量被截断）之后的亮度比较，这才是屏幕上看到的样子
            outs.push((name, gray(&ff, &out, "format=rgb24,").await));
        }
        if let [(_, a), (_, b)] = &outs[..] {
            let (ma, mb) = (mean(a), mean(b));
            assert!((ma - mb).abs() < 30.0, "{trc}：zscale 平均亮度 {ma:.0}，内置 LUT {mb:.0}，相差太大");
        }
    }
}

#[tokio::test]
async fn variable_frame_rate_becomes_constant() {
    let (ff, d, caps) = need!(setup("vfr").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "vfr.mp4",
            &["-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=6", "-f", "lavfi", "-i", "sine=d=6"],
            &["-vf", "select='lt(mod(n,10),3)'", "-fps_mode", "vfr", "-c:v", "libx264", "-c:a", "aac", "-shortest"]
        )
        .await
    );
    let before = analyze::detect_vfr(&ff, &src).await.unwrap();
    assert!(before.variable, "{before:?}");
    let spec = preset("screen").unwrap();
    let (out, plan, _) = normalize(&ff, &src, &spec, &caps, &d).await;
    assert!(plan.notes.iter().any(|n| n.contains("可变帧率转成固定")), "{:?}", plan.notes);
    let after = analyze::detect_vfr(&ff, &out).await.unwrap();
    assert!(!after.variable, "{after:?}");
    let fps = facts::read(&ff, &out).await.unwrap().video.unwrap().fps.unwrap();
    assert!([24.0, 25.0, 30.0, 50.0, 60.0, 23.976, 29.97, 59.94].iter().any(|s| (s - fps).abs() < 0.1), "{fps}");
}

#[tokio::test]
async fn black_bars_are_cropped() {
    let (ff, d, caps) = need!(setup("bars").await);
    let src = need!(gen(&ff, &d, "bars.mp4", &["-f", "lavfi", "-i", SRC], &["-vf", "pad=640:480:0:60:black", "-c:v", "libx264", "-pix_fmt", "yuv420p"]).await);
    let facts = facts::read(&ff, &src).await.unwrap();
    let c = analyze::detect_crop(&ff, &src, &facts).await.expect("检测到黑边");
    assert_eq!((c.w, c.x), (640, 0));
    assert!((i64::from(c.h) - 360).abs() <= 4 && (i64::from(c.y) - 60).abs() <= 4, "{c:?}");
    let (out, plan, _) = normalize(&ff, &src, &preset("platform").unwrap(), &caps, &d).await;
    assert!(plan.notes.iter().any(|n| n.contains("去黑边")));
    let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
    assert!((i64::from(v.height) - 360).abs() <= 4 && v.width == 640, "{}×{}", v.width, v.height);
}

#[tokio::test]
async fn live_recording_is_remuxed_synced_and_loudness_normalized() {
    let (ff, d, caps) = need!(setup("live").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "live.flv",
            &["-f", "lavfi", "-i", SRC, "-f", "lavfi", "-i", "sine=f=440:d=2,volume=0.05"],
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ar", "44100", "-shortest", "-f", "flv"]
        )
        .await
    );
    let (out, plan, f) = normalize(&ff, &src, &preset("live").unwrap(), &caps, &d).await;
    assert!(f.is_stream_container() && plan.video_copy && plan.ext == "mp4");
    let o = facts::read(&ff, &out).await.unwrap();
    assert!(o.container.contains("mp4"), "{}", o.container);
    assert_eq!(o.video.as_ref().unwrap().codec, "h264");
    let a = o.audio.unwrap();
    assert_eq!((a.codec.as_str(), a.sample_rate, a.channels), ("aac", 48000, 2));
    // 两遍响度标准化：输出的响度应接近 -16 LUFS
    let m = analyze::measure_loudness(&ff, &out, -16.0).await.expect("测得响度");
    assert!((m.input_i + 16.0).abs() < 2.0, "输出响度 {} LUFS", m.input_i);
}

#[tokio::test]
async fn rotation_is_kept_when_copying_and_baked_when_reencoding() {
    let (ff, d, caps) = need!(setup("rot").await);
    let plain = need!(
        gen(
            &ff,
            &d,
            "plain.mp4",
            &["-f", "lavfi", "-i", SRC, "-f", "lavfi", "-i", "sine=d=2"],
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]
        )
        .await
    );
    let rot = need!(gen(&ff, &d, "rot.mp4", &["-display_rotation", "90", "-i", &plain.to_string_lossy()], &["-c", "copy"]).await);
    let f = facts::read(&ff, &rot).await.unwrap();
    assert_eq!(f.video.as_ref().unwrap().rotation, 90, "{:?}", f.video);
    // 普通预设：已经符合规格，不动
    let (_, plan, _) = normalize(&ff, &rot, &preset("compat").unwrap(), &caps, &d).await;
    assert!(plan.unchanged || plan.video_copy, "{:?}", plan.notes);
    // 统一成竖屏 1080×1920：画面转正后缩放
    let sub = d.join("edit");
    std::fs::create_dir_all(&sub).unwrap();
    let (out, plan, _) = normalize(&ff, &rot, &preset("edit").unwrap(), &caps, &sub).await;
    assert!(plan.notes.iter().any(|n| n.contains("旋转标记")), "{:?}", plan.notes);
    let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
    assert_eq!((v.width, v.height, v.rotation), (1080, 1920, 0), "{v:?}");
}

#[tokio::test]
async fn full_range_is_converted_to_limited_bt709() {
    let (ff, d, caps) = need!(setup("range").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "full.mp4",
            &["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=30:d=1"],
            &["-vf", "format=yuvj420p", "-c:v", "libx264", "-pix_fmt", "yuvj420p"]
        )
        .await
    );
    let f = facts::read(&ff, &src).await.unwrap();
    assert!(f.video.as_ref().unwrap().full_range(), "{:?}", f.video);
    let (out, _, _) = normalize(&ff, &src, &NormSpec { size: "keep".into(), ..Default::default() }, &caps, &d).await;
    let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
    assert_eq!((v.pix_fmt.as_str(), v.range.as_deref(), v.matrix.as_deref()), ("yuv420p", Some("tv"), Some("bt709")), "{v:?}");
}

#[tokio::test]
async fn landscape_becomes_vertical_with_blurred_background() {
    let (ff, d, caps) = need!(setup("blur").await);
    let src = need!(gen(&ff, &d, "land.mp4", &["-f", "lavfi", "-i", SRC], &["-c:v", "libx264", "-pix_fmt", "yuv420p"]).await);
    let (out, _, _) = normalize(&ff, &src, &preset("shorts").unwrap(), &caps, &d).await;
    let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
    assert_eq!((v.width, v.height), (1080, 1920));
    // 上方补边区域不是纯黑（模糊背景）
    let top = gray(&ff, &out, "crop=1080:300:0:0,").await;
    assert!(mean(&top) > 12.0, "上方应是模糊背景，平均亮度 {}", mean(&top));
}

const INVERT_CUBE: &str = "LUT_3D_SIZE 2\n1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n";

#[tokio::test]
async fn lut_is_applied_with_strength_and_awkward_paths() {
    let (ff, d, caps) = need!(setup("lut").await);
    let src = need!(gen(&ff, &d, "src.mp4", &["-f", "lavfi", "-i", SRC], &["-c:v", "libx264", "-pix_fmt", "yuv420p"]).await);
    // 路径里有空格、逗号、冒号、单引号、方括号：滤镜参数的转义要对
    let odd = d.join("a b,c:d'e [f]");
    std::fs::create_dir_all(&odd).unwrap();
    let lut = odd.join("invert.cube");
    std::fs::write(&lut, INVERT_CUBE).unwrap();
    let base = mean(&gray(&ff, &src, "").await);
    let mut got = vec![];
    for strength in [1.0, 0.5] {
        let spec = NormSpec { size: "keep".into(), lut: Some(lut.to_string_lossy().into_owned()), lut_strength: strength, ..Default::default() };
        let sub = d.join(format!("s{}", (strength * 10.0) as u32));
        std::fs::create_dir_all(&sub).unwrap();
        let (out, plan, _) = normalize(&ff, &src, &spec, &caps, &sub).await;
        assert!(plan.notes.iter().any(|n| n.contains("应用 LUT")));
        got.push(mean(&gray(&ff, &out, "").await));
    }
    assert!((got[0] - (255.0 - base)).abs() < 40.0, "反相后的平均亮度应接近 255-{base:.0}，实际 {:.0}", got[0]);
    let mid = (base + got[0]) / 2.0;
    assert!((got[1] - mid).abs() < 25.0, "一半强度应在原图和反相之间：{base:.0} / {:.0} / {:.0}", got[1], got[0]);
}

#[tokio::test]
async fn auto_levels_stretch_a_flat_picture() {
    let (ff, d, caps) = need!(setup("levels").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "flat.mp4",
            &["-f", "lavfi", "-i", SRC],
            &["-vf", "format=gray,eq=contrast=0.35,format=yuv420p", "-c:v", "libx264", "-pix_fmt", "yuv420p"]
        )
        .await
    );
    let before = spread(&gray(&ff, &src, "").await);
    let spec = NormSpec { size: "keep".into(), levels: 1.0, ..Default::default() };
    let (out, plan, _) = normalize(&ff, &src, &spec, &caps, &d).await;
    assert!(plan.notes.iter().any(|n| n.contains("自动色阶")));
    let after = spread(&gray(&ff, &out, "").await);
    assert!(after > before * 1.25, "对比度应被拉开：{before} → {after}");
}

#[tokio::test]
async fn hevc_output_when_an_encoder_exists() {
    let (ff, d, caps) = need!(setup("hevc").await);
    need!(caps.hevc);
    let src = need!(
        gen(
            &ff,
            &d,
            "src.mp4",
            &["-f", "lavfi", "-i", SRC, "-f", "lavfi", "-i", "sine=d=2"],
            &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]
        )
        .await
    );
    let (out, _, _) = normalize(&ff, &src, &preset("archive").unwrap(), &caps, &d).await;
    assert_eq!(facts::read(&ff, &out).await.unwrap().video.unwrap().codec, "hevc");
}

#[tokio::test]
async fn untagged_video_is_tagged_without_reencoding() {
    let (ff, d, caps) = need!(setup("tags").await);
    let src = need!(gen(&ff, &d, "untagged.mp4", &["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=30:d=1"], &["-c:v", "libx264", "-pix_fmt", "yuv420p"]).await);
    let before = facts::read(&ff, &src).await.unwrap().video.unwrap();
    assert!(before.untagged());
    let (out, plan, _) = normalize(&ff, &src, &preset("platform").unwrap(), &caps, &d).await;
    assert!(plan.video_copy && plan.notes.iter().any(|n| n.contains("补写了色彩标记")), "{:?}", plan.notes);
    let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
    assert_eq!((v.matrix.as_deref(), v.transfer.as_deref()), (Some("bt709"), Some("bt709")), "{v:?}");
    assert_eq!(v.codec, "h264");
}

#[tokio::test]
async fn preview_frames_are_written() {
    let (ff, d, caps) = need!(setup("preview").await);
    let src = need!(hdr_sample(&ff, &d, "smpte2084").await);
    let facts = facts::read(&ff, &src).await.unwrap();
    let vg = video_graph(&preset("compat").unwrap(), &facts, &Analysis::default(), &caps, &d.join("tmp"), "yuv420p", true).unwrap();
    let (before, after) = analyze::preview(&ff, &src, 500, vg.graph.as_deref(), &d, "k1").await.unwrap();
    for p in [&before, &after] {
        assert!(std::fs::metadata(p).unwrap().len() > 500, "{p:?}");
    }
    // 超出视频长度
    assert!(analyze::preview(&ff, &src, 600_000, vg.graph.as_deref(), &d, "k2").await.is_err());
}

#[tokio::test]
async fn stabilization_runs_with_vidstab_and_with_deshake() {
    use super::job::stabilize_plan;
    let (ff, d, caps) = need!(setup("stab").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "shaky.mp4",
            &["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=3"],
            &["-vf", "crop=580:300:30+12*sin(n/2.5):30+12*cos(n/3.1)", "-c:v", "libx264", "-pix_fmt", "yuv420p"]
        )
        .await
    );
    let facts = facts::read(&ff, &src).await.unwrap();
    let mut variants = vec![("vidstab", caps.clone())];
    let mut no_vidstab = caps.clone();
    no_vidstab.filters.remove("vidstabdetect");
    no_vidstab.filters.remove("vidstabtransform");
    variants.push(("deshake", no_vidstab));
    for (name, c) in variants {
        if name == "vidstab" && !c.vidstab() {
            continue;
        }
        let sub = d.join(name);
        std::fs::create_dir_all(&sub).unwrap();
        let trf = sub.join("s.trf");
        let plan = stabilize_plan(&src, "normal", &trf, &c, &facts).unwrap();
        assert_eq!(plan.pass1.is_some(), name == "vidstab");
        if let Some(p1) = &plan.pass1 {
            let mut a = base_args();
            a.extend(p1.clone());
            run_ffmpeg_capture(&ff, &a, Duration::from_secs(120)).await.unwrap_or_else(|e| panic!("{e}\n{a:?}"));
            assert!(std::fs::metadata(&trf).unwrap().len() > 0);
        }
        let out = sub.join("out.mp4");
        let mut a = base_args();
        a.extend(plan.pass2.clone());
        a.extend(muxer_args(plan.ext, &out));
        run_ffmpeg_capture(&ff, &a, Duration::from_secs(120)).await.unwrap_or_else(|e| panic!("{e}\n{a:?}"));
        let v = facts::read(&ff, &out).await.unwrap().video.unwrap();
        assert_eq!((v.width, v.height), (580, 300), "{name}");
    }
    // 两种滤镜都没有：明确报错
    let mut none = caps.clone();
    for f in ["vidstabdetect", "vidstabtransform", "deshake"] {
        none.filters.remove(f);
    }
    assert!(stabilize_plan(&src, "normal", &d.join("x.trf"), &none, &facts).is_err());
}

#[tokio::test]
async fn already_compliant_files_are_reported_unchanged() {
    let (ff, d, caps) = need!(setup("unchanged").await);
    let src = need!(
        gen(
            &ff,
            &d,
            "ok.mp4",
            &["-f", "lavfi", "-i", SRC, "-f", "lavfi", "-i", "sine=d=2"],
            &[
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_range",
                "tv",
                "-c:a",
                "aac",
                "-shortest"
            ]
        )
        .await
    );
    let (_, plan, _) = normalize(&ff, &src, &preset("compat").unwrap(), &caps, &d).await;
    assert!(plan.unchanged, "{:?}", plan.notes);
}
