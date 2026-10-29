//! Настройки игрока: `options.txt` (Minecraft) и конфиги модов (`config/**`).
//!
//! Правки лежат прямо в файлах клиента, а файлы по умолчанию скачиваются только если файла нет
//! (или игрок нажал «Вернуть по умолчанию»), поэтому изменения игрока переживают обновления.

use crate::error::{CoreError, Result};
use crate::fsutil::{safe_join, with_suffix};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_CFG_BYTES: u64 = 1 << 20;
/// Префикс ошибки проверки синтаксиса — по нему интерфейс предлагает «Сохранить без проверки».
pub const VALIDATION_PREFIX: &str = "Проверка синтаксиса не пройдена";
const EDITABLE_EXT: &[&str] = &["toml", "json", "json5", "cfg", "txt", "properties", "yaml", "yml", "conf", "ini", "snbt"];

// ───────────────────────────── options.txt ─────────────────────────────

/// `key:value` построчно, в порядке файла (строки без `:` пропускаются).
pub fn parse_options(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn check_option(k: &str, v: &str) -> Result<()> {
    if k.is_empty() || k.contains([':', '\n', '\r']) {
        return Err(CoreError::other(format!("Недопустимое имя параметра: «{k}»")));
    }
    if v.contains(['\n', '\r']) {
        return Err(CoreError::other(format!("Недопустимое значение параметра «{k}»")));
    }
    Ok(())
}

/// Применяет изменения к тексту `options.txt`: существующие ключи обновляются на месте, новые
/// дописываются в конец, все прочие строки и порядок (и тип перевода строки) сохраняются.
pub fn apply_options(text: &str, changes: &BTreeMap<String, String>) -> Result<String> {
    for (k, v) in changes {
        check_option(k, v)?;
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut remaining = changes.clone();
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        match line.split_once(':') {
            Some((k, _)) if remaining.contains_key(k) => {
                let v = remaining.remove(k).unwrap_or_default();
                out.push(format!("{k}:{v}"));
            }
            _ => out.push(line.to_string()),
        }
    }
    for (k, v) in remaining {
        out.push(format!("{k}:{v}"));
    }
    let mut s = out.join(eol);
    s.push_str(eol);
    Ok(s)
}

async fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = with_suffix(path, ".tmp");
    tokio::fs::write(&tmp, text).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub fn options_path(client_dir: &Path) -> PathBuf {
    client_dir.join("options.txt")
}

/// Пусто, если файла ещё нет (создаётся при первом запуске игры).
pub async fn read_options(client_dir: &Path) -> Result<(bool, Vec<(String, String)>)> {
    match tokio::fs::read_to_string(options_path(client_dir)).await {
        Ok(t) => Ok((true, parse_options(&t))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((false, Vec::new())),
        Err(e) => Err(e.into()),
    }
}

pub async fn write_options(client_dir: &Path, changes: &BTreeMap<String, String>) -> Result<()> {
    let current = tokio::fs::read_to_string(options_path(client_dir)).await.unwrap_or_default();
    write_atomic(&options_path(client_dir), &apply_options(&current, changes)?).await
}

// ───────────────────────────── конфиги модов ─────────────────────────────

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConfigFile {
    /// Путь внутри клиента, например `config/create-common.toml`.
    pub path: String,
    pub size: u64,
}

fn is_editable(name: &str) -> bool {
    Path::new(name).extension().map(|e| EDITABLE_EXT.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<ConfigFile>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel_path = format!("{rel}/{name}");
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth < 6 {
            walk(&e.path(), &rel_path, depth + 1, out);
        } else if t.is_file() && is_editable(&name) {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            if size <= MAX_CFG_BYTES {
                out.push(ConfigFile { path: rel_path, size });
            }
        }
    }
}

/// Редактируемые файлы настроек модов (`config/**`, текстовые, до 1 МБ).
pub async fn list_configs(client_dir: &Path) -> Vec<ConfigFile> {
    let dir = client_dir.join("config");
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        walk(&dir, "config", 0, &mut out);
        out.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
        out
    })
    .await
    .unwrap_or_default()
}

/// Путь допустим только внутри `config/`; существование файла обязательно (новые файлы не создаются).
fn config_target(client_dir: &Path, rel: &str) -> Result<PathBuf> {
    let norm = rel.replace('\\', "/");
    let inner = norm.strip_prefix("config/").ok_or_else(|| CoreError::UnsafePath(rel.to_string()))?;
    let p = safe_join(&client_dir.join("config"), inner)?;
    if !is_editable(inner) {
        return Err(CoreError::other("Этот тип файла не редактируется"));
    }
    Ok(p)
}

pub async fn read_config(client_dir: &Path, rel: &str) -> Result<String> {
    let p = config_target(client_dir, rel)?;
    let meta = tokio::fs::metadata(&p).await?;
    if meta.len() > MAX_CFG_BYTES {
        return Err(CoreError::other("Файл слишком большой для редактирования (больше 1 МБ)"));
    }
    Ok(String::from_utf8_lossy(&tokio::fs::read(&p).await?).into_owned())
}

/// Быстрая проверка синтаксиса, чтобы не сломать мод (JSON и TOML).
pub fn validate_config(rel: &str, text: &str) -> std::result::Result<(), String> {
    let ext = Path::new(rel).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "json" => serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).map(|_| ()).map_err(|e| e.to_string()),
        "toml" => text.parse::<toml::Table>().map(|_| ()).map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

pub async fn write_config(client_dir: &Path, rel: &str, text: &str, validate: bool) -> Result<()> {
    if text.len() as u64 > MAX_CFG_BYTES {
        return Err(CoreError::other("Файл слишком большой (больше 1 МБ)"));
    }
    let p = config_target(client_dir, rel)?;
    if !tokio::fs::metadata(&p).await.map(|m| m.is_file()).unwrap_or(false) {
        return Err(CoreError::other("Файл не найден — сохранять можно только существующие настройки"));
    }
    if validate {
        validate_config(rel, text).map_err(|e| CoreError::other(format!("{VALIDATION_PREFIX}: {e}")))?;
    }
    write_atomic(&p, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn options_parse_and_apply_preserve_everything_else() {
        let text = "version:3955\nfov:0.0\nkey_key.forward:key.keyboard.w\nresourcePacks:[\"vanilla\",\"file/x.zip\"]\n\nlang:en_us\n";
        let p = parse_options(text);
        assert_eq!(p[0], ("version".into(), "3955".into()));
        assert_eq!(p[3].1, "[\"vanilla\",\"file/x.zip\"]", "двоеточия внутри значения сохраняются");
        let out = apply_options(text, &ch(&[("fov", "0.5"), ("lang", "ru_ru"), ("renderDistance", "16")])).unwrap();
        assert_eq!(out, "version:3955\nfov:0.5\nkey_key.forward:key.keyboard.w\nresourcePacks:[\"vanilla\",\"file/x.zip\"]\n\nlang:ru_ru\nrenderDistance:16\n");
        // CRLF сохраняется
        let crlf = apply_options("a:1\r\nb:2\r\n", &ch(&[("b", "3"), ("c", "4")])).unwrap();
        assert_eq!(crlf, "a:1\r\nb:3\r\nc:4\r\n");
        // пустой файл
        assert_eq!(apply_options("", &ch(&[("fov", "0.1")])).unwrap(), "fov:0.1\n");
    }

    #[test]
    fn options_reject_injection() {
        for (k, v) in [("a:b", "1"), ("a\nb", "1"), ("", "1"), ("k", "x\ny"), ("k", "x\ry")] {
            assert!(apply_options("a:1\n", &ch(&[(k, v)])).is_err(), "{k:?}={v:?}");
        }
    }

    #[tokio::test]
    async fn options_file_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let (exists, list) = read_options(d.path()).await.unwrap();
        assert!(!exists && list.is_empty());
        write_options(d.path(), &ch(&[("fov", "0.25")])).await.unwrap(); // файла не было — создаётся
        write_options(d.path(), &ch(&[("maxFps", "120")])).await.unwrap();
        let (exists, list) = read_options(d.path()).await.unwrap();
        assert!(exists);
        assert_eq!(list, vec![("fov".into(), "0.25".into()), ("maxFps".into(), "120".into())]);
        assert!(!d.path().join("options.txt.tmp").exists());
    }

    #[tokio::test]
    async fn config_listing_reading_writing_and_safety() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("config");
        std::fs::create_dir_all(c.join("create/sub")).unwrap();
        std::fs::write(c.join("create-common.toml"), "a = 1\n").unwrap();
        std::fs::write(c.join("create/sub/x.json"), "{\"a\":1}").unwrap();
        std::fs::write(c.join("Z-readme.txt"), "hi").unwrap();
        std::fs::write(c.join("binary.dat"), [0u8; 4]).unwrap();
        std::fs::write(c.join("huge.toml"), vec![b'#'; 1_100_000]).unwrap();
        let names: Vec<String> = list_configs(d.path()).await.into_iter().map(|f| f.path).collect();
        assert_eq!(names, vec!["config/create-common.toml", "config/create/sub/x.json", "config/Z-readme.txt"]);

        assert_eq!(read_config(d.path(), "config/create-common.toml").await.unwrap(), "a = 1\n");
        write_config(d.path(), "config/create-common.toml", "a = 2\n", true).await.unwrap();
        assert_eq!(read_config(d.path(), "config/create-common.toml").await.unwrap(), "a = 2\n");

        // проверка синтаксиса: файл не портится, но «без проверки» сохранить можно
        let err = write_config(d.path(), "config/create-common.toml", "a = = 2", true).await.unwrap_err().to_string();
        assert!(err.starts_with(VALIDATION_PREFIX), "{err}");
        assert_eq!(read_config(d.path(), "config/create-common.toml").await.unwrap(), "a = 2\n");
        assert!(write_config(d.path(), "config/create/sub/x.json", "{bad", true).await.is_err());
        write_config(d.path(), "config/create/sub/x.json", "{bad // комментарий", false).await.unwrap();

        // безопасность
        for bad in ["config/../options.txt", "../etc/passwd", "options.txt", "config/binary.dat", "mods/a.jar", "C:/x.toml"] {
            assert!(read_config(d.path(), bad).await.is_err(), "{bad}");
            assert!(write_config(d.path(), bad, "x", false).await.is_err(), "{bad}");
        }
        assert!(read_config(d.path(), "config/huge.toml").await.is_err());
        assert!(write_config(d.path(), "config/new.toml", "x=1", false).await.is_err(), "новые файлы не создаются");
    }
}
