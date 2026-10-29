//! Настройки сервера и репозитория модов (зеркало `src/config.js`).

/// Имя лаунчера в подсказке трея (сами серверы описаны в servers.json).
pub const APP_NAME: &str = "ViLauncher";
/// Запасная версия Minecraft, если не удалось определить ни по servers.json, ни по manifest.json.
pub const MC_VERSION: &str = "1.21.1";

pub const MODS_OWNER: &str = "ViPok137";
pub const MODS_REPO: &str = "ViLauncher";
pub const MODS_BRANCH: &str = "main";

/// GitHub-репозиторий с релизами лаунчера (проверка обновлений).
pub const LAUNCHER_OWNER: &str = "ViPok137";
pub const LAUNCHER_REPO: &str = "ViLauncher";

/// Discord Client ID (https://discord.com/developers/applications); пусто — Rich Presence не работает.
pub const DISCORD_CLIENT_ID: &str = "";

pub const DEFAULT_USERNAME: &str = "Player";
/// Код доступа к панели разработчика (как в config.js).
pub const DEV_CODE: &str = "445566";

/// Azure Application (client) ID для входа через Microsoft.
/// Задаётся при сборке: `VILAUNCHER_MS_CLIENT_ID=xxxxxxxx-... npx tauri build`
/// либо в settings.json ключом `msClientId`.
pub const MS_CLIENT_ID: &str = match option_env!("VILAUNCHER_MS_CLIENT_ID") {
    Some(v) => v,
    None => "",
};

pub fn raw_base_url() -> String {
    format!("https://raw.githubusercontent.com/{MODS_OWNER}/{MODS_REPO}/{MODS_BRANCH}")
}
