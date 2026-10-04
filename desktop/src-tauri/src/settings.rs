use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    /// 只用本地解析。
    Local,
    /// 只用远程 PHP 接口（`jxindex.php`）。
    Remote,
    /// 先本地解析，失败后改用远程接口。
    #[default]
    LocalThenRemote,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: String,
    pub subfolder_by_platform: bool,
    /// 可用变量：{author} {title} {date} {id} {platform}
    pub filename_template: String,
    pub concurrency: usize,
    pub skip_existing: bool,
    pub watch_clipboard: bool,
    pub auto_download: bool,
    pub notify_on_complete: bool,
    pub parse_mode: ParseMode,
    pub remote_endpoint: String,
    /// 旧版：平台 → Cookie 字符串。启动时迁移到加密的 Cookie 存储后清空。
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub cookies: HashMap<String, String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub cookie_updated_at: HashMap<String, i64>,
    /// `system` / `dark` / `light`
    pub theme: String,
    pub close_to_tray: bool,
    pub shortcut: String,
    pub check_update: bool,
    pub disclaimer_accepted: bool,
    /// 启动时自动继续未完成的任务（否则恢复为“已暂停”）
    pub auto_resume: bool,
    /// 网络错误时的自动重试次数
    pub max_retries: u32,
    /// 第一次重试前的等待秒数，之后每次翻倍
    pub retry_delay_secs: u64,
    /// 取消任务时保留已下载的部分
    pub keep_part_on_cancel: bool,
    /// 保存解析时的原始响应，用于排查问题
    pub record_samples: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: String::new(),
            subfolder_by_platform: true,
            filename_template: "{author}_{title}_{date}".into(),
            concurrency: 3,
            skip_existing: true,
            watch_clipboard: true,
            auto_download: false,
            notify_on_complete: true,
            parse_mode: ParseMode::LocalThenRemote,
            remote_endpoint: String::new(),
            cookies: HashMap::new(),
            cookie_updated_at: HashMap::new(),
            theme: "system".into(),
            close_to_tray: true,
            shortcut: "CommandOrControl+Shift+D".into(),
            check_update: true,
            disclaimer_accepted: false,
            auto_resume: false,
            max_retries: 3,
            retry_delay_secs: 2,
            keep_part_on_cancel: false,
            record_samples: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path, default_download_dir: &Path) -> Settings {
        let mut s: Settings = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        if s.download_dir.trim().is_empty() {
            s.download_dir = default_download_dir.to_string_lossy().into_owned();
        }
        s.normalize();
        s
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.concurrency = self.concurrency.clamp(1, 8);
        self.max_retries = self.max_retries.min(10);
        self.retry_delay_secs = self.retry_delay_secs.clamp(1, 60);
        if self.filename_template.trim().is_empty() {
            self.filename_template = Settings::default().filename_template;
        }
        self.remote_endpoint = self.remote_endpoint.trim().to_string();
    }

    pub fn download_root(&self) -> PathBuf {
        PathBuf::from(&self.download_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"concurrency": 5}"#).unwrap();
        assert_eq!(s.concurrency, 5);
        assert!(s.watch_clipboard);
        assert_eq!(s.parse_mode, ParseMode::LocalThenRemote);
    }

    #[test]
    fn normalize_clamps_concurrency() {
        let mut s = Settings { concurrency: 99, ..Settings::default() };
        s.normalize();
        assert_eq!(s.concurrency, 8);
    }
}
