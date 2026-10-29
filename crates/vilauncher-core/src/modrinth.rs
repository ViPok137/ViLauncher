//! Modrinth: шейдеры и ресурспаки (порт `modrinth-*`).
//!
//! Скачанные паки хранятся в общих папках `shared/shaderpacks|resourcepacks`; для каждого сервера
//! в `settings.json` (`servers.<key>.shaders|resourcepacks`) лежит список активных. Перед запуском
//! игры из `shared/` создаются символьные ссылки в папку клиента.

use crate::downloader::{download_all, CancelToken, DeltaPlan, DownloadOptions, DownloadReason, HttpClient, PlannedFile, ProgressFn, RemoteFile};
use crate::error::{CoreError, Result};
use crate::fsutil::{encode_component, safe_file_name};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const API: &str = "https://api.modrinth.com/v2";
const UA: &str = "ViLauncher/1.0 (github.com/ViPok137/ViLauncher)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackKind {
    Shader,
    ResourcePack,
}

impl PackKind {
    /// Как в оригинале: `type === 'shader'` → шейдеры, всё остальное → ресурспаки.
    pub fn parse(s: &str) -> Self {
        if s == "shader" { Self::Shader } else { Self::ResourcePack }
    }
    fn settings_key(self) -> &'static str {
        match self {
            Self::Shader => "shaders",
            Self::ResourcePack => "resourcepacks",
        }
    }
    pub fn folder(self) -> &'static str {
        match self {
            Self::Shader => "shaderpacks",
            Self::ResourcePack => "resourcepacks",
        }
    }
}

// ───────────────────────────── API ─────────────────────────────

async fn fetch_json(client: &HttpClient, url: &str) -> Result<Value> {
    let resp = client.get(url).header("User-Agent", UA).send().await?;
    if resp.status().as_u16() >= 400 {
        return Err(CoreError::other(format!("HTTP {}", resp.status().as_u16())));
    }
    Ok(resp.json().await?)
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub id: Value,
    pub title: Value,
    pub description: Value,
    pub icon_url: Value,
    pub downloads: Value,
    pub author: Value,
}

pub async fn search(client: &HttpClient, api: &str, query: &str, project_type: &str, limit: Option<u32>, mc_version: &str) -> Result<Vec<Hit>> {
    let facets = encode_component(&json!([[format!("project_type:{project_type}")], [format!("versions:{mc_version}")]]).to_string());
    let url = format!("{api}/search?query={}&facets={facets}&limit={}", encode_component(query), limit.unwrap_or(20));
    let data = fetch_json(client, &url).await?;
    let get = |h: &Value, k: &str| h.get(k).cloned().unwrap_or(Value::Null);
    Ok(data
        .get("hits")
        .and_then(|h| h.as_array())
        .map(|a| {
            a.iter()
                .map(|h| Hit { id: get(h, "project_id"), title: get(h, "title"), description: get(h, "description"), icon_url: get(h, "icon_url"), downloads: get(h, "downloads"), author: get(h, "author") })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VersionFile {
    pub url: Value,
    pub filename: Value,
    pub size: Value,
    pub primary: Value,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: Value,
    pub version_name: Value,
    pub loaders: Value,
    pub files: Vec<VersionFile>,
}

/// Версии проекта под нужную версию Minecraft (первые 10).
pub async fn versions(client: &HttpClient, api: &str, project_id: &str, mc_version: &str) -> Result<Vec<VersionInfo>> {
    let url = format!("{api}/project/{}/version?game_versions={}", encode_component(project_id), encode_component(&json!([mc_version]).to_string()));
    let data = fetch_json(client, &url).await?;
    let get = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    Ok(data
        .as_array()
        .map(|a| {
            a.iter()
                .take(10)
                .map(|v| VersionInfo {
                    id: get(v, "id"),
                    version_name: get(v, "version_number"),
                    loaders: get(v, "loaders"),
                    files: v
                        .get("files")
                        .and_then(|f| f.as_array())
                        .map(|f| f.iter().map(|f| VersionFile { url: get(f, "url"), filename: get(f, "filename"), size: get(f, "size"), primary: get(f, "primary") }).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default())
}

// ───────────────────────────── общий кэш и активные паки ─────────────────────────────

pub fn active_packs(settings: &Map<String, Value>, server_key: &str, kind: PackKind) -> Vec<String> {
    settings
        .get("servers")
        .and_then(|s| s.get(server_key))
        .and_then(|s| s.get(kind.settings_key()))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

pub fn set_active_packs(settings: &mut Map<String, Value>, server_key: &str, kind: PackKind, packs: Vec<String>) {
    let servers = settings.entry("servers").or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    let server = servers.as_object_mut().unwrap().entry(server_key.to_string()).or_insert_with(|| json!({}));
    if !server.is_object() {
        *server = json!({});
    }
    server.as_object_mut().unwrap().insert(kind.settings_key().to_string(), json!(packs));
}

/// Добавляет/убирает пак в списке сервера; возвращает итоговый список.
pub fn toggle_pack(settings: &mut Map<String, Value>, server_key: &str, kind: PackKind, filename: &str, enabled: bool) -> Vec<String> {
    let mut current = active_packs(settings, server_key, kind);
    if enabled {
        if !current.iter().any(|f| f == filename) {
            current.push(filename.to_string());
        }
    } else {
        current.retain(|f| f != filename);
    }
    set_active_packs(settings, server_key, kind, current.clone());
    current
}

/// Убирает пак из списков всех серверов.
pub fn remove_everywhere(settings: &mut Map<String, Value>, kind: PackKind, filename: &str) {
    let Some(servers) = settings.get_mut("servers").and_then(|s| s.as_object_mut()) else { return };
    for server in servers.values_mut() {
        if let Some(list) = server.get_mut(kind.settings_key()).and_then(|l| l.as_array_mut()) {
            list.retain(|f| f.as_str() != Some(filename));
        }
    }
}

pub async fn list_installed(shared_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(shared_dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".zip") || n.ends_with(".jar") {
                out.push(n);
            }
        }
    }
    out.sort();
    out
}

/// Удаляет файл пака из `shared/` (отсутствие файла — не ошибка).
pub async fn remove_pack_file(shared_dir: &Path, filename: &str) -> Result<()> {
    let name = safe_file_name(filename)?;
    let _ = tokio::fs::remove_file(shared_dir.join(name)).await;
    Ok(())
}

/// Скачивает пак в `shared/`. Имя файла приходит из внешнего API, поэтому проверяется на путь;
/// URL — только https.
pub async fn download_pack(client: &HttpClient, url: &str, shared_dir: &Path, filename: &str, cancel: &CancelToken, on_pct: impl Fn(u8) + Send + Sync + 'static) -> Result<PathBuf> {
    let name = safe_file_name(filename)?;
    if !url.starts_with("https://") {
        return Err(CoreError::other("Разрешены только https-ссылки"));
    }
    tokio::fs::create_dir_all(shared_dir).await?;
    let dest = shared_dir.join(name);
    let plan = DeltaPlan {
        to_download: vec![PlannedFile { file: RemoteFile { path: name.to_string(), url: url.to_string(), ..Default::default() }, dest: dest.clone(), reason: DownloadReason::Missing, tag: 0 }],
        up_to_date: 0,
    };
    let cb: ProgressFn = Arc::new(move |e| on_pct(e.file_pct));
    let summary = download_all(client, &plan, &DownloadOptions { concurrency: 1, ..Default::default() }, cancel, Some(cb)).await?;
    match summary.failed.first() {
        Some(f) => Err(CoreError::other(f.error.clone())),
        None => Ok(dest),
    }
}

// ───────────────────────────── симлинки ─────────────────────────────

#[cfg(unix)]
fn make_symlink(src: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, link)
}
#[cfg(windows)]
fn make_symlink(src: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(src, link)
}

/// Пересоздаёт символьные ссылки `shared/ → gameDir/shaderpacks|resourcepacks` для активных паков
/// сервера. Удаляются только старые симлинки — обычные файлы игрока не трогаются.
pub fn sync_symlinks(game_dir: &Path, shared_shaders: &Path, shared_resourcepacks: &Path, settings: &Map<String, Value>, server_key: &str) {
    for kind in [PackKind::Shader, PackKind::ResourcePack] {
        let shared = if kind == PackKind::Shader { shared_shaders } else { shared_resourcepacks };
        let target_dir = game_dir.join(kind.folder());
        let _ = std::fs::create_dir_all(&target_dir);

        match std::fs::read_dir(&target_dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    if std::fs::symlink_metadata(e.path()).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                        let _ = std::fs::remove_file(e.path()).or_else(|_| std::fs::remove_dir(e.path()));
                    }
                }
            }
            Err(e) => log::warn!("[symlinks] Очистка {}: {e}", target_dir.display()),
        }

        for pack in active_packs(settings, server_key, kind) {
            let Ok(name) = safe_file_name(&pack) else { continue };
            let (src, link) = (shared.join(name), target_dir.join(name));
            if src.exists() && std::fs::symlink_metadata(&link).is_err() {
                match make_symlink(&src, &link) {
                    Ok(()) => log::info!("[symlinks] Создан симлинк: {name} → {}", link.display()),
                    Err(e) => log::error!("[symlinks] symlink {name}: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn active_packs_toggle_and_remove() {
        let mut s = Map::new();
        assert!(active_packs(&s, "default", PackKind::Shader).is_empty());
        assert_eq!(toggle_pack(&mut s, "default", PackKind::Shader, "a.zip", true), vec!["a.zip"]);
        assert_eq!(toggle_pack(&mut s, "default", PackKind::Shader, "a.zip", true), vec!["a.zip"], "без дублей");
        toggle_pack(&mut s, "default", PackKind::Shader, "b.zip", true);
        toggle_pack(&mut s, "srv2", PackKind::Shader, "a.zip", true);
        toggle_pack(&mut s, "default", PackKind::ResourcePack, "r.zip", true);
        assert_eq!(toggle_pack(&mut s, "default", PackKind::Shader, "a.zip", false), vec!["b.zip"]);
        remove_everywhere(&mut s, PackKind::Shader, "a.zip");
        assert!(active_packs(&s, "srv2", PackKind::Shader).is_empty());
        assert_eq!(active_packs(&s, "default", PackKind::Shader), vec!["b.zip"]);
        assert_eq!(active_packs(&s, "default", PackKind::ResourcePack), vec!["r.zip"], "ресурспаки не затронуты");
        // повреждённая структура в settings не вызывает паники
        let mut broken = map(json!({"servers": 5}));
        set_active_packs(&mut broken, "default", PackKind::Shader, vec!["x".into()]);
        assert_eq!(active_packs(&broken, "default", PackKind::Shader), vec!["x"]);
    }

    #[test]
    fn kind_parsing_matches_original() {
        assert_eq!(PackKind::parse("shader"), PackKind::Shader);
        assert_eq!(PackKind::parse("resourcepack"), PackKind::ResourcePack);
        assert_eq!(PackKind::parse("anything"), PackKind::ResourcePack);
    }

    #[tokio::test]
    async fn installed_list_and_remove_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.zip", "a.jar", "readme.txt"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        assert_eq!(list_installed(dir.path()).await, vec!["a.jar", "b.zip"]);
        assert!(list_installed(&dir.path().join("none")).await.is_empty());
        std::fs::write(dir.path().join("../outside.zip"), b"x").unwrap_or(());
        assert!(remove_pack_file(dir.path(), "../outside.zip").await.is_err(), "traversal отвергнут");
        remove_pack_file(dir.path(), "b.zip").await.unwrap();
        remove_pack_file(dir.path(), "b.zip").await.unwrap(); // уже нет — не ошибка
        assert_eq!(list_installed(dir.path()).await, vec!["a.jar"]);
        let _ = std::fs::remove_file(dir.path().join("../outside.zip"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refreshed_without_touching_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let (game, sp, rp) = (dir.path().join("client"), dir.path().join("shared/sp"), dir.path().join("shared/rp"));
        std::fs::create_dir_all(&sp).unwrap();
        std::fs::create_dir_all(&rp).unwrap();
        std::fs::write(sp.join("bsl.zip"), b"shader").unwrap();
        std::fs::write(sp.join("old.zip"), b"old").unwrap();
        std::fs::write(rp.join("faithful.zip"), b"rp").unwrap();
        // прежнее состояние: ссылка на отключённый пак, файл игрока и ссылка на пак, которого уже нет
        std::fs::create_dir_all(game.join("shaderpacks")).unwrap();
        std::os::unix::fs::symlink(sp.join("old.zip"), game.join("shaderpacks/old.zip")).unwrap();
        std::os::unix::fs::symlink(sp.join("gone.zip"), game.join("shaderpacks/gone.zip")).unwrap();
        std::fs::write(game.join("shaderpacks/mine.zip"), b"player's own").unwrap();

        let mut settings = Map::new();
        toggle_pack(&mut settings, "default", PackKind::Shader, "bsl.zip", true);
        toggle_pack(&mut settings, "default", PackKind::Shader, "missing.zip", true); // нет в shared — пропускается
        toggle_pack(&mut settings, "default", PackKind::Shader, "../evil.zip", true); // небезопасное имя — пропускается
        toggle_pack(&mut settings, "default", PackKind::ResourcePack, "faithful.zip", true);

        sync_symlinks(&game, &sp, &rp, &settings, "default");
        let is_link = |p: PathBuf| std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        assert!(is_link(game.join("shaderpacks/bsl.zip")));
        assert_eq!(std::fs::read(game.join("shaderpacks/bsl.zip")).unwrap(), b"shader");
        assert!(is_link(game.join("resourcepacks/faithful.zip")));
        assert!(!game.join("shaderpacks/old.zip").exists() && std::fs::symlink_metadata(game.join("shaderpacks/old.zip")).is_err(), "ссылка на отключённый пак убрана");
        assert!(std::fs::symlink_metadata(game.join("shaderpacks/gone.zip")).is_err());
        assert_eq!(std::fs::read(game.join("shaderpacks/mine.zip")).unwrap(), b"player's own", "файл игрока цел");
        assert!(!game.join("shaderpacks/missing.zip").exists() && !dir.path().join("client/evil.zip").exists());
        sync_symlinks(&game, &sp, &rp, &settings, "default"); // идемпотентность
        assert!(is_link(game.join("shaderpacks/bsl.zip")));
    }
}
