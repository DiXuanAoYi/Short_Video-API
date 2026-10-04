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

/// 把视频轨和音频轨无损合并为一个文件（不重新编码）。
pub async fn merge(st: &AppState, inputs: &[PathBuf], output: &Path, final_path: &Path) -> AppResult<()> {
    let ffmpeg = find_ffmpeg(st).ok_or_else(ffmpeg_missing)?;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    for p in inputs {
        args.push("-i".into());
        args.push(p.to_string_lossy().into_owned());
    }
    for i in 0..inputs.len() {
        args.push("-map".into());
        args.push(format!("{i}"));
    }
    args.extend(["-c".into(), "copy".into()]);
    let muxer = muxer_for(final_path);
    if muxer == "mp4" {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    args.extend(["-f".into(), muxer.into(), output.to_string_lossy().into_owned()]);
    run_ffmpeg(&ffmpeg, &args).await
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
    fn muxer_from_extension() {
        assert_eq!(muxer_for(Path::new("/a/b.mp4")), "mp4");
        assert_eq!(muxer_for(Path::new("/a/b.MKV")), "matroska");
        assert_eq!(muxer_for(Path::new("/a/b.m4a")), "ipod");
        assert_eq!(muxer_for(Path::new("/a/b")), "mp4");
    }
}
