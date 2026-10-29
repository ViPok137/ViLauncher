//! Модуль 2: безопасное хранение токенов.
//!
//! Refresh-токен Microsoft/Mojang хранится ТОЛЬКО в системном хранилище ОС:
//!   * Windows — Credential Manager,
//!   * macOS   — Keychain,
//!   * Linux   — keyutils (ядро) через крейт `keyring`.
//! В `settings.json` секреты не попадают. Access-токен Minecraft живёт только в памяти.
//!
//! Нюанс Windows: blob учётной записи ограничен 2560 байтами, а `keyring` хранит пароль
//! в UTF-16 (2 байта на символ). Refresh-токены Microsoft бывают длинными, поэтому токен
//! режется на куски по `CHUNK_CHARS` символов и хранится в нескольких записях.

use crate::error::{CoreError, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const CHUNK_CHARS: usize = 400;
pub const DEFAULT_SERVICE: &str = "ViLauncher";
const REFRESH_KEY: &str = "ms-refresh-token";

/// Абстракция над хранилищем секретов: в проде — keyring ОС, в тестах — память.
pub trait SecretBackend: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// Системное хранилище ОС.
pub struct KeyringBackend {
    service: String,
}

impl KeyringBackend {
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }
    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new(&self.service, key)?)
    }
}

impl SecretBackend for KeyringBackend {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        Ok(self.entry(key)?.set_password(value)?)
    }
    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Хранилище в памяти — для тестов.
#[derive(Default)]
pub struct MemoryBackend {
    map: Mutex<HashMap<String, String>>,
}

impl MemoryBackend {
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn dump(&self) -> String {
        format!("{:?}", self.map.lock().unwrap())
    }
}

impl SecretBackend for MemoryBackend {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Слот для пароля «Запомнить меня» (заглушка входа).
pub const SAVED_PASSWORD_SLOT: &str = "saved-password";

/// Хранилище секретов с разбиением на куски. Слот `ms-refresh-token` — refresh-токен Microsoft,
/// `saved-password` — пароль «Запомнить меня».
#[derive(Clone)]
pub struct TokenStore {
    backend: Arc<dyn SecretBackend>,
}

impl TokenStore {
    pub fn new(backend: Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }

    /// Хранилище на системном keyring с сервисом по умолчанию.
    pub fn system() -> Self {
        Self::new(Arc::new(KeyringBackend::new(DEFAULT_SERVICE)))
    }

    fn count_key(slot: &str) -> String {
        format!("{slot}/count")
    }
    fn chunk_key(slot: &str, i: usize) -> String {
        format!("{slot}/{i}")
    }

    fn read_count(&self, slot: &str) -> Result<usize> {
        Ok(self
            .backend
            .get(&Self::count_key(slot))?
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(0))
    }

    pub fn save_secret(&self, slot: &str, value: &str) -> Result<()> {
        if value.is_empty() {
            return Err(CoreError::other("Пустой секрет не сохраняется"));
        }
        let chars: Vec<char> = value.chars().collect();
        let chunks: Vec<String> = chars.chunks(CHUNK_CHARS).map(|c| c.iter().collect()).collect();
        let old_count = self.read_count(slot)?;

        // Сначала куски, счётчик — последним: при сбое посередине останется старое
        // (целое) значение либо «ничего», но не склейка из разных значений.
        for (i, c) in chunks.iter().enumerate() {
            self.backend.set(&Self::chunk_key(slot, i), c)?;
        }
        self.backend.set(&Self::count_key(slot), &chunks.len().to_string())?;
        for i in chunks.len()..old_count {
            self.backend.delete(&Self::chunk_key(slot, i))?;
        }
        Ok(())
    }

    pub fn load_secret(&self, slot: &str) -> Result<Option<String>> {
        let n = self.read_count(slot)?;
        if n == 0 {
            return Ok(None);
        }
        let mut out = String::new();
        for i in 0..n {
            match self.backend.get(&Self::chunk_key(slot, i))? {
                Some(part) => out.push_str(&part),
                None => {
                    log::warn!("[secure_store] повреждён набор кусков «{slot}» (нет #{i}) — сбрасываем");
                    self.clear_secret(slot)?;
                    return Ok(None);
                }
            }
        }
        Ok(Some(out))
    }

    pub fn has_secret(&self, slot: &str) -> Result<bool> {
        Ok(self.read_count(slot)? > 0)
    }

    pub fn clear_secret(&self, slot: &str) -> Result<()> {
        let n = self.read_count(slot)?;
        self.backend.delete(&Self::count_key(slot))?;
        for i in 0..n.max(1) {
            self.backend.delete(&Self::chunk_key(slot, i))?;
        }
        Ok(())
    }

    // ── refresh-токен Microsoft ──
    pub fn save_refresh_token(&self, token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            return Err(CoreError::other("Пустой refresh-токен не сохраняется"));
        }
        self.save_secret(REFRESH_KEY, token)
    }
    pub fn load_refresh_token(&self) -> Result<Option<String>> {
        self.load_secret(REFRESH_KEY)
    }
    pub fn has_refresh_token(&self) -> Result<bool> {
        self.has_secret(REFRESH_KEY)
    }
    /// Удаляет refresh-токен Microsoft (пароль «Запомнить меня» не трогает).
    pub fn clear(&self) -> Result<()> {
        self.clear_secret(REFRESH_KEY)
    }
}

// ───────────────────── миграция со старого settings.json ─────────────────────

/// Ключи, содержащие refresh-токен: переносим в keyring и удаляем из файла.
const TOKEN_KEYS: &[&str] = &["refreshToken", "refresh_token", "msRefreshToken", "microsoftRefreshToken"];
/// Секреты, которым в файле делать нечего: просто удаляем.
const SECRET_KEYS: &[&str] = &[
    "password",
    "accessToken",
    "access_token",
    "mcAccessToken",
    "idToken",
    "id_token",
    "clientToken",
];

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub file_changed: bool,
    pub refresh_token_moved: bool,
    pub saved_password_moved: bool,
    pub removed_keys: Vec<String>,
}

/// Вычищает секреты из `settings.json` (атомарной перезаписью файла).
/// Refresh-токен, если он там нашёлся, переезжает в системное хранилище.
/// Если токен в keyring уже есть — он считается актуальнее и не перезаписывается.
pub fn migrate_legacy_settings(settings_path: &Path, store: &TokenStore) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();
    let raw = match std::fs::read_to_string(settings_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(e.into()),
    };
    let mut json: serde_json::Value = match serde_json::from_str(raw.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(_) => return Ok(report), // чужой/битый файл не трогаем
    };
    let Some(obj) = json.as_object_mut() else { return Ok(report) };

    for key in TOKEN_KEYS {
        if let Some(v) = obj.remove(*key) {
            report.removed_keys.push((*key).to_string());
            if let Some(tok) = v.as_str().filter(|s| !s.trim().is_empty()) {
                if !report.refresh_token_moved && !store.has_refresh_token()? {
                    store.save_refresh_token(tok)?;
                    report.refresh_token_moved = true;
                }
            }
        }
    }
    // Пароль «Запомнить меня»: переезжает в системное хранилище, из файла удаляется.
    if let Some(v) = obj.remove("savedPass") {
        report.removed_keys.push("savedPass".to_string());
        if let Some(pw) = v.as_str().filter(|s| !s.is_empty()) {
            if !store.has_secret(SAVED_PASSWORD_SLOT)? {
                store.save_secret(SAVED_PASSWORD_SLOT, pw)?;
                report.saved_password_moved = true;
            }
        }
    }
    for key in SECRET_KEYS {
        if obj.remove(*key).is_some() {
            report.removed_keys.push((*key).to_string());
        }
    }

    if report.removed_keys.is_empty() {
        return Ok(report);
    }
    let tmp = crate::fsutil::with_suffix(settings_path, ".tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&json)?)?;
    std::fs::rename(&tmp, settings_path)?;
    report.file_changed = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> (Arc<MemoryBackend>, TokenStore) {
        let m = Arc::new(MemoryBackend::default());
        (m.clone(), TokenStore::new(m))
    }

    #[test]
    fn long_token_roundtrip_is_chunked() {
        let (mem, store) = mem();
        let token: String = (0..2100).map(|i| char::from(b'A' + (i % 26) as u8)).collect();
        store.save_refresh_token(&token).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some(token.as_str()));
        // ни одна запись не длиннее лимита Windows (400 симв. * 2 байта UTF-16 << 2560)
        assert!(mem.dump().len() > token.len());
    }

    #[test]
    fn shorter_token_removes_stale_chunks() {
        let (mem, store) = mem();
        store.save_refresh_token(&"x".repeat(1500)).unwrap();
        let before = mem.len();
        store.save_refresh_token("short").unwrap();
        assert!(mem.len() < before);
        assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("short"));
    }

    #[test]
    fn unicode_is_safe_and_clear_removes_everything() {
        let (mem, store) = mem();
        let t = "токен🔑".repeat(300);
        store.save_refresh_token(&t).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap(), t);
        store.clear().unwrap();
        assert!(mem.is_empty(), "{}", mem.dump());
        assert_eq!(store.load_refresh_token().unwrap(), None);
    }

    #[test]
    fn missing_chunk_resets_instead_of_returning_garbage() {
        let (mem, store) = mem();
        store.save_refresh_token(&"y".repeat(900)).unwrap();
        mem.delete(&TokenStore::chunk_key(REFRESH_KEY, 1)).unwrap();
        assert_eq!(store.load_refresh_token().unwrap(), None);
        assert!(mem.is_empty());
    }

    #[test]
    fn migration_moves_token_and_purges_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, r#"{"nickname":"Vik","savedPass":"hunter2","refreshToken":"M.C5_REFRESH","accessToken":"abc","ram":{"max":4}}"#).unwrap();
        let (_m, store) = mem();
        let rep = migrate_legacy_settings(&p, &store).unwrap();
        assert!(rep.file_changed && rep.refresh_token_moved);
        let after = std::fs::read_to_string(&p).unwrap();
        for secret in ["hunter2", "M.C5_REFRESH", "abc", "savedPass", "refreshToken", "accessToken"] {
            assert!(!after.contains(secret), "{secret} остался в файле");
        }
        assert!(rep.saved_password_moved);
        assert_eq!(store.load_secret(SAVED_PASSWORD_SLOT).unwrap().as_deref(), Some("hunter2"), "пароль не потерян");
        assert!(after.contains("Vik") && after.contains("ram"), "обычные настройки сохранены");
        assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("M.C5_REFRESH"));
        // идемпотентность
        let rep2 = migrate_legacy_settings(&p, &store).unwrap();
        assert!(!rep2.file_changed);
    }

    #[test]
    fn migration_does_not_overwrite_existing_keyring_token() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, r#"{"refreshToken":"OLD"}"#).unwrap();
        let (_m, store) = mem();
        store.save_refresh_token("NEW").unwrap();
        migrate_legacy_settings(&p, &store).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("NEW"));
        assert!(!std::fs::read_to_string(&p).unwrap().contains("OLD"));
    }

    #[test]
    fn saved_password_slot_is_independent_from_refresh_token() {
        let (_m, store) = mem();
        store.save_secret(SAVED_PASSWORD_SLOT, "p@ss").unwrap();
        store.save_refresh_token("M.C5_X").unwrap();
        store.clear().unwrap(); // выход из Microsoft не стирает «Запомнить меня»
        assert_eq!(store.load_secret(SAVED_PASSWORD_SLOT).unwrap().as_deref(), Some("p@ss"));
        store.clear_secret(SAVED_PASSWORD_SLOT).unwrap();
        assert_eq!(store.load_secret(SAVED_PASSWORD_SLOT).unwrap(), None);
    }

    #[test]
    fn migration_ignores_missing_or_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let (_m, store) = mem();
        assert!(!migrate_legacy_settings(&dir.path().join("nope.json"), &store).unwrap().file_changed);
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "not json {{{").unwrap();
        assert!(!migrate_legacy_settings(&p, &store).unwrap().file_changed);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "not json {{{");
    }

    /// Реальный keyring ОС. Запуск вручную: `cargo test -- --ignored real_os_keyring`
    #[test]
    #[ignore]
    fn real_os_keyring_roundtrip() {
        let store = TokenStore::new(Arc::new(KeyringBackend::new("ViLauncher-test")));
        let t = "M.C5_".to_string() + &"z".repeat(1800);
        store.save_refresh_token(&t).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap(), t);
        store.clear().unwrap();
        assert_eq!(store.load_refresh_token().unwrap(), None);
    }
}
