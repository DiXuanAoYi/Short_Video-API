mod clipboard;
mod commands;
pub mod cookies;
pub mod db;
pub mod diagnostics;
pub mod download;
pub mod engine;
pub mod error;
pub mod model;
pub mod naming;
pub mod net;
pub mod postprocess;
pub mod providers;
pub mod secret;
pub mod settings;
pub mod tools;
mod tray;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tauri::{Manager, WindowEvent};

use crate::cookies::CookieStore;
use crate::db::Db;
use crate::download::DownloadManager;
use crate::settings::Settings;

pub struct AppState {
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub db: Db,
    /// 解析用（带总超时）
    pub client: reqwest::Client,
    /// 下载用（只有读超时，适合大文件）
    pub dl_client: reqwest::Client,
    pub downloads: DownloadManager,
    /// 托盘里临时暂停剪贴板监听
    pub clipboard_paused: AtomicBool,
    /// 程序自己写入剪贴板的内容，监听时忽略
    pub clipboard_ignore: Mutex<Option<String>>,
    pub cookies: CookieStore,
    pub samples_dir: PathBuf,
    pub log_dir: PathBuf,
    /// 最近一次注册全局快捷键失败的原因
    pub shortcut_error: Mutex<Option<String>>,
    /// 按网站分流的网络客户端、限速与请求间隔
    pub net: Arc<net::NetManager>,
    /// 程序管理的外部组件目录（yt-dlp、ffmpeg）
    pub tools_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 解析用的上下文；开启“录制样本”时附带样本目录。
    pub fn parse_ctx<'a>(&'a self, settings: &'a Settings) -> providers::Ctx<'a> {
        let mut ctx = providers::Ctx::new(&self.client, settings, &self.cookies);
        ctx.net = Some(&self.net);
        if settings.record_samples {
            ctx.samples = Some(self.samples_dir.clone());
        }
        ctx
    }
}

/// 定期检查即将过期的登录 Cookie 并发送通知（每个账号每天最多提醒一次）。
fn spawn_account_reminders(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut notified: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            let st = app.state::<Arc<AppState>>().inner().clone();
            let now = db::now();
            for a in st.cookies.summaries() {
                let Some(exp) = a.expires_at else { continue };
                let left = exp - now;
                if left > 3 * 86400 || notified.get(&a.id).is_some_and(|t| now - t < 86400) {
                    continue;
                }
                let body = if left <= 0 {
                    format!("{}账号“{}”的登录状态已过期，需要登录的内容将无法下载。", a.site_name, a.label)
                } else {
                    format!("{}账号“{}”的登录状态将在 {} 小时后过期。", a.site_name, a.label, left / 3600 + 1)
                };
                use tauri_plugin_notification::NotificationExt;
                let _ = app.notification().builder().title("登录即将失效").body(body).show();
                notified.insert(a.id.clone(), now);
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: Some("clearclip".into()) }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(2 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(5))
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        clipboard::parse_clipboard_now(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let default_dl = app.path().download_dir().or_else(|_| app.path().home_dir()).unwrap_or_else(|_| PathBuf::from(".")).join("ClearClip");
            let settings_path = config_dir.join("settings.json");
            let mut settings = Settings::load(&settings_path, &default_dl);
            let db = Db::open(&data_dir.join("clearclip.db")).map_err(|e| e.to_string())?;
            let log_dir = app.path().app_log_dir().unwrap_or_else(|_| data_dir.join("logs"));
            log::info!("ClearClip {} starting on {}", app.package_info().version, std::env::consts::OS);

            let key = secret::load_or_create_key(&data_dir);
            let cookie_store = CookieStore::open(cookies::store_path(&data_dir), key.key, key.in_keyring);
            if !settings.cookies.is_empty() {
                match cookie_store.migrate_legacy(&settings.cookies) {
                    Ok(n) => {
                        log::info!("migrated {n} legacy cookie entries into the encrypted store");
                        settings.cookies.clear();
                        settings.cookie_updated_at.clear();
                        let _ = settings.save(&settings_path);
                    }
                    Err(e) => log::warn!("legacy cookie migration failed: {e}"),
                }
            }

            app.manage(Arc::new(AppState {
                settings: RwLock::new(settings.clone()),
                settings_path,
                db,
                client: providers::build_client(),
                dl_client: download::build_download_client(),
                downloads: DownloadManager::default(),
                clipboard_paused: AtomicBool::new(false),
                clipboard_ignore: Mutex::new(None),
                cookies: cookie_store,
                samples_dir: diagnostics::samples_dir(&data_dir),
                log_dir,
                shortcut_error: Mutex::new(None),
                net: Arc::new(net::NetManager::default()),
                tools_dir: data_dir.join("tools"),
                data_dir: data_dir.clone(),
            }));
            download::restore(&handle);
            spawn_account_reminders(handle.clone());

            tray::create(&handle)?;
            commands::apply_shortcut(&handle, &settings.shortcut);
            clipboard::start_watcher(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "main" => {
                        let app = window.app_handle();
                        let to_tray = app.state::<Arc<AppState>>().settings().close_to_tray;
                        if to_tray {
                            api.prevent_close();
                            let _ = window.hide();
                        } else {
                            app.exit(0);
                        }
                    }
                    "mini" => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_settings,
            commands::save_settings,
            commands::detect_links,
            commands::resolve_link,
            commands::resolve_and_enqueue,
            commands::enqueue,
            commands::list_tasks,
            commands::pause_task,
            commands::resume_task,
            commands::cancel_task,
            commands::remove_task,
            commands::clear_finished,
            commands::pause_all,
            commands::resume_all,
            commands::list_history,
            commands::delete_history,
            commands::clear_history,
            commands::list_library,
            commands::delete_library,
            commands::list_orphan_parts,
            commands::delete_orphan_parts,
            commands::list_accounts,
            commands::import_cookies_file,
            commands::import_cookies_text,
            commands::rename_account,
            commands::set_default_account,
            commands::delete_account,
            commands::check_account,
            commands::test_route,
            commands::get_diagnostics,
            commands::open_log_dir,
            commands::open_samples_dir,
            commands::copy_text,
            commands::open_file,
            commands::reveal_file,
            commands::open_url,
            commands::show_main,
            commands::hide_mini,
            commands::open_login,
            commands::save_login_cookies,
            commands::check_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ClearClip");
}
