#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod logger;
mod settings;
mod state;

use settings::Settings;
use state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, RwLock};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use vilauncher_core::downloader::{build_client, CancelToken};
use vilauncher_core::instances::{migrate_legacy_client, Migration};
use vilauncher_core::servers;
use vilauncher_core::secure_store::{migrate_legacy_settings, TokenStore};

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_main(app);
        }
    }
}

fn main() {
    tauri::Builder::default()
        // Обновления лаунчера: подпись проверяется публичным ключом из tauri.conf.json (plugins.updater.pubkey).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Тот же каталог, что и у Electron-версии (%APPDATA%\.mc-launcher), —
            // уже установленный клиент подхватывается без переустановки.
            let launcher_dir = app.path().config_dir()?.join(".mc-launcher");
            logger::init(&launcher_dir);
            log::info!("ViLauncher {} (Tauri) стартует, каталог: {}", env!("CARGO_PKG_VERSION"), launcher_dir.display());

            // Секреты — только в системное хранилище. Старый settings.json вычищаем.
            let tokens = TokenStore::system();
            let settings_path = launcher_dir.join("settings.json");
            match migrate_legacy_settings(&settings_path, &tokens) {
                Ok(rep) if rep.file_changed => log::info!(
                    "[secure_store] из settings.json удалено: {:?}, пароль перенесён: {}, refresh-токен перенесён: {}",
                    rep.removed_keys, rep.saved_password_moved, rep.refresh_token_moved
                ),
                Ok(_) => {}
                Err(e) => log::warn!("[secure_store] миграция settings.json не удалась: {e}"),
            }

            let settings = tauri::async_runtime::block_on(Settings::load(&settings_path));
            let http = build_client(&format!("ViLauncher/{}", env!("CARGO_PKG_VERSION")))?;

            // Список серверов: кэш последнего servers.json, иначе встроенный; свежий придёт в фоне.
            let server_list = tauri::async_runtime::block_on(servers::load_servers_cache(&launcher_dir)).unwrap_or_else(servers::default_servers);
            let selected = tauri::async_runtime::block_on(settings.get_str("selectedServer"))
                .filter(|id| server_list.iter().any(|s| &s.id == id))
                .unwrap_or_else(|| server_list.iter().find(|s| s.active).or(server_list.first()).map(|s| s.id.clone()).unwrap_or_default());

            // Миграция: единственный клиент прежних версий (.mc-launcher/client) → instance основного modded-сервера.
            let default_instance = server_list.iter().find(|s| s.active && !s.is_vanilla()).or(server_list.first()).map(|s| s.instance_id.clone()).unwrap_or_else(|| "create".into());
            match migrate_legacy_client(&launcher_dir, &default_instance) {
                Ok(Migration::Moved) => log::info!("[migrate] клиент перенесён в instances/{default_instance}"),
                Ok(Migration::TargetExists) => log::warn!("[migrate] есть и client/, и instances/{default_instance} — старый клиент не тронут"),
                Ok(Migration::NothingToDo) => {}
                Err(e) => log::warn!("[migrate] перенос клиента не удался: {e}"),
            }
            // Версия модов и ключ установки теперь хранятся по instance'ам — старые общие значения не нужны.
            if tauri::async_runtime::block_on(async { settings.get("modsVersion").await.is_some() || settings.get("clientRevision").await.is_some() || settings.get("installWanted").await.is_some() }) {
                let _ = tauri::async_runtime::block_on(settings.update(|m| {
                    for k in ["modsVersion", "clientRevision", "installWanted"] {
                        m.remove(k);
                    }
                }));
            }

            app.manage(AppState {
                launcher_dir,
                http,
                tokens,
                settings,
                session: Mutex::new(None),
                sync_cancel: StdMutex::new(CancelToken::new()),
                auth_cancel: StdMutex::new(CancelToken::new()),
                game_running: AtomicBool::new(false),
                blocklist: Mutex::new(None),
                discord: Mutex::new(None),
                quitting: AtomicBool::new(false),
                game_kill: StdMutex::new(None),
                servers: RwLock::new(server_list),
                selected: RwLock::new(selected),
                instance_settings: StdMutex::new(HashMap::new()),
                busy: AtomicBool::new(false),
                pending_update: Mutex::new(None),
            });

            // Трей: «Открыть» / «Выход»; клик по иконке — показать/скрыть окно.
            let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &sep, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip(config::APP_NAME)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => {
                        app.state::<AppState>().quitting.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        toggle_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // Свежий servers.json из репозитория (включить/выключить сервер можно правкой файла, без пересборки).
            let h2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::refresh_servers_inner(&h2).await;
            });

            // Discord Rich Presence (если Discord запущен).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::init_discord(&handle.state::<AppState>()).await;
            });
            Ok(())
        })
        // Крестик сворачивает в трей, настоящий выход — через меню трея.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let quitting = window.app_handle().try_state::<AppState>().map(|s| s.quitting.load(Ordering::SeqCst)).unwrap_or(true);
                if !quitting {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::setting_set,
            commands::nick_get,
            commands::nick_set,
            commands::news_fetch,
            commands::open_url,
            commands::open_launcher_dir,
            commands::ram_get,
            commands::ram_set,
            commands::dev_auth,
            commands::dev_flag_set,
            commands::dev_status,
            commands::install_check,
            commands::install_start,
            commands::reinstall_client,
            commands::check_mods,
            commands::mods_sync,
            commands::mods_cancel,
            commands::auth_login,
            commands::auth_get_user,
            commands::auth_get_saved,
            commands::auth_logout,
            commands::ms_login,
            commands::ms_cancel,
            commands::ms_restore,
            commands::ms_logout,
            commands::game_launch,
            commands::server_ping,
            commands::updater_check,
            commands::updater_install,
            commands::list_servers,
            commands::select_server,
            commands::get_instance_status,
            commands::refresh_servers,
            commands::skin_get,
            commands::skin_upload,
            commands::skin_reset,
            commands::scan_cheats,
            commands::defaults_reset,
            commands::game_options_get,
            commands::game_options_set,
            commands::mod_configs_list,
            commands::mod_config_read,
            commands::mod_config_write,
            commands::modrinth_search,
            commands::modrinth_versions,
            commands::modrinth_download,
            commands::modrinth_installed,
            commands::modrinth_active,
            commands::modrinth_toggle,
            commands::modrinth_remove,
            commands::discord_set_playing,
            commands::discord_set_launcher,
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска ViLauncher");
}
