//! Единый тип ошибок ядра. Сериализуется в строку, чтобы Tauri-команды
//! могли отдавать его во фронтенд как `Err(String)`.

use serde::{Serialize, Serializer};

#[derive(thiserror::Error, Debug)]
pub enum CoreError {
    #[error("Ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),

    #[error("Ошибка сети: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Ошибка системного хранилища ключей: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("Ошибка ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Небезопасный путь в манифесте: {0}")]
    UnsafePath(String),

    #[error("Хеш не совпал для {path}: ожидался {expected}, получен {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("Размер файла не совпал для {path}: ожидалось {expected} байт, получено {actual}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error("Операция отменена")]
    Cancelled,

    #[error("Ошибка авторизации: {0}")]
    Auth(String),

    #[error("Требуется повторный вход через Microsoft")]
    AuthRequired,

    #[error("Синхронизация завершилась с ошибками ({count} файл(ов)): {first}")]
    SyncFailed { count: usize, first: String },

    #[error("{0}")]
    Other(String),
}

impl CoreError {
    pub fn other(msg: impl Into<String>) -> Self {
        CoreError::Other(msg.into())
    }
}

impl Serialize for CoreError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
