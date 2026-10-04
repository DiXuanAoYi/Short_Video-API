mod clipboard;
mod commands;
pub mod db;
pub mod download;
pub mod model;
pub mod naming;
pub mod providers;
pub mod settings;
mod tray;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tauri::{Manager, WindowEvent};

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
}

impl AppState {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
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
            let settings = Settings::load(&settings_path, &default_dl);
            let db = Db::open(&data_dir.join("clearclip.db")).map_err(|e| e.to_string())?;

            app.manage(Arc::new(AppState {
                settings: RwLock::new(settings.clone()),
                settings_path,
                db,
                client: providers::build_client(),
                dl_client: download::build_download_client(),
                downloads: DownloadManager::default(),
                clipboard_paused: AtomicBool::new(false),
                clipboard_ignore: Mutex::new(None),
            }));

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
