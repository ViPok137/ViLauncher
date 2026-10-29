//! Tauri-команды. Возвращают JSON вида `{ ok: bool, ... }` — тот же контракт, что был у
//! IPC-обработчиков Electron, поэтому фронтенд (`window.api`) меняется минимально.

use crate::config;
use crate::state::AppState;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};
use vilauncher_core::downloader::{fetch_text, CancelToken, DownloadOptions, ProgressFn};
use vilauncher_core::installer::{install, install_key, InstallProgressFn, InstallSources, InstallTarget, LoaderSource};
use vilauncher_core::servers::ServerCfg;
use tauri_plugin_updater::UpdaterExt;
use vilauncher_core::secure_store::SAVED_PASSWORD_SLOT;
use vilauncher_core::launch::{build_args, preflight, ram_plan, spawn_game, LaunchJson, LaunchParams};
use vilauncher_core::anticheat;
use vilauncher_core::discord::{Activity, DiscordRpc};
use vilauncher_core::modrinth::{self, PackKind};
use vilauncher_core::serverping::ping_server;
use vilauncher_core::skins;
use vilauncher_core::gamecfg;
use vilauncher_core::sync::{allowed_mods, fetch_defaults, fetch_manifest, load_cache, run_sync, save_cache, sync_defaults, verify_mods, HashMapFile, Manifest, SyncConfig};
use vilauncher_core::watchdog::{build_snapshot, check_tick, Violation, ViolationKind};

type Json = Value;

/// Серверы с `active = false` видны в списке, но ничего с ними делать нельзя (синк, установка, запуск).
fn require_playable(state: &AppState) -> std::result::Result<ServerCfg, String> {
    let s = state.selected_server();
    if s.can_play() {
        Ok(s)
    } else {
        Err(format!("Сервер «{}» пока недоступен для игры", s.name))
    }
}

fn busy_fail() -> Json {
    fail("Уже выполняется другая операция — дождись её завершения")
}

fn fail(e: impl ToString) -> Json {
    json!({ "ok": false, "error": e.to_string() })
}

fn total_ram_gb() -> f64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0
}

async fn user_ram(state: &AppState) -> (Option<u32>, Option<u32>) {
    let parse = |s: Option<String>| s.and_then(|v| v.trim().parse::<u32>().ok()).filter(|v| *v > 0);
    (parse(state.settings.get_str("ramMaxG").await), parse(state.settings.get_str("ramMinG").await))
}

// ───────────────────────────── общие ─────────────────────────────

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> Result<Json, ()> {
    let installed: Vec<String> = [17u32, 21, 25].iter().filter(|m| state.java_for(**m).exe_exists()).map(|m| m.to_string()).collect();
    let java_label = if installed.is_empty() { "Не установлена — нажми «Установить»".to_string() } else { format!("Изолированная Java {} (portable)", installed.join(", ")) };
    let total = total_ram_gb();
    let (umax, umin) = user_ram(&state).await;
    let plan = ram_plan(total, umax, umin);
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "modsVersion": state.inst().await.get_str("modsVersion").await,
        "server": state.selected_server().name,
        "electron": "Tauri 2",
        "os": if cfg!(windows) { "Windows" } else { std::env::consts::OS },
        "java": java_label,
        "ram": format!("{}G / {:.1} ГБ всего", plan.max_g, total),
        "devMode": state.settings.get_bool("devMode", false).await,
        "minimizeToTray": state.settings.get_bool("minimizeToTray", true).await,
    }))
}

#[tauri::command]
pub async fn setting_set(state: State<'_, AppState>, key: String, value: Value) -> Result<Json, ()> {
    const ALLOWED: [&str; 4] = ["devMode", "minimizeToTray", "ramMin", "ramMax"];
    if !ALLOWED.contains(&key.as_str()) {
        return Ok(fail(format!("Неизвестный ключ настройки: {key}")));
    }
    Ok(match state.settings.set(&key, value).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn nick_get(state: State<'_, AppState>) -> Result<String, ()> {
    Ok(state.settings.get_str("nickname").await.unwrap_or_default())
}

#[tauri::command]
pub async fn nick_set(state: State<'_, AppState>, nick: String) -> Result<Json, ()> {
    Ok(match state.settings.set("nickname", Value::String(nick)).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn news_fetch(state: State<'_, AppState>) -> Result<Json, ()> {
    let url = format!("{}/NEWS.json", config::raw_base_url());
    let fallback = json!([{ "id": 0, "date": "—", "title": "Нет связи с GitHub", "body": "Проверь интернет." }]);
    Ok(match fetch_text(&state.http, &url, 2).await {
        Ok(t) => serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap_or(fallback),
        Err(_) => fallback,
    })
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<Json, ()> {
    // Только https и только github.com (как в Electron-версии); `user@host` отвергаем.
    let Some(rest) = url.strip_prefix("https://") else { return Ok(fail("URL не разрешён")) };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let allowed = !host.contains('@') && !host.contains(':') && (host == "github.com" || host.ends_with(".github.com"));
    if !allowed {
        return Ok(fail("URL не разрешён"));
    }
    Ok(match open::that(&url) {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn open_launcher_dir(state: State<'_, AppState>) -> Result<Json, ()> {
    Ok(match open::that(&state.launcher_dir) {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

// ───────────────────────────── RAM ─────────────────────────────

#[tauri::command]
pub async fn ram_get(state: State<'_, AppState>) -> Result<Json, ()> {
    let total = total_ram_gb();
    let auto = ram_plan(total, None, None);
    Ok(json!({
        "totalGb": total.round(),
        "autoMax": auto.max_g,
        "autoMin": auto.min_g,
        "currentMax": state.settings.get_str("ramMaxG").await.unwrap_or_else(|| "auto".into()),
        "currentMin": state.settings.get_str("ramMinG").await.unwrap_or_else(|| "auto".into()),
    }))
}

#[tauri::command]
pub async fn ram_set(state: State<'_, AppState>, max_g: Value, min_g: Value) -> Result<Json, ()> {
    let res = state
        .settings
        .update(|m| {
            m.insert("ramMaxG".into(), max_g);
            m.insert("ramMinG".into(), min_g);
        })
        .await;
    Ok(match res {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

// ───────────────────────────── dev-панель ─────────────────────────────

#[tauri::command]
pub async fn dev_auth(state: State<'_, AppState>, code: String) -> Result<Json, ()> {
    if code.trim() != config::DEV_CODE {
        log::warn!("[dev-auth] Неверный код доступа");
        return Ok(json!({ "ok": false }));
    }
    let now = !state.settings.get_bool("devMode", false).await;
    let res = state
        .settings
        .update(|m| {
            m.insert("devMode".into(), Value::Bool(now));
            if !now {
                m.insert("devDisableUpdates".into(), Value::Bool(false));
                m.insert("devDisableAnticheat".into(), Value::Bool(false));
            }
        })
        .await;
    Ok(match res {
        Ok(()) => json!({ "ok": true, "devMode": now }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn dev_flag_set(state: State<'_, AppState>, flag: String, value: bool) -> Result<Json, ()> {
    if !["devDisableUpdates", "devDisableAnticheat"].contains(&flag.as_str()) {
        return Ok(json!({ "ok": false }));
    }
    if !state.settings.get_bool("devMode", false).await {
        return Ok(fail("devMode не активен"));
    }
    Ok(match state.settings.set(&flag, Value::Bool(value)).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn dev_status(state: State<'_, AppState>) -> Result<Json, ()> {
    Ok(json!({
        "devMode": state.settings.get_bool("devMode", false).await,
        "devDisableUpdates": state.settings.get_bool("devDisableUpdates", false).await,
        "devDisableAnticheat": state.settings.get_bool("devDisableAnticheat", false).await,
    }))
}

// ───────────────────────────── установка / Java ─────────────────────────────

#[tauri::command]
pub async fn install_check(state: State<'_, AppState>) -> Result<Json, ()> {
    let server = state.selected_server();
    Ok(json!({ "installed": instance_installed(&state, &server).await, "active": server.active, "modsVersion": state.inst_of(&server).await.get_str("modsVersion").await }))
}

/// Установлено = есть launch.json ТОЙ ЖЕ связки «Minecraft + загрузчик», что требует манифест сервера
/// (ключ запоминается при синхронизации). Клиент от прежней версии считается устаревшим.
async fn instance_installed(state: &AppState, server: &ServerCfg) -> bool {
    let Ok(l) = LaunchJson::load(&state.instance_dir_of(server).join("launch.json")).await else { return false };
    if l.mc_version.is_empty() {
        return false;
    }
    match state.inst_of(server).await.get_str("installWanted").await {
        Some(wanted) => l.install_key == wanted,
        None => server.mc_version == "latest" || l.mc_version == server.mc_version,
    }
}

/// Установка клиента выбранного сервера: манифест Mojang → Java нужной версии → клиент → библиотеки →
/// ресурсы → (для modded) загрузчик NeoForge → `launch.json`. Для vanilla загрузчика нет, а версия `latest`
/// определяется по манифесту Mojang. Модded: версия MC и installer берутся из manifest.json сервера.
#[tauri::command]
pub async fn install_start(app: AppHandle, state: State<'_, AppState>) -> Result<Json, ()> {
    let server = match require_playable(&state) {
        Ok(s) => s,
        Err(e) => return Ok(fail(e)),
    };
    let Some(_busy) = state.try_busy() else { return Ok(busy_fail()) };
    let (client_dir, inst) = (state.client_dir(), state.inst_of(&server).await);
    if let Err(e) = tokio::fs::create_dir_all(&client_dir).await {
        return Ok(fail(e));
    }
    let _ = tokio::fs::remove_file(client_dir.join("launch.json")).await;

    let target = if server.is_vanilla() {
        InstallTarget { mc_version: server.mc_version.clone(), loader: None }
    } else {
        let base = server.manifest_base(&config::raw_base_url());
        let manifest = match fetch_manifest(&state.http, &base).await {
            Ok((m, _)) => m,
            Err(e) => return Ok(fail(format!("Не удалось получить manifest.json сервера «{}»: {e}", server.name))),
        };
        let Some(entry) = manifest.loader.as_ref() else {
            return Ok(fail("В manifest.json нет раздела loader (installer загрузчика NeoForge). Добавь его и повтори."));
        };
        let loader = match LoaderSource::from_manifest(entry, &base) {
            Ok(l) => l,
            Err(e) => return Ok(fail(e)),
        };
        let mc_version = manifest.mc_version.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| server.mc_version.clone());
        InstallTarget { mc_version, loader: Some(loader) }
    };

    let app2 = app.clone();
    let progress: InstallProgressFn = std::sync::Arc::new(move |p| {
        let phase = match p.phase {
            "java" => "☕ Изолированная Java",
            "manifest" => "📋 Манифест",
            "client" => "📦 Клиент",
            "libraries" => "📚 Библиотеки",
            "assets" => "🖼 Ресурсы",
            "forge" => "⚙ NeoForge (2-3 мин)",
            "done" => "✅ Готово",
            other => other,
        };
        let pct = if p.total > 0 { ((p.done as f64 / p.total as f64) * 100.0).round() as u64 } else { 0 };
        let _ = app2.emit("install-progress", json!({ "phase": phase, "done": p.done, "total": p.total, "name": p.name, "pct": pct }));
    });

    let cancel = CancelToken::new();
    match install(&state.http, &state.launcher_dir, &client_dir, &InstallSources::default(), &target, &cancel, Some(progress)).await {
        Ok(launch) => {
            let _ = inst.set("installWanted", Value::String(launch.install_key.clone())).await;
            log::info!("[install] «{}»: Minecraft {} (Java {}) установлен", server.name, launch.mc_version, launch.java_major);
            Ok(json!({ "ok": true, "mcVersion": launch.mc_version }))
        }
        Err(e) => {
            log::error!("[install] {e}");
            Ok(fail(e))
        }
    }
}

#[tauri::command]
pub async fn reinstall_client(state: State<'_, AppState>) -> Result<Json, ()> {
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(game_busy_error());
    }
    let Some(_busy) = state.try_busy() else { return Ok(busy_fail()) };
    // Удаляется только instance выбранного сервера; Java, шейдеры и настройки лаунчера остаются.
    let server = state.selected_server();
    let dir = state.instance_dir_of(&server);
    if tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
            return Ok(fail(e));
        }
    }
    state.forget_instance_settings(&server.instance_id);
    log::info!("[settings] Клиент «{}» удалён для переустановки", server.name);
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── моды (модуль 1) ─────────────────────────────

#[tauri::command]
pub async fn check_mods(state: State<'_, AppState>) -> Result<Json, ()> {
    Ok(match state.inst().await.update(|m| { m.remove("modsVersion"); }).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn mods_sync(app: AppHandle, state: State<'_, AppState>, force: Option<bool>) -> Result<Json, ()> {
    let dev = state.settings.get_bool("devMode", false).await;
    if dev && state.settings.get_bool("devDisableUpdates", false).await {
        return Ok(json!({ "ok": true, "updated": false, "msg": "Обновления отключены (DEV MODE)" }));
    }
    let server = match require_playable(&state) {
        Ok(s) => s,
        Err(e) => return Ok(fail(e)),
    };
    if server.is_vanilla() {
        return Ok(json!({ "ok": true, "updated": false, "msg": "Для этого сервера моды не нужны (чистая ваниль)" }));
    }
    let Some(_busy) = state.try_busy() else { return Ok(busy_fail()) };
    let (client_dir, inst) = (state.client_dir(), state.inst_of(&server).await);

    let cancel = CancelToken::new();
    *state.sync_cancel.lock().unwrap() = cancel.clone();

    let _ = app.emit("mods-status", json!({ "text": "Загружаю манифест и хэши с GitHub..." }));
    let base = server.manifest_base(&config::raw_base_url());
    let (manifest, hashes) = match fetch_manifest(&state.http, &base).await {
        Ok(x) => x,
        Err(e) => return Ok(fail(e)),
    };

    // Запоминаем, какая связка «Minecraft + загрузчик» требуется манифестом: install_check по ней
    // понимает, что после обновления загрузчика нужно переустановить клиент.
    if let Some(entry) = manifest.loader.as_ref() {
        if let Ok(l) = LoaderSource::from_manifest(entry, &base) {
            let mc = manifest.mc_version.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| server.mc_version.clone());
            let _ = inst.set("installWanted", Value::String(install_key(&mc, &l.version))).await;
        }
    }

    // Настройки по умолчанию — отдельный манифест (defaults.json): сюда попадают только отсутствующие файлы.
    let defaults = fetch_defaults(&state.http, &base, &manifest).await.unwrap_or_default();
    save_cache(&client_dir, &manifest, &hashes, &defaults).await;
    let mut mods_manifest = manifest.clone();
    mods_manifest.groups.defaults.clear(); // устаревшая группа defaults внутри manifest.json обрабатывается через defaults.json

    let base_for_defaults = base.clone();
    let mut cfg = SyncConfig::new(client_dir.clone(), base);
    cfg.previous_version = inst.get_str("modsVersion").await;
    cfg.previous_client_revision = inst.get_u64("clientRevision").await;
    cfg.force = force.unwrap_or(false);
    cfg.remove_unauthorized_mods = !(dev && state.settings.get_bool("devDisableAnticheat", false).await);

    let _ = app.emit("mods-status", json!({ "text": "📦 Дельта-синхронизация: сверяю хеши..." }));
    let app2 = app.clone();
    let progress: ProgressFn = std::sync::Arc::new(move |e| {
        let _ = app2.emit("mods-progress", e);
    });

    let result = run_sync(&state.http, &cfg, &mods_manifest, &hashes, &DownloadOptions::default(), &cancel, Some(progress)).await;
    // Недостающие файлы настроек докачиваются при каждом старте (правки игрока не затрагиваются).
    let defaults_report = if result.is_ok() {
        match sync_defaults(&state.http, &base_for_defaults, &client_dir, &defaults, &hashes, false, None, &DownloadOptions::default(), &cancel, None).await {
            Ok(r) => {
                if r.downloaded > 0 || !r.failed.is_empty() {
                    log::info!("[defaults] установлено недостающих файлов настроек: {}, ошибок: {}", r.downloaded, r.failed.len());
                }
                serde_json::to_value(r).unwrap_or_default()
            }
            Err(e) => {
                log::warn!("[defaults] {e}");
                json!({ "error": e.to_string() })
            }
        }
    } else {
        Value::Null
    };
    match result {
        Ok(rep) if !rep.updated => Ok(json!({
            "ok": true, "updated": false,
            "msg": format!("Моды актуальны ({})", rep.version),
            "defaults": defaults_report,
        })),
        Ok(rep) => {
            let (ver, rev) = (rep.version.clone(), rep.client_revision);
            let saved = inst
                .update(move |m| {
                    m.insert("modsVersion".into(), Value::String(ver));
                    if let Some(r) = rev {
                        m.insert("clientRevision".into(), json!(r));
                    }
                })
                .await;
            if let Err(e) = saved {
                return Ok(fail(e));
            }
            log::info!(
                "[mods-sync] v{}: скачано {}, актуально {}, удалено лишних {}",
                rep.version, rep.downloaded, rep.up_to_date, rep.removed_mods.len()
            );
            Ok(json!({ "ok": true, "updated": true, "version": rep.version, "report": rep, "defaults": defaults_report }))
        }
        Err(e) => {
            log::error!("[mods-sync] {e}");
            Ok(fail(e))
        }
    }
}

#[tauri::command]
pub async fn mods_cancel(state: State<'_, AppState>) -> Result<Json, ()> {
    state.sync_cancel.lock().unwrap().cancel();
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── авторизация ─────────────────────────────
// Логика оригинала сохранена: форма логин/пароль — заглушка (проверки на сервере нет,
// принимаются любые непустые данные). Изменено только место хранения пароля «Запомнить меня»:
// вместо открытого `settings.json` он лежит в системном хранилище ОС (модуль 2).

#[tauri::command]
pub async fn auth_login(
    state: State<'_, AppState>,
    username: Option<String>,
    password: Option<String>,
    remember: Option<bool>,
) -> Result<Json, ()> {
    let username = username.unwrap_or_default();
    let password = password.unwrap_or_default();
    if username.is_empty() || password.is_empty() {
        return Ok(fail("Введи логин и пароль"));
    }
    let remember = remember.unwrap_or(false);
    let user = username.clone();
    let saved = state
        .settings
        .update(move |m| {
            m.insert("username".into(), Value::String(user.clone()));
            if remember {
                m.insert("savedUser".into(), Value::String(user));
            } else {
                m.remove("savedUser");
            }
        })
        .await;
    if let Err(e) = saved {
        return Ok(fail(e));
    }
    let secret = if remember {
        state.tokens.save_secret(SAVED_PASSWORD_SLOT, &password)
    } else {
        state.tokens.clear_secret(SAVED_PASSWORD_SLOT)
    };
    if let Err(e) = secret {
        // Хранилище ОС недоступно: не падаем (вход — заглушка), но пароль не сохраняем.
        log::warn!("[auth] системное хранилище недоступно, пароль не сохранён: {e}");
    }
    Ok(json!({ "ok": true, "username": username }))
}

/// Имя текущего пользователя или `null` (как `auth-get-user` в оригинале).
#[tauri::command]
pub async fn auth_get_user(state: State<'_, AppState>) -> Result<Option<String>, ()> {
    Ok(state.settings.get_str("username").await)
}

#[tauri::command]
pub async fn auth_get_saved(state: State<'_, AppState>) -> Result<Json, ()> {
    let password = state.tokens.load_secret(SAVED_PASSWORD_SLOT).ok().flatten().unwrap_or_default();
    Ok(json!({
        "username": state.settings.get_str("savedUser").await.unwrap_or_default(),
        "password": password,
    }))
}

/// Выход: как в оригинале удаляется только `username` (сохранённый логин/пароль остаются).
#[tauri::command]
pub async fn auth_logout(state: State<'_, AppState>) -> Result<Json, ()> {
    *state.session.lock().await = None;
    Ok(match state.settings.update(|m| { m.remove("username"); }).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

// ───────────── Microsoft / Mojang (опционально, в UI не подключено) ─────────────
// Модуль 2 в части OAuth: вход device code, refresh-токен в системном хранилище.
// Пока игрок не вызвал `ms_login`, ничего не меняется: игра запускается офлайн по нику.

fn public_session(s: &vilauncher_core::auth::McSession) -> Json {
    json!({ "ok": true, "username": s.username, "uuid": s.uuid })
}

#[tauri::command]
pub async fn ms_login(app: AppHandle, state: State<'_, AppState>) -> Result<Json, ()> {
    let cancel = CancelToken::new();
    *state.auth_cancel.lock().unwrap() = cancel.clone();
    let mgr = state.auth_manager().await;
    let app2 = app.clone();
    let res = mgr
        .login(
            move |dc| {
                if dc.verification_uri.starts_with("https://") {
                    let _ = open::that(&dc.verification_uri);
                }
                let _ = app2.emit("ms-device-code", dc);
            },
            &cancel,
        )
        .await;
    match res {
        Ok(session) => {
            let out = public_session(&session);
            *state.session.lock().await = Some(session);
            Ok(out)
        }
        Err(e) => Ok(fail(e)),
    }
}

#[tauri::command]
pub async fn ms_cancel(state: State<'_, AppState>) -> Result<Json, ()> {
    state.auth_cancel.lock().unwrap().cancel();
    Ok(json!({ "ok": true }))
}

/// Тихое восстановление сессии по refresh-токену из системного хранилища.
#[tauri::command]
pub async fn ms_restore(state: State<'_, AppState>) -> Result<Json, ()> {
    match state.auth_manager().await.restore().await {
        Ok(Some(s)) => {
            let out = public_session(&s);
            *state.session.lock().await = Some(s);
            Ok(out)
        }
        Ok(None) => Ok(json!({ "ok": false })),
        Err(e) => Ok(fail(e)),
    }
}

#[tauri::command]
pub async fn ms_logout(state: State<'_, AppState>) -> Result<Json, ()> {
    *state.session.lock().await = None;
    Ok(match state.tokens.clear() {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

// ───────────────────────────── запуск игры (модуль 3) ─────────────────────────────

fn redact(args: &[String]) -> String {
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for a in args {
        out.push(if hide_next { "***".to_string() } else { a.clone() });
        hide_next = a == "--accessToken";
    }
    out.join("\n")
}

#[tauri::command]
pub async fn game_launch(
    app: AppHandle,
    state: State<'_, AppState>,
    nickname: Option<String>,
) -> Result<Json, ()> {
    if state.game_running.swap(true, Ordering::SeqCst) {
        return Ok(fail("Minecraft уже запущен! Закрой игру перед повторным запуском."));
    }
    match launch_inner(&app, &state, nickname.unwrap_or_default()).await {
        Ok(v) => Ok(v),
        Err(e) => {
            state.game_running.store(false, Ordering::SeqCst);
            log::error!("[game-launch] {e}");
            Ok(fail(e))
        }
    }
}

async fn launch_inner(app: &AppHandle, state: &AppState, nickname: String) -> Result<Json, String> {
    // Играть можно только на активном сервере; адрес, instance и модпак берутся из servers.json.
    let server = require_playable(state)?;
    if !tokio::fs::try_exists(state.launch_json()).await.unwrap_or(false) {
        return Err("Minecraft не установлен. Нажми «Установить».".into());
    }
    let launch = LaunchJson::load(&state.launch_json()).await.map_err(|e| e.to_string())?;
    if launch.mc_version.is_empty() {
        return Err("Установленный клиент от прежней версии лаунчера (другая версия Minecraft). Нажми «Установить».".into());
    }
    let mc_version = launch.mc_version.clone();

    // Единственный источник Java — изолированная JRE нужной версии (системная игнорируется).
    let java = state.java_for(launch.java_major_or_default());
    if !java.exe_exists() {
        return Err(format!(
            "Изолированная Java {} не найдена: {}\nНажми «Установить» (или «Переустановить клиент» в настройках).",
            java.major(),
            java.java_exe().display()
        ));
    }

    // Закрытая ваниль всегда на последней версии: если вышла новая, просим обновить клиент (без сети — играем как есть).
    if server.is_vanilla() && server.mc_version == "latest" {
        if let Ok(latest) = vilauncher_core::installer::latest_release(&state.http, &InstallSources::default().mojang_manifest).await {
            if latest != launch.mc_version {
                return Err(format!("Вышла новая версия Minecraft {latest} (установлена {}). Нажми «Установить», чтобы обновить клиент.", launch.mc_version));
            }
        }
    }

    // Список модов проверяется при каждом нажатии «Играть»: подменённые файлы докачиваются, лишние удаляются.
    let dev = state.settings.get_bool("devMode", false).await;
    let skip_updates = dev && state.settings.get_bool("devDisableUpdates", false).await;
    let skip_anticheat = dev && state.settings.get_bool("devDisableAnticheat", false).await;
    let verified = if skip_updates || server.is_vanilla() { None } else { verify_on_play(app, state, &server, !skip_anticheat).await? };

    // Предстартовая проверка целостности.
    let pf = preflight(&launch, &mc_version).await.map_err(|e| e.to_string())?;
    if !pf.missing_classpath.is_empty() {
        return Err(format!("Отсутствуют файлы classpath ({} шт). Переустанови клиент.", pf.missing_classpath.len()));
    }
    if let Some(jar) = &pf.corrupt_client_jar {
        let _ = tokio::fs::remove_file(jar).await;
        let _ = tokio::fs::remove_file(state.launch_json()).await;
        return Err("Файл клиента Minecraft повреждён. Удалён автоматически — нажми Установить для переустановки.".into());
    }
    if !pf.corrupt_mods.is_empty() {
        for m in &pf.corrupt_mods {
            let _ = tokio::fs::remove_file(std::path::Path::new(&launch.game_dir).join("mods").join(m)).await;
        }
        let _ = state.inst().await.update(|m| { m.remove("modsVersion"); }).await;
        return Err(format!(
            "Повреждены моды ({} шт): {}\nФайлы удалены — перезапусти лаунчер для перекачки.",
            pf.corrupt_mods.len(),
            pf.corrupt_mods.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    // Сессия Microsoft используется только после явного `ms_login`; иначе — офлайн по нику, как раньше.
    let session = state.session.lock().await.clone().filter(|s| !s.is_expired());

    // Чёрный манифест: запуск блокируется, если в mods/, resourcepacks/ или shaderpacks/ найдены запрещённые файлы.
    if !skip_anticheat {
        if let Some(bl) = load_blocklist(state, true).await {
            let threats = anticheat::scan_for_cheats(&state.client_dir(), &bl).await;
            if !threats.is_empty() {
                let names = threats.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join("\n• ");
                log::error!("[anticheat] Запуск заблокирован. Угрозы:\n• {names}");
                return Err(format!("Обнаружены запрещённые файлы ({} шт.):\n• {names}\n\nУдали их вручную и попробуй снова.", threats.len()));
            }
        }
    }

    // Символьные ссылки shared/ → shaderpacks|resourcepacks клиента.
    {
        let (game_dir, sp, rp) = (std::path::PathBuf::from(&launch.game_dir), state.shared_dir(PackKind::Shader), state.shared_dir(PackKind::ResourcePack));
        let snapshot = state.settings.snapshot().await;
        let key = server.id.clone();
        let _ = tokio::task::spawn_blocking(move || modrinth::sync_symlinks(&game_dir, &sp, &rp, &snapshot, &key)).await;
    }

    let addr = server.address.trim().to_string();
    let (umax, umin) = user_ram(state).await;
    let params = LaunchParams {
        nickname: &nickname,
        default_username: config::DEFAULT_USERNAME,
        mc_version: &mc_version,
        server_address: Some(&addr),
        ram: ram_plan(total_ram_gb(), umax, umin),
        session: session.as_ref(),
    };
    let args = build_args(&launch, &params);

    // Для отладки пишем команду запуска, но access-токен в лог не попадает.
    let _ = tokio::fs::write(state.launcher_dir.join("launch-cmd.log"), redact(&args)).await;

    let log_path = state.launcher_dir.join("minecraft.log");
    let mut child = spawn_game(&java, &launch, &args, &log_path).map_err(|e| e.to_string())?;
    let pid = child.id();
    log::info!(
        "[game-launch] «{}» (instance {}): PID {:?}, Java {} (изолированная JRE {}), режим: {}",
        server.name,
        server.instance_id,
        pid,
        java.java_exe().display(),
        java.major(),
        if session.is_some() { "Microsoft" } else { "офлайн" }
    );

    if state.settings.get_bool("minimizeToTray", true).await {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
    }
    let _ = app.emit("mc-running", json!({ "running": true }));

    // Discord: «Играет на …» с числом игроков (пинг в фоне, чтобы не задерживать запуск).
    {
        let app3 = app.clone();
        tauri::async_runtime::spawn(async move {
            let st = app3.state::<AppState>();
            let ping = match st.selected_server().host_port() {
                Some((h, p)) => ping_server(&h, p, std::time::Duration::from_secs(2)).await,
                None => Default::default(),
            };
            discord_playing(&st, ping.online_count).await;
        });
    }

    // Сторож: раз в минуту проверяет чёрный манифест и целостность списка модов; при нарушении закрывает игру.
    let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<String>();
    *state.game_kill.lock().unwrap() = Some(kill_tx);
    if !skip_anticheat {
        let allowed = verified.as_ref().map(|(m, h)| allowed_mods(m, h));
        spawn_watchdog(app.clone(), allowed);
    }

    let app2 = app.clone();
    let log_path2 = log_path.clone();
    tauri::async_runtime::spawn(async move {
        enum Outcome {
            Exited(std::io::Result<std::process::ExitStatus>),
            Killed(String),
        }
        let outcome = tokio::select! {
            status = child.wait() => Outcome::Exited(status),
            Ok(reason) = &mut kill_rx => {
                let _ = child.kill().await;
                Outcome::Killed(reason)
            }
        };
        let st = app2.state::<AppState>();
        st.game_running.store(false, Ordering::SeqCst);
        let _ = st.game_kill.lock().unwrap().take();
        if let Some(w) = app2.get_webview_window("main") {
            let _ = w.show();
            let _ = w.set_focus();
        }
        let _ = app2.emit("mc-running", json!({ "running": false }));
        discord_launcher(&st).await;
        let status = match outcome {
            Outcome::Killed(message) => {
                log::error!("[anticheat] Игра закрыта: {message}");
                let first = message.lines().next().unwrap_or("").to_string();
                let _ = app2.emit("anticheat-alert", json!({ "title": first, "message": message }));
                let _ = app2.emit("game-error", json!({ "error": message }));
                return;
            }
            Outcome::Exited(s) => s,
        };
        match status {
            Ok(s) => match s.code() {
                Some(0) | None => log::info!("[game-launch] игра завершилась (код {:?})", s.code()),
                Some(code) => {
                    let text = tokio::fs::read(&log_path2).await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    let chars: Vec<char> = text.chars().collect();
                    let tail: String = chars[chars.len().saturating_sub(2000)..].iter().collect();
                    let last: Vec<&str> = tail.split('\n').collect();
                    let last_lines = last[last.len().saturating_sub(15)..].join("\n");
                    let msg = format!("Java завершилась с кодом {code}.\nЛог: {last_lines}");
                    log::error!("[game-launch] {msg}");
                    let _ = app2.emit("game-error", json!({ "error": msg }));
                }
            },
            Err(e) => {
                let _ = app2.emit("game-error", json!({ "error": format!("Не удалось запустить Java: {e}") }));
            }
        }
    });

    Ok(json!({ "ok": true, "logPath": log_path }))
}

// ───────────────────────────── общие помощники ─────────────────────────────

/// Версия Minecraft установленного клиента (иначе значение по умолчанию из конфига).
async fn current_mc(state: &AppState) -> String {
    match LaunchJson::load(&state.launch_json()).await {
        Ok(l) if !l.mc_version.is_empty() => l.mc_version,
        _ => config::MC_VERSION.to_string(),
    }
}

/// blocklist.json: из кэша, иначе с GitHub (при ошибке кэш не заполняется — повтор при следующем вызове).
async fn ensure_blocklist(state: &AppState) -> Option<anticheat::Blocklist> {
    load_blocklist(state, false).await
}

/// `refresh = true` — перечитать чёрный манифест с GitHub (при сбое остаётся прежняя копия).
async fn load_blocklist(state: &AppState, refresh: bool) -> Option<anticheat::Blocklist> {
    if !refresh {
        if let Some(bl) = state.blocklist.lock().await.clone() {
            return Some(bl);
        }
    }
    match anticheat::load_blocklist(&state.http, &config::raw_base_url()).await {
        Ok(bl) => {
            log::info!("[anticheat] Blocklist загружен, версия: {:?}", bl.version);
            *state.blocklist.lock().await = Some(bl.clone());
            Some(bl)
        }
        Err(e) => {
            log::warn!("[anticheat] Не удалось загрузить blocklist.json: {e}");
            state.blocklist.lock().await.clone()
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn releases_button() -> Vec<(String, String)> {
    vec![("Скачать лаунчер".into(), format!("https://github.com/{}/{}/releases", config::LAUNCHER_OWNER, config::LAUNCHER_REPO))]
}

async fn push_activity(state: &AppState, activity: Activity) {
    let mut g = state.discord.lock().await;
    if let Some(rpc) = g.as_mut() {
        if let Err(e) = rpc.set_activity(&activity).await {
            log::warn!("[discord] setActivity: {e}");
            *g = None; // соединение потеряно
        }
    }
}

pub async fn discord_launcher(state: &AppState) {
    let mc = current_mc(state).await;
    push_activity(state, Activity {
        details: format!("В лаунчере {}", config::APP_NAME),
        state: format!("Minecraft {mc} NeoForge"),
        large_image: "launcher_logo".into(), large_text: "ViPok Launcher".into(),
        small_image: "minecraft_icon".into(), small_text: format!("Minecraft {mc}"),
        start_timestamp: now_secs(), buttons: releases_button(),
    }).await;
}

pub async fn discord_playing(state: &AppState, players: Option<u64>) {
    let mc = current_mc(state).await;
    push_activity(state, Activity {
        details: format!("Играет на {}", state.selected_server().name),
        state: match players { Some(n) => format!("Онлайн: {n} игроков"), None => format!("Minecraft {mc} NeoForge") },
        large_image: "minecraft_icon".into(), large_text: format!("Minecraft {mc} NeoForge"),
        small_image: "launcher_logo".into(), small_text: "ViPok Launcher".into(),
        start_timestamp: now_secs(), buttons: releases_button(),
    }).await;
}

/// Подключение к Discord при старте; без Discord/с неверным Client ID просто тихо не работает.
pub async fn init_discord(state: &AppState) {
    let client_id = state
        .settings
        .get_str("discordClientId")
        .await
        .unwrap_or_else(|| if config::DISCORD_CLIENT_ID.is_empty() { "1234567890123456789".to_string() } else { config::DISCORD_CLIENT_ID.to_string() });
    match DiscordRpc::connect(&client_id).await {
        Ok(rpc) => {
            log::info!("[discord] RPC подключён");
            *state.discord.lock().await = Some(rpc);
            discord_launcher(state).await;
        }
        Err(e) => log::warn!("[discord] RPC недоступен (Discord не запущен или неверный Client ID): {e}"),
    }
}

// ───────────────────────────── пинг сервера ─────────────────────────────

/// Пинг сервера из servers.json (по id; без id — выбранный). Нет адреса → `{ online: false }`.
#[tauri::command]
pub async fn server_ping(state: State<'_, AppState>, server_id: Option<String>) -> Result<Json, ()> {
    let server = server_id.and_then(|id| state.server_by_id(&id)).unwrap_or_else(|| state.selected_server());
    let Some((host, port)) = server.host_port() else { return Ok(json!({ "online": false })) };
    let r = ping_server(&host, port, std::time::Duration::from_secs(4)).await;
    Ok(serde_json::to_value(r).unwrap_or_else(|_| json!({ "online": false })))
}

// ───────────────────────────── обновление лаунчера (tauri-plugin-updater) ─────────────────────────────
// Подписанные обновления: latest.json из GitHub Releases (endpoints в tauri.conf.json), подпись проверяется
// публичным ключом из конфигурации. Скачивание и установка выполняются плагином; сверка хеша/подписи обязательна.

/// Проверка обновления. Найденное сохраняется, чтобы `updater_install` ставил именно его.
#[tauri::command]
pub async fn updater_check(app: AppHandle, state: State<'_, AppState>) -> Result<Json, ()> {
    let local = env!("CARGO_PKG_VERSION");
    if state.settings.get_bool("devMode", false).await {
        log::info!("[update] devMode=true — проверка версии пропущена");
        return Ok(json!({ "hasUpdate": false, "local": local, "devMode": true }));
    }
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("[update] обновления не настроены: {e}");
            return Ok(json!({ "hasUpdate": false, "local": local, "error": format!("Обновления не настроены: {e}") }));
        }
    };
    Ok(match updater.check().await {
        Ok(Some(update)) => {
            let out = json!({
                "hasUpdate": true, "remote": update.version, "local": update.current_version,
                "notes": update.body, "date": update.date.map(|d| d.to_string()),
            });
            log::info!("[update] доступна версия {} (установлена {})", update.version, update.current_version);
            *state.pending_update.lock().await = Some(update);
            out
        }
        Ok(None) => json!({ "hasUpdate": false, "remote": local, "local": local }),
        Err(e) => {
            log::warn!("[update] проверка обновления не удалась: {e}");
            json!({ "hasUpdate": false, "local": local, "error": e.to_string() })
        }
    })
}

/// Скачивает (с прогрессом `launcher-dl-progress`), проверяет подпись, устанавливает и перезапускает лаунчер.
#[tauri::command]
pub async fn updater_install(app: AppHandle, state: State<'_, AppState>) -> Result<Json, ()> {
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(fail("Закрой Minecraft перед обновлением лаунчера"));
    }
    let Some(update) = state.pending_update.lock().await.take() else {
        return Ok(fail("Нет найденного обновления — сначала выполни проверку"));
    };
    let (done, app2) = (std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)), app.clone());
    let done2 = done.clone();
    let res = update
        .download_and_install(
            move |chunk, total| {
                let got = done2.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                let pct = total.filter(|t| *t > 0).map(|t| ((got as f64 / t as f64) * 100.0).min(100.0) as u64).unwrap_or(0);
                let _ = app2.emit("launcher-dl-progress", json!({ "pct": pct, "downloaded": got, "total": total }));
            },
            || log::info!("[update] обновление скачано, подпись проверена — запускаю установку"),
        )
        .await;
    match res {
        Ok(()) => {
            state.quitting.store(true, Ordering::SeqCst);
            // На Windows установщик сам завершает приложение; на остальных ОС перезапускаемся вручную.
            app.restart()
        }
        Err(e) => {
            log::error!("[update] установка обновления не удалась: {e}");
            Ok(fail(format!("Не удалось установить обновление: {e}")))
        }
    }
}

// ───────────────────────────── скины ─────────────────────────────

#[tauri::command]
pub async fn skin_get(state: State<'_, AppState>, username: Option<String>) -> Result<Json, ()> {
    // Кастомный скин приоритетнее: к API не обращаемся.
    if let Some(url) = skins::custom_skin_data_url(&state.custom_skin()).await {
        return Ok(json!({ "skinUrl": url, "isCustom": true }));
    }
    let url = skins::fetch_skin(&state.http, skins::SKIN_API, &username.unwrap_or_default()).await;
    Ok(json!({ "skinUrl": url, "isCustom": false }))
}

#[tauri::command]
pub async fn skin_upload(state: State<'_, AppState>) -> Result<Json, ()> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Выбери файл скина (PNG, 64x64)")
        .add_filter("PNG изображения", &["png"])
        .pick_file()
        .await;
    let Some(file) = picked else { return Ok(json!({ "ok": false })) };
    Ok(match skins::install_custom_skin(file.path(), &state.custom_skin()).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn skin_reset(state: State<'_, AppState>) -> Result<Json, ()> {
    skins::reset_custom_skin(&state.custom_skin()).await;
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── античит ─────────────────────────────

#[tauri::command]
pub async fn scan_cheats(state: State<'_, AppState>) -> Result<Json, ()> {
    if state.settings.get_bool("devMode", false).await && state.settings.get_bool("devDisableAnticheat", false).await {
        return Ok(json!({ "ok": true, "threats": [], "devSkipped": true }));
    }
    Ok(match ensure_blocklist(&state).await {
        None => json!({ "ok": true, "threats": [], "noBlocklist": true }),
        Some(bl) => json!({ "ok": true, "threats": anticheat::scan_for_cheats(&state.client_dir(), &bl).await }),
    })
}

// ───────────────────────────── Modrinth ─────────────────────────────

/// Единый набор полей запросов `modrinth-*` (как объекты, которые шлёт интерфейс).
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ModrinthArgs {
    query: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    limit: Option<u32>,
    project_id: Option<String>,
    url: Option<String>,
    filename: Option<String>,
    server_key: Option<String>,
    enabled: Option<bool>,
}

fn kind_of(a: &ModrinthArgs) -> PackKind {
    PackKind::parse(a.kind.as_deref().unwrap_or(""))
}

#[tauri::command]
pub async fn modrinth_search(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let mc = current_mc(&state).await;
    Ok(match modrinth::search(&state.http, modrinth::API, data.query.as_deref().unwrap_or(""), data.kind.as_deref().unwrap_or(""), data.limit, &mc).await {
        Ok(hits) => json!({ "ok": true, "hits": hits }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn modrinth_versions(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let mc = current_mc(&state).await;
    Ok(match modrinth::versions(&state.http, modrinth::API, data.project_id.as_deref().unwrap_or(""), &mc).await {
        Ok(v) => json!({ "ok": true, "versions": v }),
        Err(e) => fail(e),
    })
}

/// Скачивает пак в общую папку и автоматически активирует его для сервера (если передан serverKey).
#[tauri::command]
pub async fn modrinth_download(app: AppHandle, state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let kind = kind_of(&data);
    let (Some(url), Some(filename)) = (data.url.clone(), data.filename.clone()) else {
        return Ok(fail("Не указан url или имя файла"));
    };
    let app2 = app.clone();
    let name = filename.clone();
    let res = modrinth::download_pack(&state.http, &url, &state.shared_dir(kind), &filename, &CancelToken::new(), move |pct| {
        let _ = app2.emit("modrinth-dl-progress", json!({ "filename": name, "pct": pct }));
    })
    .await;
    match res {
        Ok(path) => {
            if let Some(sk) = data.server_key.as_deref().filter(|k| !k.is_empty()) {
                let _ = state.settings.update(|m| { modrinth::toggle_pack(m, sk, kind, &filename, true); }).await;
            }
            Ok(json!({ "ok": true, "path": path }))
        }
        Err(e) => {
            log::error!("[modrinth-download] {e}");
            Ok(fail(e))
        }
    }
}

#[tauri::command]
pub async fn modrinth_installed(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    Ok(json!({ "ok": true, "files": modrinth::list_installed(&state.shared_dir(kind_of(&data))).await }))
}

#[tauri::command]
pub async fn modrinth_active(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let sk = data.server_key.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| "default".into());
    Ok(json!({ "ok": true, "files": modrinth::active_packs(&state.settings.snapshot().await, &sk, kind_of(&data)) }))
}

#[tauri::command]
pub async fn modrinth_toggle(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let sk = data.server_key.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| "default".into());
    let Some(filename) = data.filename.clone() else { return Ok(fail("Не указано имя файла")) };
    let kind = kind_of(&data);
    let enabled = data.enabled.unwrap_or(false);
    let mut active = Vec::new();
    Ok(match state.settings.update(|m| active = modrinth::toggle_pack(m, &sk, kind, &filename, enabled)).await {
        Ok(()) => json!({ "ok": true, "active": active }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn modrinth_remove(state: State<'_, AppState>, data: ModrinthArgs) -> Result<Json, ()> {
    let Some(filename) = data.filename.clone() else { return Ok(fail("Не указано имя файла")) };
    let kind = kind_of(&data);
    if let Err(e) = modrinth::remove_pack_file(&state.shared_dir(kind), &filename).await {
        return Ok(fail(e));
    }
    Ok(match state.settings.update(|m| modrinth::remove_everywhere(m, kind, &filename)).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => fail(e),
    })
}

// ───────────────────────────── Discord ─────────────────────────────

#[tauri::command]
pub async fn discord_set_playing(state: State<'_, AppState>, player_count: Option<u64>) -> Result<Json, ()> {
    discord_playing(&state, player_count).await;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub async fn discord_set_launcher(state: State<'_, AppState>) -> Result<Json, ()> {
    discord_launcher(&state).await;
    Ok(json!({ "ok": true }))
}

// ───────────────────────────── проверка модов при «Играть» и сторож ─────────────────────────────

/// Период проверки во время игры и обновления чёрного манифеста.
const WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const BLOCKLIST_REFRESH: std::time::Duration = std::time::Duration::from_secs(300);

/// Проверка перед запуском: свежий манифест (или сохранённая копия без сети) → хеши модов → докачка/удаление
/// лишнего → недостающие файлы настроек. `Ok(None)` — проверить нечем (нет ни сети, ни кэша).
async fn verify_on_play(app: &AppHandle, state: &AppState, server: &ServerCfg, remove_unauthorized: bool) -> std::result::Result<Option<(Manifest, HashMapFile)>, String> {
    let base = server.manifest_base(&config::raw_base_url());
    let (manifest, hashes, defaults) = match fetch_manifest(&state.http, &base).await {
        Ok((m, h)) => {
            let d = fetch_defaults(&state.http, &base, &m).await.unwrap_or_default();
            save_cache(&state.client_dir(), &m, &h, &d).await;
            (m, h, d)
        }
        Err(e) => match load_cache(&state.client_dir()).await {
            Some(c) => {
                log::warn!("[verify] манифест недоступен ({e}) — моды проверяются по сохранённой копии");
                c
            }
            None => {
                log::warn!("[verify] манифест недоступен и сохранённой копии нет — проверка модов пропущена: {e}");
                return Ok(None);
            }
        },
    };

    let _ = app.emit("mods-status", json!({ "text": "🔍 Проверяю моды перед запуском..." }));
    let app2 = app.clone();
    let progress: ProgressFn = std::sync::Arc::new(move |e| {
        let _ = app2.emit("mods-progress", e);
    });
    let opts = DownloadOptions::default();
    let cancel = CancelToken::new();
    let rep = verify_mods(&state.http, &base, &state.client_dir(), &manifest, &hashes, remove_unauthorized, &opts, &cancel, Some(progress))
        .await
        .map_err(|e| format!("Проверка модов не пройдена: {e}"))?;
    if rep.downloaded > 0 || !rep.removed_mods.is_empty() {
        log::info!("[verify] перед запуском исправлено модов: {}, удалено лишних: {:?}", rep.downloaded, rep.removed_mods);
    }
    let _ = app.emit("mods-status", json!({ "text": "✓ Моды проверены" }));

    // Настройки по умолчанию: только то, чего нет на диске.
    if let Err(e) = sync_defaults(&state.http, &base, &state.client_dir(), &defaults, &hashes, false, None, &opts, &cancel, None).await {
        log::warn!("[defaults] {e}");
    }
    Ok(Some((manifest, hashes)))
}

fn violation_message(violations: &[Violation]) -> String {
    let list = |kind: ViolationKind| -> Vec<String> { violations.iter().filter(|v| v.kind == kind).map(|v| format!("• {}", v.name)).collect() };
    let (cheats, unauthorized, modified) = (list(ViolationKind::Cheat), list(ViolationKind::Unauthorized), list(ViolationKind::Modified));
    let mut parts: Vec<String> = Vec::new();
    if !cheats.is_empty() {
        parts.push(format!("Запрещённые файлы (читы):\n{}", cheats.join("\n")));
    }
    if !unauthorized.is_empty() {
        parts.push(format!("Моды, которых нет в списке сервера:\n{}", unauthorized.join("\n")));
    }
    if !modified.is_empty() {
        parts.push(format!("Моды с изменённым содержимым:\n{}", modified.join("\n")));
    }
    let head = if cheats.is_empty() { "Игра закрыта: обнаружены посторонние или изменённые моды" } else { "Игра закрыта: обнаружены запрещённые файлы" };
    format!("{head}\n\n{}\n\nСоединение с сервером прервано. Удали эти файлы и запусти игру снова.", parts.join("\n\n"))
}

fn spawn_watchdog(app: AppHandle, allowed: Option<std::collections::HashMap<String, String>>) {
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let mut snapshot = build_snapshot(&st.client_dir().join("mods")).await;
        let mut blocklist = st.blocklist.lock().await.clone();
        let (mut last_tick, mut last_bl) = (std::time::Instant::now(), std::time::Instant::now());
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if !st.game_running.load(Ordering::SeqCst) {
                break;
            }
            if last_tick.elapsed() < WATCHDOG_INTERVAL {
                continue;
            }
            last_tick = std::time::Instant::now();
            if last_bl.elapsed() >= BLOCKLIST_REFRESH {
                last_bl = std::time::Instant::now();
                if let Some(b) = load_blocklist(&st, true).await {
                    blocklist = Some(b);
                }
            }
            let violations = check_tick(&st.client_dir(), allowed.as_ref(), &mut snapshot, blocklist.as_ref()).await;
            if violations.is_empty() {
                continue;
            }
            log::error!("[watchdog] нарушения: {violations:?}");
            let tx = st.game_kill.lock().unwrap().take();
            if let Some(tx) = tx {
                let _ = tx.send(violation_message(&violations));
            }
            break;
        }
    });
}

// ───────────────────────────── настройки по умолчанию / настройки игры ─────────────────────────────

fn game_busy_error() -> Json {
    fail("Игра запущена — закрой Minecraft, иначе он перезапишет твои изменения при выходе.")
}

/// «Вернуть настройки по умолчанию»: перезаписывает файлы из defaults.json (все или один `path`).
#[tauri::command]
pub async fn defaults_reset(state: State<'_, AppState>, path: Option<String>) -> Result<Json, ()> {
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(game_busy_error());
    }
    let server = match require_playable(&state) {
        Ok(s) => s,
        Err(e) => return Ok(fail(e)),
    };
    let base = server.manifest_base(&config::raw_base_url());
    let (hashes, defaults) = match fetch_manifest(&state.http, &base).await {
        Ok((m, h)) => {
            let d = fetch_defaults(&state.http, &base, &m).await.unwrap_or_default();
            save_cache(&state.client_dir(), &m, &h, &d).await;
            (h, d)
        }
        Err(e) => match load_cache(&state.client_dir()).await {
            Some((_, h, d)) => (h, d),
            None => return Ok(fail(format!("Не удалось получить настройки по умолчанию: {e}"))),
        },
    };
    if defaults.is_empty() {
        return Ok(fail("В репозитории нет настроек по умолчанию (defaults.json)"));
    }
    match sync_defaults(&state.http, &base, &state.client_dir(), &defaults, &hashes, true, path.as_deref(), &DownloadOptions::default(), &CancelToken::new(), None).await {
        Ok(r) if r.failed.is_empty() => Ok(json!({ "ok": true, "restored": r.downloaded })),
        Ok(r) => Ok(fail(format!("Не удалось восстановить {} файл(ов): {}", r.failed.len(), r.failed[0].error))),
        Err(e) => Ok(fail(e)),
    }
}

#[tauri::command]
pub async fn game_options_get(state: State<'_, AppState>) -> Result<Json, ()> {
    Ok(match gamecfg::read_options(&state.client_dir()).await {
        Ok((exists, list)) => json!({
            "ok": true, "exists": exists,
            "options": list.into_iter().map(|(k, v)| json!({ "key": k, "value": v })).collect::<Vec<_>>(),
            "running": state.game_running.load(Ordering::SeqCst),
        }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn game_options_set(state: State<'_, AppState>, changes: std::collections::BTreeMap<String, String>) -> Result<Json, ()> {
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(game_busy_error());
    }
    Ok(match gamecfg::write_options(&state.client_dir(), &changes).await {
        Ok(()) => json!({ "ok": true, "saved": changes.len() }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn mod_configs_list(state: State<'_, AppState>) -> Result<Json, ()> {
    let files = gamecfg::list_configs(&state.client_dir()).await;
    // Файлы, которые можно вернуть к варианту из репозитория (есть в defaults.json).
    let resettable: Vec<String> = load_cache(&state.client_dir())
        .await
        .map(|(_, _, d)| d.into_iter().map(|f| f.path.replace('\\', "/")).filter(|p| p.starts_with("config/")).collect())
        .unwrap_or_default();
    Ok(json!({ "ok": true, "files": files, "resettable": resettable, "running": state.game_running.load(Ordering::SeqCst) }))
}

#[tauri::command]
pub async fn mod_config_read(state: State<'_, AppState>, path: String) -> Result<Json, ()> {
    Ok(match gamecfg::read_config(&state.client_dir(), &path).await {
        Ok(text) => json!({ "ok": true, "text": text }),
        Err(e) => fail(e),
    })
}

#[tauri::command]
pub async fn mod_config_write(state: State<'_, AppState>, path: String, text: String, force: Option<bool>) -> Result<Json, ()> {
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(game_busy_error());
    }
    Ok(match gamecfg::write_config(&state.client_dir(), &path, &text, !force.unwrap_or(false)).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => {
            let msg = e.to_string();
            json!({ "ok": false, "error": msg, "validation": msg.contains(gamecfg::VALIDATION_PREFIX) })
        }
    })
}

// ───────────────────────────── серверы и instance'ы ─────────────────────────────

/// Карточка сервера для интерфейса: поля из servers.json + состояние instance'а.
async fn server_view(state: &AppState, s: &ServerCfg) -> Json {
    let mut v = serde_json::to_value(s).unwrap_or_else(|_| json!({}));
    if let Some(o) = v.as_object_mut() {
        o.insert("installed".into(), json!(instance_installed(state, s).await));
        o.insert("selected".into(), json!(*state.selected.read().unwrap() == s.id));
        o.insert("playable".into(), json!(s.can_play()));
    }
    v
}

#[tauri::command]
pub async fn list_servers(state: State<'_, AppState>) -> Result<Json, ()> {
    let list = state.servers.read().unwrap().clone();
    let mut servers = Vec::with_capacity(list.len());
    for s in &list {
        servers.push(server_view(&state, s).await);
    }
    Ok(json!({ "ok": true, "selected": state.selected_server().id, "servers": servers }))
}

/// Выбор сервера: дальше синк, установка, настройки и запуск работают с его instance'ом.
#[tauri::command]
pub async fn select_server(state: State<'_, AppState>, id: String) -> Result<Json, ()> {
    let Some(server) = state.server_by_id(&id) else { return Ok(fail("Такого сервера нет в списке")) };
    if state.game_running.load(Ordering::SeqCst) {
        return Ok(fail("Игра запущена — закрой Minecraft, чтобы переключить сервер"));
    }
    if state.busy.load(Ordering::SeqCst) {
        return Ok(fail("Идёт синхронизация или установка — дождись завершения"));
    }
    *state.selected.write().unwrap() = server.id.clone();
    let _ = state.settings.set("selectedServer", Value::String(server.id.clone())).await;
    log::info!("[servers] выбран «{}» (instance {}, активен: {})", server.name, server.instance_id, server.active);
    Ok(json!({ "ok": true, "server": server_view(&state, &server).await }))
}

/// Состояние instance'а: установлен ли клиент, версии модов/Minecraft/Java.
#[tauri::command]
pub async fn get_instance_status(state: State<'_, AppState>, id: Option<String>) -> Result<Json, ()> {
    let server = id.and_then(|i| state.server_by_id(&i)).unwrap_or_else(|| state.selected_server());
    let launch = LaunchJson::load(&state.instance_dir_of(&server).join("launch.json")).await.ok();
    let java_major = launch.as_ref().map(|l| l.java_major_or_default());
    Ok(json!({
        "ok": true,
        "server": server_view(&state, &server).await,
        "modsVersion": state.inst_of(&server).await.get_str("modsVersion").await,
        "mcVersion": launch.as_ref().map(|l| l.mc_version.clone()).filter(|v| !v.is_empty()),
        "javaMajor": java_major,
        "javaInstalled": java_major.map(|m| state.java_for(m).exe_exists()),
        "busy": state.busy.load(Ordering::SeqCst),
    }))
}

/// Обновляет список серверов из репозитория (servers.json); при сбое остаётся кэш/встроенный список.
pub async fn refresh_servers_inner(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    match vilauncher_core::servers::fetch_servers(&state.http, &config::raw_base_url()).await {
        Ok((list, raw)) => {
            vilauncher_core::servers::save_servers_cache(&state.launcher_dir, &raw).await;
            let changed = *state.servers.read().unwrap() != list;
            if changed {
                let selected = state.selected.read().unwrap().clone();
                if !list.iter().any(|s| s.id == selected) {
                    let fallback = list.iter().find(|s| s.active).or(list.first()).map(|s| s.id.clone()).unwrap_or_default();
                    *state.selected.write().unwrap() = fallback;
                }
                *state.servers.write().unwrap() = list;
                log::info!("[servers] список серверов обновлён из репозитория");
                let _ = app.emit("servers-changed", json!({}));
            }
            changed
        }
        Err(e) => {
            log::info!("[servers] servers.json недоступен ({e}) — используется сохранённый/встроенный список");
            false
        }
    }
}

#[tauri::command]
pub async fn refresh_servers(app: AppHandle) -> Result<Json, ()> {
    let changed = refresh_servers_inner(&app).await;
    Ok(json!({ "ok": true, "changed": changed }))
}
