//! 根据命名模板生成文件名和保存路径。

use std::path::{Path, PathBuf};

use crate::model::{Asset, AssetKind, MediaInfo};

const MAX_TITLE_CHARS: usize = 60;
const MAX_NAME_CHARS: usize = 100;

/// 渲染模板，变量：{author} {title} {date} {id} {platform}
pub fn render_base(template: &str, info: &MediaInfo) -> String {
    let date = info.published_at.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)).unwrap_or_else(chrono::Utc::now).format("%Y%m%d").to_string();
    let title: String = clean_title(&info.title).chars().take(MAX_TITLE_CHARS).collect();
    let raw = template
        .replace("{author}", &info.author)
        .replace("{title}", &title)
        .replace("{date}", &date)
        .replace("{id}", &info.id)
        .replace("{platform}", &info.platform_name);
    let mut name: String = sanitize(&raw).chars().take(MAX_NAME_CHARS).collect();
    name = name.trim_matches(|c: char| c == '_' || c == '.' || c == ' ' || c == '-').to_string();
    if name.is_empty() {
        name = sanitize(&info.id);
    }
    if name.is_empty() {
        name = "clearclip".into();
    }
    avoid_reserved(name)
}

/// Windows 不允许 CON、NUL、COM1 等作为文件名（不论扩展名），在后面加下划线。
pub fn avoid_reserved(name: String) -> String {
    let stem = name.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0');
    if reserved {
        format!("{name}_")
    } else {
        name
    }
}

/// Windows 传统路径上限约 260 字符。完整路径过长时缩短文件名主体（保留后缀）。
pub fn fit_path(dir: &Path, base: &str, asset: &Asset, max_len: usize) -> PathBuf {
    let mut base: String = base.to_string();
    loop {
        let path = dir.join(file_name(&base, asset));
        // 预留 ".part" 和 " (99)" 的长度
        if path.as_os_str().len() + 10 <= max_len || base.chars().count() <= 8 {
            return path;
        }
        let keep = base.chars().count().saturating_sub(4).max(8);
        base = base.chars().take(keep).collect::<String>().trim_end().to_string();
    }
}

/// 资源对应的文件名（不含目录）。
pub fn file_name(base: &str, asset: &Asset) -> String {
    let suffix = match asset.kind {
        AssetKind::Video => String::new(),
        AssetKind::Image => format!("_{:02}", asset.index.unwrap_or(0) + 1),
        AssetKind::Audio => "_music".into(),
        AssetKind::Cover => "_cover".into(),
    };
    format!("{base}{suffix}.{}", asset.ext)
}

pub fn target_dir(root: &Path, info: &MediaInfo, by_platform: bool) -> PathBuf {
    if by_platform && !info.platform_name.is_empty() {
        root.join(sanitize(&info.platform_name))
    } else {
        root.to_path_buf()
    }
}

/// 如果文件已存在或已被其他任务占用，在文件名后追加 (1)、(2)…
pub fn unique_path(path: PathBuf, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    if !taken(&path) {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    for n in 1.. {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(name);
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// 去掉话题标签和多余空白，让文件名更短。
fn clean_title(title: &str) -> String {
    let no_tags: Vec<&str> = title.split_whitespace().filter(|w| !w.starts_with('#') && !w.starts_with('@')).collect();
    let joined = no_tags.join(" ");
    if joined.trim().is_empty() {
        title.trim().to_string()
    } else {
        joined
    }
}

/// 替换各系统文件名中不允许的字符。
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaKind;

    fn info() -> MediaInfo {
        MediaInfo {
            platform: "douyin".into(),
            platform_name: "抖音".into(),
            id: "742".into(),
            source_url: String::new(),
            title: "秋天第一锅/板栗焖鸡?\n #家常菜 @山野".into(),
            author: "山野厨房".into(),
            cover: None,
            duration_ms: None,
            kind: MediaKind::Video,
            width: None,
            height: None,
            published_at: Some(1_727_856_000),
            assets: vec![],
            entries: vec![],
            series: None,
            extractor: None,
        }
    }

    #[test]
    fn renders_template_and_sanitizes() {
        assert_eq!(render_base("{author}_{title}_{date}", &info()), "山野厨房_秋天第一锅_板栗焖鸡__20241002");
        assert_eq!(render_base("{platform}-{id}", &info()), "抖音-742");
    }

    #[test]
    fn empty_result_falls_back_to_id() {
        let mut i = info();
        i.author.clear();
        assert_eq!(render_base("{author}", &i), "742");
    }

    #[test]
    fn file_names_per_asset_kind() {
        assert_eq!(file_name("a", &Asset::video("u".into(), None, None)), "a.mp4");
        assert_eq!(file_name("a", &Asset::image(0, "u.jpeg".into(), None, None)), "a_01.jpg");
        assert_eq!(file_name("a", &Asset::audio("u.mp3".into())), "a_music.mp3");
        assert_eq!(file_name("a", &Asset::cover("u.webp".into())), "a_cover.webp");
    }

    #[test]
    fn reserved_names_are_escaped() {
        assert_eq!(avoid_reserved("CON".into()), "CON_");
        assert_eq!(avoid_reserved("nul.txt".into()), "nul.txt_");
        assert_eq!(avoid_reserved("COM1".into()), "COM1_");
        assert_eq!(avoid_reserved("COM0".into()), "COM0");
        assert_eq!(avoid_reserved("CONSOLE".into()), "CONSOLE");
        let mut i = info();
        i.title = "AUX".into();
        assert_eq!(render_base("{title}", &i), "AUX_");
    }

    #[test]
    fn long_paths_are_shortened() {
        let dir = PathBuf::from("/d".repeat(60));
        let base = "很长的标题".repeat(30);
        let p = fit_path(&dir, &base, &Asset::video("u".into(), None, None), 240);
        assert!(p.as_os_str().len() + 10 <= 240, "{}", p.as_os_str().len());
        assert!(p.to_string_lossy().ends_with(".mp4"));
    }

    #[test]
    fn unique_path_appends_counter() {
        let taken = |p: &Path| p.ends_with("a.mp4") || p.ends_with("a (1).mp4");
        assert_eq!(unique_path(PathBuf::from("/x/a.mp4"), &taken), PathBuf::from("/x/a (2).mp4"));
    }
}
