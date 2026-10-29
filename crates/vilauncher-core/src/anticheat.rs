//! Античит — «чёрный манифест» `blocklist.json` с ключевыми словами читов.
//!
//! ```json
//! { "_version": 3,
//!   "mods":          ["wurst", "meteor-client"],   // имена файлов в mods/
//!   "resourcepacks": ["xray"],                     // имена файлов в resourcepacks/
//!   "shaderpacks":   [],                           // имена файлов в shaderpacks/
//!   "any":           ["killaura"] }                // применяется ко всем трём папкам
//! ```
//! Ключевое слово совпадает, если входит (без учёта регистра) в имя файла без расширения.

use crate::downloader::{fetch_text, HttpClient};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Blocklist {
    #[serde(default, rename = "_version")]
    pub version: Option<serde_json::Value>,
    #[serde(default)]
    pub mods: Vec<String>,
    #[serde(default)]
    pub resourcepacks: Vec<String>,
    #[serde(default)]
    pub shaderpacks: Vec<String>,
    /// Ключевые слова для всех папок сразу.
    #[serde(default)]
    pub any: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Threat {
    pub name: String,
    pub path: PathBuf,
}

pub async fn load_blocklist(client: &HttpClient, raw_base: &str) -> Result<Blocklist> {
    let text = fetch_text(client, &format!("{}/blocklist.json", raw_base.trim_end_matches('/')), 1).await?;
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
}

/// Имя без расширения, в нижнем регистре.
pub fn normalize_file_name(name: &str) -> String {
    Path::new(name).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Подстрока нормализованного имени (пустые записи blocklist игнорируются, иначе они совпали бы со всем).
pub fn matches_blocklist(name: &str, list: &[String]) -> bool {
    let n = normalize_file_name(name);
    list.iter().filter(|e| !e.trim().is_empty()).any(|e| n.contains(&e.to_lowercase()))
}

pub async fn scan_for_cheats(client_dir: &Path, bl: &Blocklist) -> Vec<Threat> {
    let mut threats = Vec::new();
    for (dir, list, ext) in [("mods", &bl.mods, ".jar"), ("resourcepacks", &bl.resourcepacks, ".zip"), ("shaderpacks", &bl.shaderpacks, ".zip")] {
        let dir = client_dir.join(dir);
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { continue };
        let mut names = Vec::new();
        while let Ok(Some(e)) = rd.next_entry().await {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        for f in names {
            if !f.ends_with(ext) && !f.ends_with(".jar") {
                continue;
            }
            if matches_blocklist(&f, list) || matches_blocklist(&f, &bl.any) {
                log::warn!("[anticheat] Обнаружена угроза: {}", dir.join(&f).display());
                threats.push(Threat { path: dir.join(&f), name: f });
            }
        }
    }
    threats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_rules() {
        let l = vec!["wurst".to_string(), "Meteor-Client".to_string(), "".to_string()];
        assert!(matches_blocklist("Wurst-1.2.jar", &l));
        assert!(matches_blocklist("meteor-client-0.5.jar", &l));
        assert!(!matches_blocklist("create-1.21.jar", &l));
        assert!(!matches_blocklist("anything.jar", &[String::new()]), "пустая запись не должна блокировать всё");
        assert_eq!(normalize_file_name("My.Mod.v1.JAR"), "my.mod.v1");
    }

    #[tokio::test]
    async fn scans_mods_and_resourcepacks() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = (dir.path().join("mods"), dir.path().join("resourcepacks"));
        std::fs::create_dir_all(&m).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        for (d, n) in [(&m, "create.jar"), (&m, "XRay-Mod.jar"), (&m, "xray-notes.txt"), (&r, "xray-pack.zip"), (&r, "nice.zip"), (&r, "xray-x.jar")] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let bl = Blocklist { version: None, mods: vec!["xray".into()], resourcepacks: vec!["xray".into()], ..Default::default() };
        let names: Vec<String> = scan_for_cheats(dir.path(), &bl).await.into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["XRay-Mod.jar", "xray-pack.zip", "xray-x.jar"]);
        // нет папок — угроз нет
        assert!(scan_for_cheats(&dir.path().join("nope"), &bl).await.is_empty());
    }

    #[tokio::test]
    async fn shaderpacks_and_any_keywords() {
        let dir = tempfile::tempdir().unwrap();
        for (d, n) in [("mods", "cool-killaura.jar"), ("mods", "create.jar"), ("shaderpacks", "BSL.zip"), ("shaderpacks", "xray-shader.zip"), ("resourcepacks", "KillAura-pack.zip")] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
            std::fs::write(dir.path().join(d).join(n), b"x").unwrap();
        }
        let bl: Blocklist = serde_json::from_str(r#"{"any":["killaura"],"shaderpacks":["xray"]}"#).unwrap();
        let mut names: Vec<String> = scan_for_cheats(dir.path(), &bl).await.into_iter().map(|t| t.name).collect();
        names.sort();
        assert_eq!(names, vec!["KillAura-pack.zip", "cool-killaura.jar", "xray-shader.zip"]);
    }

    #[test]
    fn blocklist_json_shape() {
        let b: Blocklist = serde_json::from_str(r#"{"_version":3,"mods":["a"],"resourcepacks":["b"]}"#).unwrap();
        assert_eq!((b.mods.len(), b.resourcepacks.len()), (1, 1));
        let empty: Blocklist = serde_json::from_str("{}").unwrap();
        assert!(empty.mods.is_empty());
    }
}
