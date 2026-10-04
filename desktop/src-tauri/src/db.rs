//! 本地 SQLite：解析历史和已下载文件（媒体库），用于去重。

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::model::{AppResult, MediaInfo, MediaKind};

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: i64,
    pub platform: String,
    pub media_id: String,
    pub title: String,
    pub author: String,
    pub cover: Option<String>,
    pub kind: String,
    pub source_url: String,
    pub created_at: i64,
    pub info: MediaInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: i64,
    pub platform: String,
    pub media_id: String,
    pub asset_id: String,
    pub title: String,
    pub author: String,
    pub cover: Option<String>,
    pub path: String,
    pub size: i64,
    pub finished_at: i64,
    pub exists: bool,
}

pub struct NewDownload<'a> {
    pub platform: &'a str,
    pub media_id: &'a str,
    pub asset_id: &'a str,
    pub title: &'a str,
    pub author: &'a str,
    pub cover: Option<&'a str>,
    pub path: &'a str,
    pub size: i64,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Db::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> AppResult<Db> {
        Db::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> AppResult<Db> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                platform TEXT NOT NULL,
                media_id TEXT NOT NULL,
                title TEXT NOT NULL,
                author TEXT NOT NULL,
                cover TEXT,
                kind TEXT NOT NULL,
                source_url TEXT NOT NULL,
                info_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(platform, media_id)
             );
             CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                platform TEXT NOT NULL,
                media_id TEXT NOT NULL,
                asset_id TEXT NOT NULL,
                title TEXT NOT NULL,
                author TEXT NOT NULL,
                cover TEXT,
                path TEXT NOT NULL,
                size INTEGER NOT NULL,
                finished_at INTEGER NOT NULL,
                UNIQUE(platform, media_id, asset_id)
             );",
        )?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn upsert_history(&self, info: &MediaInfo) -> AppResult<()> {
        let kind = match info.kind {
            MediaKind::Video => "video",
            MediaKind::Images => "images",
        };
        self.conn().execute(
            "INSERT INTO history (platform, media_id, title, author, cover, kind, source_url, info_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(platform, media_id) DO UPDATE SET
                title=excluded.title, author=excluded.author, cover=excluded.cover, kind=excluded.kind,
                source_url=excluded.source_url, info_json=excluded.info_json, created_at=excluded.created_at",
            params![info.platform, info.id, info.title, info.author, info.cover, kind, info.source_url, serde_json::to_string(info)?, now()],
        )?;
        Ok(())
    }

    pub fn list_history(&self, query: &str, limit: i64) -> AppResult<Vec<HistoryItem>> {
        let like = format!("%{}%", query.trim());
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, platform, media_id, title, author, cover, kind, source_url, created_at, info_json FROM history
             WHERE title LIKE ?1 OR author LIKE ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![like, limit], |r| {
            let info_json: String = r.get(9)?;
            Ok((
                HistoryItem {
                    id: r.get(0)?,
                    platform: r.get(1)?,
                    media_id: r.get(2)?,
                    title: r.get(3)?,
                    author: r.get(4)?,
                    cover: r.get(5)?,
                    kind: r.get(6)?,
                    source_url: r.get(7)?,
                    created_at: r.get(8)?,
                    info: placeholder_info(),
                },
                info_json,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (mut item, json) = row?;
            match serde_json::from_str(&json) {
                Ok(info) => item.info = info,
                Err(_) => continue,
            }
            out.push(item);
        }
        Ok(out)
    }

    pub fn delete_history(&self, id: i64) -> AppResult<()> {
        self.conn().execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> AppResult<()> {
        self.conn().execute("DELETE FROM history", [])?;
        Ok(())
    }

    pub fn record_download(&self, d: &NewDownload<'_>) -> AppResult<()> {
        self.conn().execute(
            "INSERT INTO downloads (platform, media_id, asset_id, title, author, cover, path, size, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(platform, media_id, asset_id) DO UPDATE SET
                title=excluded.title, author=excluded.author, cover=excluded.cover, path=excluded.path,
                size=excluded.size, finished_at=excluded.finished_at",
            params![d.platform, d.media_id, d.asset_id, d.title, d.author, d.cover, d.path, d.size, now()],
        )?;
        Ok(())
    }

    /// 已下载且文件仍在磁盘上时返回文件路径。
    pub fn existing_download(&self, platform: &str, media_id: &str, asset_id: &str) -> AppResult<Option<String>> {
        let path: Option<String> = self
            .conn()
            .query_row("SELECT path FROM downloads WHERE platform=?1 AND media_id=?2 AND asset_id=?3", params![platform, media_id, asset_id], |r| r.get(0))
            .optional()?;
        Ok(path.filter(|p| Path::new(p).exists()))
    }

    pub fn list_library(&self, query: &str, limit: i64) -> AppResult<Vec<LibraryItem>> {
        let like = format!("%{}%", query.trim());
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, platform, media_id, asset_id, title, author, cover, path, size, finished_at FROM downloads
             WHERE title LIKE ?1 OR author LIKE ?1 ORDER BY finished_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![like, limit], |r| {
            let path: String = r.get(7)?;
            Ok(LibraryItem {
                id: r.get(0)?,
                platform: r.get(1)?,
                media_id: r.get(2)?,
                asset_id: r.get(3)?,
                title: r.get(4)?,
                author: r.get(5)?,
                cover: r.get(6)?,
                exists: Path::new(&path).exists(),
                path,
                size: r.get(8)?,
                finished_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// 删除媒体库记录，返回该记录的文件路径。
    pub fn delete_library(&self, id: i64) -> AppResult<Option<String>> {
        let conn = self.conn();
        let path: Option<String> = conn.query_row("SELECT path FROM downloads WHERE id=?1", params![id], |r| r.get(0)).optional()?;
        conn.execute("DELETE FROM downloads WHERE id=?1", params![id])?;
        Ok(path)
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn placeholder_info() -> MediaInfo {
    MediaInfo {
        platform: String::new(),
        platform_name: String::new(),
        id: String::new(),
        source_url: String::new(),
        title: String::new(),
        author: String::new(),
        cover: None,
        duration_ms: None,
        kind: MediaKind::Video,
        width: None,
        height: None,
        published_at: None,
        assets: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Asset;

    fn sample(id: &str, title: &str) -> MediaInfo {
        MediaInfo {
            platform: "douyin".into(),
            platform_name: "抖音".into(),
            id: id.into(),
            title: title.into(),
            author: "作者".into(),
            assets: vec![Asset::video("https://x/v.mp4".into(), None, None)],
            ..placeholder_info()
        }
    }

    #[test]
    fn history_upsert_dedupes_and_searches() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_history(&sample("1", "板栗焖鸡")).unwrap();
        db.upsert_history(&sample("2", "手冲咖啡")).unwrap();
        db.upsert_history(&sample("1", "板栗焖鸡（更新）")).unwrap();
        let all = db.list_history("", 50).unwrap();
        assert_eq!(all.len(), 2);
        let hit = db.list_history("板栗", 50).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].title, "板栗焖鸡（更新）");
        assert_eq!(hit[0].info.assets.len(), 1);
    }

    #[test]
    fn existing_download_requires_file_on_disk() {
        let db = Db::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("clearclip-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.mp4");
        std::fs::write(&file, b"x").unwrap();
        let path = file.to_string_lossy().to_string();
        db.record_download(&NewDownload { platform: "douyin", media_id: "1", asset_id: "video", title: "t", author: "a", cover: None, path: &path, size: 1 })
            .unwrap();
        assert_eq!(db.existing_download("douyin", "1", "video").unwrap(), Some(path.clone()));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(db.existing_download("douyin", "1", "video").unwrap(), None);
        assert_eq!(db.list_library("", 10).unwrap().len(), 1);
        assert!(!db.list_library("", 10).unwrap()[0].exists);
        let _ = std::fs::remove_dir_all(dir);
    }
}
