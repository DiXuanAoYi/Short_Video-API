//! 外部组件（yt-dlp、ffmpeg）的查找。安装、校验、更新见阶段 3 的组件管理。

use std::path::{Path, PathBuf};

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
}

impl Tool {
    pub fn bin_name(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Tool::YtDlp, true) => "yt-dlp.exe",
            (Tool::YtDlp, false) => "yt-dlp",
            (Tool::Ffmpeg, true) => "ffmpeg.exe",
            (Tool::Ffmpeg, false) => "ffmpeg",
        }
    }
}

/// 组件位置：优先使用程序管理的组件目录，其次系统 PATH。
pub fn resolve(st: &AppState, tool: Tool) -> Option<PathBuf> {
    let managed = st.tools_dir.join(tool.bin_name());
    if managed.is_file() {
        return Some(managed);
    }
    find_in_path(tool.bin_name())
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_shell_in_path() {
        if cfg!(unix) {
            assert!(find_in_path("sh").is_some());
        }
        assert!(find_in_path("definitely-not-a-real-binary-xyz").is_none());
    }
}
