//! 后处理：用 ffmpeg 合并音视频、转封装、提取音频、写入元数据。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::error::{AppError, AppResult, ErrorKind};
use crate::AppState;

/// ffmpeg 的位置：优先用程序管理的组件目录，其次系统 PATH。
pub fn find_ffmpeg(st: &AppState) -> Option<PathBuf> {
    crate::tools::resolve(st, crate::tools::Tool::Ffmpeg)
}

/// 由目标文件扩展名推断 ffmpeg 的输出格式（输出到 .part 临时文件时必须显式指定）。
pub fn muxer_for(final_path: &Path) -> &'static str {
    match final_path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mkv") => "matroska",
        Some("webm") => "webm",
        Some("m4a") => "ipod",
        Some("mp3") => "mp3",
        Some("ts") => "mpegts",
        Some("flv") => "flv",
        _ => "mp4",
    }
}

pub fn ffmpeg_missing() -> AppError {
    AppError::new(ErrorKind::NeedUpdate, "需要 ffmpeg 才能合并音视频。请在“设置 → 组件”中安装 ffmpeg 后重试。")
}

fn base_args() -> Vec<String> {
    vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()]
}

/// 输出参数：TS 里的 AAC 写入 MP4 时需要转换为 ASC 格式；MP4 把索引移到文件头，便于边下边播。
fn output_args(args: &mut Vec<String>, output: &Path, final_path: &Path, aac_fix: bool) {
    let muxer = muxer_for(final_path);
    if aac_fix && matches!(muxer, "mp4" | "ipod") {
        args.extend(["-bsf:a".into(), "aac_adtstoasc".into()]);
    }
    if muxer == "mp4" {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    args.extend(["-f".into(), muxer.into(), output.to_string_lossy().into_owned()]);
}

/// 把视频轨和音频轨无损合并为一个文件（不重新编码）。`aac_fix` 用于来自 m3u8（TS）的输入。
pub async fn merge(st: &AppState, inputs: &[PathBuf], output: &Path, final_path: &Path, aac_fix: bool) -> AppResult<()> {
    let ffmpeg = find_ffmpeg(st).ok_or_else(ffmpeg_missing)?;
    let mut args = base_args();
    for p in inputs {
        args.push("-i".into());
        args.push(p.to_string_lossy().into_owned());
    }
    for i in 0..inputs.len() {
        args.push("-map".into());
        args.push(format!("{i}"));
    }
    args.extend(["-c".into(), "copy".into()]);
    output_args(&mut args, output, final_path, aac_fix);
    run_ffmpeg(&ffmpeg, &args).await
}

/// 无损转封装（如 m3u8 拼接出的 TS → MP4）。
pub async fn remux(st: &AppState, input: &Path, output: &Path, final_path: &Path, aac_fix: bool) -> AppResult<()> {
    let ffmpeg = find_ffmpeg(st).ok_or_else(ffmpeg_missing)?;
    let mut args = base_args();
    args.extend(["-i".into(), input.to_string_lossy().into_owned(), "-map".into(), "0".into(), "-c".into(), "copy".into()]);
    output_args(&mut args, output, final_path, aac_fix);
    run_ffmpeg(&ffmpeg, &args).await
}

/// 提取音频的编码参数：m4a 先尝试直接复制 AAC 音轨，mp3 用 LAME 高质量 VBR。
pub fn audio_codec_args(fmt: &str, copy: bool) -> Vec<String> {
    match (fmt, copy) {
        ("m4a", true) => vec!["-c:a".into(), "copy".into()],
        ("m4a", false) => vec!["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()],
        ("opus", _) => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), "128k".into()],
        ("flac", _) => vec!["-c:a".into(), "flac".into()],
        ("wav", _) => vec!["-c:a".into(), "pcm_s16le".into()],
        _ => vec!["-c:a".into(), "libmp3lame".into(), "-q:a".into(), "2".into()],
    }
}

/// 从视频（或其他音频）中提取音频为 `fmt` 格式。
pub async fn extract_audio(st: &AppState, input: &Path, output: &Path, fmt: &str) -> AppResult<()> {
    let ffmpeg = find_ffmpeg(st).ok_or_else(|| AppError::new(ErrorKind::NeedUpdate, "需要 ffmpeg 才能提取音频。请在“设置 → 组件”中安装 ffmpeg 后重试。"))?;
    let muxer = match fmt {
        "m4a" => "ipod",
        "opus" => "ogg",
        "flac" => "flac",
        "wav" => "wav",
        _ => "mp3",
    };
    let run = |copy: bool| {
        let mut args = base_args();
        args.extend(["-i".into(), input.to_string_lossy().into_owned(), "-vn".into(), "-map".into(), "0:a:0".into()]);
        args.extend(audio_codec_args(fmt, copy));
        args.extend(["-f".into(), muxer.into(), output.to_string_lossy().into_owned()]);
        args
    };
    if fmt == "m4a" && run_ffmpeg(&ffmpeg, &run(true)).await.is_ok() {
        return Ok(());
    }
    run_ffmpeg(&ffmpeg, &run(false)).await
}

/// 把 Pixiv 动图的帧压缩包合成为视频。依次尝试 H.264（MP4）、VP9（WebM）、GIF，
/// 返回实际使用的扩展名（LGPL 版 ffmpeg 不含 x264，会落到 WebM）。
pub async fn ugoira(st: &AppState, zip_path: &Path, frames: &[(String, u64)], output: &Path) -> AppResult<&'static str> {
    let ffmpeg = find_ffmpeg(st)
        .ok_or_else(|| AppError::new(ErrorKind::NeedUpdate, "需要 ffmpeg 才能把动图合成为视频。请在“设置 → 组件”中安装 ffmpeg，或改为下载原始帧压缩包。"))?;
    let mut dir = output.as_os_str().to_owned();
    dir.push(".frames");
    let dir = PathBuf::from(dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let result = async {
        let zp = zip_path.to_path_buf();
        let d = dir.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&zp)?).map_err(|e| AppError::msg(format!("动图压缩包损坏：{e}")))?;
            for i in 0..zip.len() {
                let mut f = zip.by_index(i).map_err(|e| AppError::msg(e.to_string()))?;
                let Some(name) = f.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_owned())) else { continue };
                let mut out = std::fs::File::create(d.join(name))?;
                std::io::copy(&mut f, &mut out)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
        let list = dir.join("frames.ffconcat");
        std::fs::write(&list, ffconcat(frames))?;
        let base = |codec: &[&str], muxer: &str| {
            let mut a = base_args();
            a.extend(["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.to_string_lossy().into_owned()]);
            a.extend(codec.iter().map(|s| s.to_string()));
            a.extend(["-f".into(), muxer.into(), output.to_string_lossy().into_owned()]);
            a
        };
        let even = "pad=ceil(iw/2)*2:ceil(ih/2)*2";
        let attempts: [(&[&str], &str, &'static str); 3] = [
            (&["-vsync", "vfr", "-vf", even, "-pix_fmt", "yuv420p", "-c:v", "libx264", "-crf", "18", "-movflags", "+faststart"], "mp4", "mp4"),
            (&["-vsync", "vfr", "-vf", even, "-pix_fmt", "yuv420p", "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "24"], "webm", "webm"),
            (&["-vf", "split[a][b];[a]palettegen[p];[b][p]paletteuse", "-loop", "0"], "gif", "gif"),
        ];
        let mut last = AppError::msg("动图合成失败");
        for (codec, muxer, ext) in attempts {
            match run_ffmpeg(&ffmpeg, &base(codec, muxer)).await {
                Ok(()) => return Ok(ext),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// ffconcat 帧列表：每帧的显示时长；最后一帧重复一次，否则它的时长会被忽略。
pub fn ffconcat(frames: &[(String, u64)]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for (file, delay) in frames {
        s.push_str(&format!("file '{}'\nduration {:.3}\n", file.replace('\'', ""), *delay as f64 / 1000.0));
    }
    if let Some((last, _)) = frames.last() {
        s.push_str(&format!("file '{}'\n", last.replace('\'', "")));
    }
    s
}

pub async fn run_ffmpeg(ffmpeg: &Path, args: &[String]) -> AppResult<()> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // 不弹出控制台窗口
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().await.map_err(|e| AppError::new(ErrorKind::NeedUpdate, format!("无法运行 ffmpeg：{e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
        Err(AppError::msg(format!("ffmpeg 处理失败：{tail}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffconcat_list() {
        let s = ffconcat(&[("000000.jpg".into(), 80), ("000001.jpg".into(), 1200)]);
        assert_eq!(s, "ffconcat version 1.0\nfile '000000.jpg'\nduration 0.080\nfile '000001.jpg'\nduration 1.200\nfile '000001.jpg'\n");
    }

    #[test]
    fn muxer_from_extension() {
        assert_eq!(muxer_for(Path::new("/a/b.mp4")), "mp4");
        assert_eq!(muxer_for(Path::new("/a/b.MKV")), "matroska");
        assert_eq!(muxer_for(Path::new("/a/b.m4a")), "ipod");
        assert_eq!(muxer_for(Path::new("/a/b")), "mp4");
    }
}
