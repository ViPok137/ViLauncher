//! `settings.json` — несекретные настройки. Запись атомарная (tmp + rename), доступ
//! сериализован мьютексом: в Electron-версии `loadStore()` был async, но вызывался без
//! `await`, из-за чего настройки могли портиться — здесь такой гонки нет.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub struct Settings {
    path: PathBuf,
    data: Mutex<Map<String, Value>>,
}

impl Settings {
    pub async fn load(path: &Path) -> Self {
        let data = match tokio::fs::read_to_string(path).await {
            Ok(t) => match serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')) {
                Ok(Value::Object(m)) => m,
                _ => Map::new(),
            },
            Err(_) => Map::new(),
        };
        Self { path: path.to_path_buf(), data: Mutex::new(data) }
    }

    pub async fn get(&self, key: &str) -> Option<Value> {
        self.data.lock().await.get(key).cloned()
    }

    /// Копия всех настроек (для чистых функций ядра).
    pub async fn snapshot(&self) -> Map<String, Value> {
        self.data.lock().await.clone()
    }

    pub async fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key).await? {
            Value::String(s) if !s.is_empty() => Some(s),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    pub async fn get_u64(&self, key: &str) -> Option<u64> {
        match self.get(key).await? {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub async fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).await.and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// Применяет изменения и сразу сохраняет файл.
    pub async fn update(&self, f: impl FnOnce(&mut Map<String, Value>)) -> std::io::Result<()> {
        let mut data = self.data.lock().await;
        f(&mut data);
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = vilauncher_core::fsutil::with_suffix(&self.path, ".tmp");
        let text = serde_json::to_string_pretty(&Value::Object(data.clone()))?;
        tokio::fs::write(&tmp, text).await?;
        tokio::fs::rename(&tmp, &self.path).await
    }

    pub async fn set(&self, key: &str, value: Value) -> std::io::Result<()> {
        let key = key.to_string();
        self.update(move |m| {
            m.insert(key, value);
        })
        .await
    }
}
