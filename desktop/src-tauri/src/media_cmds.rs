//! 媒体工具箱和字幕工具的前端命令。

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult, ErrorKind};
use crate::media_tools::{self, JobSnap, MediaInfoLite, ToolJob};
use crate::subtitle::DanmakuLayout;
use crate::{postprocess, subtitle_tools, AppState};

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub fn media_job_start(app: AppHandle, job: ToolJob) -> AppResult<u64> {
    media_tools::start(&app, job)
}

#[tauri::command]
pub fn media_jobs(state: St<'_>) -> Vec<JobSnap> {
    state.media_jobs.snapshot()
}

#[tauri::command]
pub fn media_job_cancel(app: AppHandle, state: St<'_>, id: u64) {
    use tauri::Emitter;
    state.media_jobs.cancel(id);
    let _ = app.emit(media_tools::EVT_JOBS, state.media_jobs.snapshot());
}

#[tauri::command]
pub fn media_jobs_clear(app: AppHandle, state: St<'_>) {
    use tauri::Emitter;
    state.media_jobs.clear_finished();
    let _ = app.emit(media_tools::EVT_JOBS, state.media_jobs.snapshot());
}

/// 读取文件的时长、画面尺寸和标签。
#[tauri::command]
pub async fn media_info(state: St<'_>, path: String) -> AppResult<MediaInfoLite> {
    let ff = postprocess::find_ffmpeg(&state).ok_or_else(|| AppError::new(ErrorKind::NeedUpdate, "需要 ffmpeg。请在“设置 → 组件”中安装 ffmpeg 后重试。"))?;
    if !Path::new(&path).is_file() {
        return Err(AppError::not_found("找不到文件。"));
    }
    let p = postprocess::probe(&ff, Path::new(&path)).await;
    if let Some(e) = &p.error {
        return Err(AppError::invalid(format!("无法读取这个文件：{e}")));
    }
    Ok(p.into())
}

// ---------- 字幕工具 ----------

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SubOp {
    Shift {
        offset_ms: i64,
    },
    Rescale {
        factor: f64,
    },
    Align {
        a_from: u64,
        a_to: u64,
        b_from: u64,
        b_to: u64,
    },
    /// 双语合并：`second` 是另一个字幕文件，文字放在下面
    Merge {
        second: String,
    },
    Clean {
        drop_hearing: bool,
    },
    Convert,
    /// B站弹幕 XML 转 ASS（使用“设置”里的弹幕样式）。宽高留空按 16:9
    Danmaku {
        width: Option<u32>,
        height: Option<u32>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubJob {
    pub input: String,
    #[serde(flatten)]
    pub op: SubOp,
    /// 输出格式 srt / vtt / ass，默认 srt（输入是 vtt 时为 vtt）
    pub format: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubResult {
    pub output: String,
    pub count: usize,
}

fn run_sub(job: &SubJob, danmaku: &crate::settings::DanmakuStyle) -> AppResult<SubResult> {
    let input = Path::new(&job.input);
    if !input.is_file() {
        return Err(AppError::not_found("找不到字幕文件。"));
    }
    let in_ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if let SubOp::Danmaku { width, height } = &job.op {
        if in_ext != "xml" {
            return Err(AppError::invalid("请选择 B站弹幕的 XML 文件。"));
        }
        let xml = std::fs::read_to_string(input)?;
        let layout = DanmakuLayout::for_video(*width, *height).styled(danmaku);
        let ass = crate::subtitle::danmaku_xml_to_ass_font(&xml, layout, None, &danmaku.font)?;
        let out = subtitle_tools::output_path(input, "弹幕", "ass");
        std::fs::write(&out, ass)?;
        return Ok(SubResult { output: out.to_string_lossy().into_owned(), count: 0 });
    }
    let cues = subtitle_tools::read(input)?;
    let (cues, tag) = match &job.op {
        SubOp::Shift { offset_ms } => (subtitle_tools::shift(&cues, *offset_ms), "平移"),
        SubOp::Rescale { factor } => (subtitle_tools::rescale(&cues, *factor), "缩放"),
        SubOp::Align { a_from, a_to, b_from, b_to } => (subtitle_tools::align(&cues, *a_from, *a_to, *b_from, *b_to)?, "校准"),
        SubOp::Merge { second } => (subtitle_tools::merge_bilingual(&cues, &subtitle_tools::read(Path::new(second))?), "双语"),
        SubOp::Clean { drop_hearing } => (subtitle_tools::clean(&cues, *drop_hearing), "清理"),
        SubOp::Convert => (cues, "转换"),
        SubOp::Danmaku { .. } => unreachable!(),
    };
    if cues.is_empty() {
        return Err(AppError::invalid("处理后没有剩下任何字幕。"));
    }
    let fmt = job.format.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| if in_ext == "vtt" { "vtt".into() } else { "srt".into() });
    let text = subtitle_tools::write_as(&cues, &fmt)?;
    let out = subtitle_tools::output_path(input, tag, &fmt);
    std::fs::write(&out, text)?;
    Ok(SubResult { output: out.to_string_lossy().into_owned(), count: cues.len() })
}

#[tauri::command]
pub async fn subtitle_tool(state: St<'_>, job: SubJob) -> AppResult<SubResult> {
    let style = state.settings().danmaku;
    tokio::task::spawn_blocking(move || run_sub(&job, &style)).await.map_err(|e| AppError::msg(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "clearclip-mc-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn job(json: &str) -> SubJob {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn subtitle_jobs_write_new_files_next_to_the_input() {
        let dir = temp("sub");
        let zh = dir.join("课.zh.srt");
        let en = dir.join("课.en.srt");
        std::fs::write(&zh, "1\n00:00:01,000 --> 00:00:03,000\n你好\n\n2\n00:00:05,000 --> 00:00:06,000\n再见\n\n").unwrap();
        std::fs::write(&en, "1\n00:00:01,200 --> 00:00:03,100\nHello\n\n").unwrap();
        let style = crate::settings::DanmakuStyle::default();
        let zs = zh.to_string_lossy().replace('\\', "\\\\");
        let es = en.to_string_lossy().replace('\\', "\\\\");
        let r = run_sub(&job(&format!(r#"{{"input":"{zs}","op":"shift","offsetMs":-500}}"#)), &style).unwrap();
        assert!(r.output.ends_with("课.zh.平移.srt"), "{}", r.output);
        let shifted = std::fs::read_to_string(&r.output).unwrap();
        assert!(shifted.contains("00:00:00,500 --> 00:00:02,500"), "{shifted}");
        let r = run_sub(&job(&format!(r#"{{"input":"{zs}","op":"merge","second":"{es}","format":"ass"}}"#)), &style).unwrap();
        let ass = std::fs::read_to_string(&r.output).unwrap();
        assert!(ass.contains("你好\\NHello"), "{ass}");
        assert_eq!(r.count, 2);
        let r = run_sub(&job(&format!(r#"{{"input":"{zs}","op":"convert","format":"vtt"}}"#)), &style).unwrap();
        assert!(std::fs::read_to_string(&r.output).unwrap().starts_with("WEBVTT"));
        // 弹幕
        let xml = dir.join("d.xml");
        std::fs::write(&xml, r#"<i><d p="1,1,25,16777215,0,0,0,0">hi</d></i>"#).unwrap();
        let xs = xml.to_string_lossy().replace('\\', "\\\\");
        let r = run_sub(
            &job(&format!(r#"{{"input":"{xs}","op":"danmaku","width":1080,"height":1920}}"#)),
            &crate::settings::DanmakuStyle { font_size: 50, ..style.clone() },
        )
        .unwrap();
        let ass = std::fs::read_to_string(&r.output).unwrap();
        assert!(ass.contains("PlayResX: 608") && ass.contains("Danmaku,Microsoft YaHei,50"), "{ass}");
        // 错误情况
        assert!(run_sub(&job(&format!(r#"{{"input":"{zs}","op":"danmaku"}}"#)), &style).is_err(), "not an xml");
        assert!(run_sub(&job(r#"{"input":"/nope.srt","op":"convert"}"#), &style).is_err());
        assert!(run_sub(&job(&format!(r#"{{"input":"{zs}","op":"shift","offsetMs":-99999999}}"#)), &style).is_err(), "nothing left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
