use crate::settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use tokio::sync::Mutex;
use vilauncher_core::anticheat::Blocklist;
use vilauncher_core::auth::{AuthManager, McSession};
use vilauncher_core::discord::DiscordRpc;
use vilauncher_core::downloader::CancelToken;
use vilauncher_core::instances::instance_dir;
use vilauncher_core::java_runtime::JavaRuntime;
use vilauncher_core::secure_store::TokenStore;
use vilauncher_core::servers::ServerCfg;

pub struct AppState {
    pub launcher_dir: PathBuf,
    pub http: reqwest_client::Client,
    pub tokens: TokenStore,
    /// Общие настройки лаунчера (ник, RAM, выбранный сервер…) — `settings.json`.
    pub settings: Settings,
    pub session: Mutex<Option<McSession>>,
    pub sync_cancel: StdMutex<CancelToken>,
    pub auth_cancel: StdMutex<CancelToken>,
    pub game_running: AtomicBool,
    /// Кэш blocklist.json (как `cachedBlocklist` в оригинале).
    pub blocklist: Mutex<Option<Blocklist>>,
    pub discord: Mutex<Option<DiscordRpc>>,
    /// true — выход из трея; иначе закрытие окна только скрывает его.
    pub quitting: AtomicBool,
    /// Канал, которым сторож закрывает запущенную игру (с причиной для предупреждения).
    pub game_kill: StdMutex<Option<tokio::sync::oneshot::Sender<String>>>,
    /// Список серверов (servers.json), порядок для отображения.
    pub servers: RwLock<Vec<ServerCfg>>,
    /// id выбранного сервера. Все операции (синк, установка, запуск, настройки) идут с его instance'ом.
    pub selected: RwLock<String>,
    /// Настройки по instance'ам (`instances/<id>/instance.json`): версия модов, ключ установки.
    pub instance_settings: StdMutex<HashMap<String, Arc<Settings>>>,
    /// Идёт синхронизация/установка/проверка — выбор другого сервера в это время запрещён.
    pub busy: AtomicBool,
    /// Найденное обновление лаунчера, ожидает установки.
    pub pending_update: Mutex<Option<tauri_plugin_updater::Update>>,
}

/// Снимает флаг занятости при выходе из области видимости.
pub struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl AppState {
    /// Выбранный сервер (если выбранного почему-то нет в списке — первый).
    pub fn selected_server(&self) -> ServerCfg {
        let id = self.selected.read().unwrap().clone();
        let list = self.servers.read().unwrap();
        list.iter().find(|s| s.id == id).or_else(|| list.first()).cloned().expect("список серверов не может быть пустым")
    }

    pub fn server_by_id(&self, id: &str) -> Option<ServerCfg> {
        self.servers.read().unwrap().iter().find(|s| s.id == id).cloned()
    }

    /// Каталог клиента выбранного сервера: `instances/<instanceId>`.
    pub fn client_dir(&self) -> PathBuf {
        self.instance_dir_of(&self.selected_server())
    }

    pub fn instance_dir_of(&self, s: &ServerCfg) -> PathBuf {
        // instanceId проверен при разборе servers.json; запасной вариант на случай ошибки — каталог по id.
        instance_dir(&self.launcher_dir, &s.instance_id).unwrap_or_else(|| self.launcher_dir.join("instances").join("invalid"))
    }

    pub fn launch_json(&self) -> PathBuf {
        self.client_dir().join("launch.json")
    }

    pub fn custom_skin(&self) -> PathBuf {
        self.launcher_dir.join("custom_skin.png")
    }

    pub fn shared_dir(&self, kind: vilauncher_core::modrinth::PackKind) -> PathBuf {
        self.launcher_dir.join("shared").join(kind.folder())
    }

    /// Изолированная Java нужной версии (`runtime/java<major>`); общая для всех instance'ов с этой версией.
    pub fn java_for(&self, major: u32) -> JavaRuntime {
        JavaRuntime::with_major(&self.launcher_dir, major)
    }

    /// Настройки выбранного instance'а.
    pub async fn inst(&self) -> Arc<Settings> {
        self.inst_of(&self.selected_server()).await
    }

    pub async fn inst_of(&self, s: &ServerCfg) -> Arc<Settings> {
        if let Some(found) = self.instance_settings.lock().unwrap().get(&s.instance_id) {
            return found.clone();
        }
        let loaded = Arc::new(Settings::load(&self.instance_dir_of(s).join("instance.json")).await);
        self.instance_settings.lock().unwrap().entry(s.instance_id.clone()).or_insert(loaded).clone()
    }

    /// После удаления instance'а in-memory копия его настроек устарела.
    pub fn forget_instance_settings(&self, instance_id: &str) {
        self.instance_settings.lock().unwrap().remove(instance_id);
    }

    /// `None`, если уже идёт другая операция.
    pub fn try_busy(&self) -> Option<BusyGuard<'_>> {
        if self.busy.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(BusyGuard(&self.busy))
        }
    }

    /// Менеджер авторизации с актуальным client_id (из settings.json или из сборки).
    pub async fn auth_manager(&self) -> AuthManager {
        let from_settings = self.settings.get_str("msClientId").await;
        let client_id = from_settings.unwrap_or_else(|| crate::config::MS_CLIENT_ID.to_string());
        AuthManager::new(vilauncher_core::auth::MicrosoftAuth::new(self.http.clone(), client_id), self.tokens.clone())
    }
}

/// Реэкспорт, чтобы не тянуть `reqwest` в зависимости оболочки напрямую.
pub mod reqwest_client {
    pub use vilauncher_core::downloader::HttpClient as Client;
}
