//! Синхронизация клиента с манифестами репозитория/сервера.
//!
//! Манифест разделён на части:
//!   * `manifest.json` — **список модов** (`groups.mods`), `core`, `loader`, версия Minecraft.
//!     Проверяется при старте лаунчера, при нажатии «Играть» (`verify_mods`) и периодически во время игры.
//!     `core` — строгая проверка хеша + «Smart Wipe» при смене `clientRevision`;
//!     `mods` — дельта-синхронизация + удаление `.jar`, которых нет в манифесте.
//!   * `defaults.json` — **настройки по умолчанию** (`options.txt`, `config/**`). Скачиваются ТОЛЬКО если файла
//!     нет либо игрок нажал «Вернуть настройки по умолчанию» (`sync_defaults`); правки игрока не затираются.
//!     Старый формат (группа `groups.defaults` внутри manifest.json) тоже понимается.
//!   * `blocklist.json` — «чёрный манифест» с ключевыми словами читов (модуль `anticheat`).

use crate::downloader::{
    download_all, fetch_text, plan_delta, CancelToken, DeltaPlan, DownloadOptions, FailedFile, ProgressFn,
    RemoteFile, SyncPolicy,
};
use crate::error::{CoreError, Result};
use crate::fsutil::{encode_url_path, safe_join};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const TAG_CORE: u32 = 0;
pub const TAG_MODS: u32 = 1;
pub const TAG_DEFAULTS: u32 = 2;

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!("version: ожидалась строка/число, получено {other}"))),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Groups {
    #[serde(default)]
    pub core: Vec<RemoteFile>,
    #[serde(default)]
    pub mods: Vec<RemoteFile>,
    #[serde(default)]
    pub defaults: Vec<RemoteFile>,
}

/// Раздел `loader` манифеста: installer загрузчика модов (NeoForge) в репозитории/на сервере.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LoaderEntry {
    /// Имя файла в репозитории (`neoforge-21.1.172-installer.jar`) либо полный URL.
    #[serde(default)]
    pub installer: String,
    #[serde(default)]
    pub sha1: Option<String>,
    /// Метка версии (`21.1.172`): смена метки = загрузчик нужно переустановить.
    #[serde(default)]
    pub version: Option<String>,
    /// `neoforge` / `forge` — информативно.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(deserialize_with = "string_or_number")]
    pub version: String,
    #[serde(default)]
    pub mc_version: Option<String>,
    #[serde(default)]
    pub client_revision: Option<u64>,
    #[serde(default)]
    pub groups: Groups,
    #[serde(default)]
    pub loader: Option<LoaderEntry>,
}

/// `hashes.json`: `{ "mods/x.jar": "<sha1>", ... }`
pub type HashMapFile = HashMap<String, String>;

#[derive(Clone, Debug)]
pub struct SyncConfig {
    pub client_dir: PathBuf,
    /// База без завершающего `/`, например `https://raw.githubusercontent.com/ViPok137/ViLauncher/main`.
    pub raw_base_url: String,
    pub previous_version: Option<String>,
    pub previous_client_revision: Option<u64>,
    /// Игнорировать совпадение версии и перепроверить все файлы (кнопка «Проверить моды»).
    pub force: bool,
    /// Удалять `.jar` в `mods/`, которых нет в манифесте (античит-политика).
    pub remove_unauthorized_mods: bool,
    /// Каталоги, очищаемые при смене `clientRevision`.
    pub core_wipe_dirs: Vec<String>,
}

impl SyncConfig {
    pub fn new(client_dir: impl Into<PathBuf>, raw_base_url: impl Into<String>) -> Self {
        Self {
            client_dir: client_dir.into(),
            raw_base_url: raw_base_url.into().trim_end_matches('/').to_string(),
            previous_version: None,
            previous_client_revision: None,
            force: false,
            remove_unauthorized_mods: true,
            core_wipe_dirs: vec!["bin".to_string()],
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    /// false — версия не изменилась, ничего не делали.
    pub updated: bool,
    pub version: String,
    pub client_revision: Option<u64>,
    pub downloaded: usize,
    pub up_to_date: usize,
    pub bytes: u64,
    pub removed_mods: Vec<String>,
    /// Ошибки НЕкритичной группы `defaults` (не прерывают синхронизацию).
    pub optional_failures: Vec<FailedFile>,
}

/// Скачивает `manifest.json` и (опционально) `hashes.json`.
pub async fn fetch_manifest(client: &reqwest::Client, raw_base_url: &str) -> Result<(Manifest, HashMapFile)> {
    let base = raw_base_url.trim_end_matches('/');
    let manifest_url = format!("{base}/manifest.json");
    let hashes_url = format!("{base}/hashes.json");
    let (m, h) = tokio::join!(fetch_text(client, &manifest_url, 3), fetch_text(client, &hashes_url, 1));
    let manifest: Manifest = serde_json::from_str(m?.trim_start_matches('\u{feff}'))?;
    let hashes: HashMapFile = match h {
        Ok(t) => serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(e) => {
            log::warn!("[sync] hashes.json недоступен ({e}) — используем только хеши из manifest.json");
            HashMapFile::new()
        }
    };
    Ok((manifest, hashes))
}

/// Подставляет URL и хеш из `hashes.json` там, где манифест их не задаёт.
fn resolve(files: &[RemoteFile], base: &str, hashes: &HashMapFile) -> Vec<RemoteFile> {
    files
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.path = f.path.replace('\\', "/");
            if f.url.trim().is_empty() {
                f.url = format!("{base}/{}", encode_url_path(&f.path));
            }
            if f.sha256.is_none() && f.sha1.is_none() && f.md5.is_none() {
                if let Some(h) = hashes.get(&f.path) {
                    f.sha1 = Some(h.clone());
                }
            }
            f
        })
        .collect()
}

async fn remove_unauthorized_jars(mods_dir: &Path, allowed: &HashSet<String>) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    tokio::fs::create_dir_all(mods_dir).await?;
    let mut rd = tokio::fs::read_dir(mods_dir).await?;
    while let Some(entry) = rd.next_entry().await? {
        // Только обычные файлы: символические ссылки и каталоги не трогаем.
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Как в оригинале: удаляются все `.jar`, которых нет в манифесте.
        if name.ends_with(".jar") && !allowed.contains(&name) {
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => {
                    log::info!("[mods] удалён лишний файл: {name}");
                    removed.push(name);
                }
                Err(e) => log::warn!("[mods] не удалось удалить {name}: {e}"),
            }
        }
    }
    removed.sort();
    Ok(removed)
}

/// Основной сценарий синхронизации. Вызывающий код сам сохраняет `report.version` /
/// `report.client_revision` в настройки — и только при `Ok`.
pub async fn run_sync(
    client: &reqwest::Client,
    cfg: &SyncConfig,
    manifest: &Manifest,
    hashes: &HashMapFile,
    opts: &DownloadOptions,
    cancel: &CancelToken,
    progress: Option<ProgressFn>,
) -> Result<SyncReport> {
    let mut report = SyncReport {
        version: manifest.version.clone(),
        client_revision: manifest.client_revision,
        ..Default::default()
    };

    if !cfg.force && cfg.previous_version.as_deref().map(str::trim) == Some(manifest.version.trim()) {
        return Ok(report); // updated = false
    }

    let base = cfg.raw_base_url.trim_end_matches('/');
    let core = resolve(&manifest.groups.core, base, hashes);
    let mods = resolve(&manifest.groups.mods, base, hashes);
    let defaults = resolve(&manifest.groups.defaults, base, hashes);

    // Весь манифест валидируется ДО любых изменений на диске (в т.ч. до Smart Wipe и удаления модов).
    for f in core.iter().chain(&mods).chain(&defaults) {
        safe_join(&cfg.client_dir, &f.path)?;
    }
    let mut wipe_paths = Vec::new();
    for d in &cfg.core_wipe_dirs {
        wipe_paths.push(safe_join(&cfg.client_dir, d)?);
    }

    // Smart Wipe при смене clientRevision.
    if let Some(rev) = manifest.client_revision {
        if rev != cfg.previous_client_revision.unwrap_or(0) {
            log::info!("[core] clientRevision {:?} → {rev}: Smart Wipe", cfg.previous_client_revision);
            for dir in wipe_paths {
                if tokio::fs::try_exists(&dir).await.unwrap_or(false) {
                    tokio::fs::remove_dir_all(&dir).await?;
                }
            }
        }
    }

    // Удаление неавторизованных модов (поведение оригинала сохранено).
    if cfg.remove_unauthorized_mods {
        let allowed: HashSet<String> = mods
            .iter()
            .filter_map(|f| Path::new(&f.path).file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        report.removed_mods = remove_unauthorized_jars(&cfg.client_dir.join("mods"), &allowed).await?;
    }

    // План по трём группам → единая очередь загрузки (параллелизм работает между группами).
    let hc = opts.concurrency.max(4);
    let mut plan = DeltaPlan::default();
    for (files, policy, tag) in [
        (&core, SyncPolicy::Strict, TAG_CORE),
        (&mods, SyncPolicy::Strict, TAG_MODS),
        (&defaults, SyncPolicy::Lazy, TAG_DEFAULTS),
    ] {
        let p = plan_delta(&cfg.client_dir, files, policy, tag, hc).await?;
        plan.up_to_date += p.up_to_date;
        plan.to_download.extend(p.to_download);
    }
    report.up_to_date = plan.up_to_date;

    let summary = download_all(client, &plan, opts, cancel, progress).await?;
    report.downloaded = summary.downloaded;
    report.bytes = summary.bytes;

    let (optional, fatal): (Vec<FailedFile>, Vec<FailedFile>) =
        summary.failed.into_iter().partition(|f| f.tag == TAG_DEFAULTS);
    report.optional_failures = optional;
    if let Some(first) = fatal.first() {
        return Err(CoreError::SyncFailed {
            count: fatal.len(),
            first: format!("{}: {}", first.path, first.error),
        });
    }

    report.updated = true;
    Ok(report)
}

// ───────────────────────────── defaults.json ─────────────────────────────

/// `defaults.json`: настройки по умолчанию (`options.txt`, `config/**`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DefaultsManifest {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub files: Vec<RemoteFile>,
}

/// Список файлов настроек по умолчанию: `defaults.json`, а если его нет — группа `defaults` из
/// старого `manifest.json`.
pub async fn fetch_defaults(client: &reqwest::Client, raw_base_url: &str, legacy: &Manifest) -> Result<Vec<RemoteFile>> {
    let url = format!("{}/defaults.json", raw_base_url.trim_end_matches('/'));
    match fetch_text(client, &url, 1).await {
        Ok(text) => {
            let d: DefaultsManifest = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
            Ok(d.files)
        }
        Err(e) => {
            log::info!("[defaults] defaults.json недоступен ({e}) — используем группу defaults из manifest.json");
            Ok(legacy.groups.defaults.clone())
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsReport {
    pub downloaded: usize,
    pub up_to_date: usize,
    pub failed: Vec<FailedFile>,
}

/// Устанавливает настройки по умолчанию.
///  * `reset = false` — только отсутствующие файлы (правки игрока не трогаются);
///  * `reset = true`  — перезаписать файлы версиями из репозитория («Вернуть настройки по умолчанию»);
///  * `only` — ограничить одним файлом (путь внутри клиента, `config/x.toml`).
#[allow(clippy::too_many_arguments)]
pub async fn sync_defaults(
    client: &reqwest::Client,
    raw_base_url: &str,
    client_dir: &Path,
    files: &[RemoteFile],
    hashes: &HashMapFile,
    reset: bool,
    only: Option<&str>,
    opts: &DownloadOptions,
    cancel: &CancelToken,
    progress: Option<ProgressFn>,
) -> Result<DefaultsReport> {
    let base = raw_base_url.trim_end_matches('/');
    let mut list = resolve(files, base, hashes);
    if let Some(p) = only {
        let want = p.replace('\\', "/");
        list.retain(|f| f.path == want);
        if list.is_empty() {
            return Err(CoreError::other(format!("Файла «{want}» нет в настройках по умолчанию")));
        }
    }
    for f in &list {
        safe_join(client_dir, &f.path)?;
    }
    let policy = if reset { SyncPolicy::Force } else { SyncPolicy::Lazy };
    let plan = plan_delta(client_dir, &list, policy, TAG_DEFAULTS, opts.concurrency.max(4)).await?;
    let summary = download_all(client, &plan, opts, cancel, progress).await?;
    Ok(DefaultsReport { downloaded: summary.downloaded, up_to_date: plan.up_to_date, failed: summary.failed })
}

// ───────────────────────────── проверка списка модов ─────────────────────────────

/// Разрешённые моды: имя файла → SHA-1 (пустая строка, если хеш в манифесте не задан).
pub fn allowed_mods(manifest: &Manifest, hashes: &HashMapFile) -> std::collections::HashMap<String, String> {
    resolve(&manifest.groups.mods, "", hashes)
        .into_iter()
        .filter_map(|f| {
            let name = Path::new(&f.path).file_name()?.to_string_lossy().into_owned();
            Some((name, f.sha1.unwrap_or_default().to_lowercase()))
        })
        .collect()
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub downloaded: usize,
    pub up_to_date: usize,
    pub removed_mods: Vec<String>,
}

/// Проверка при нажатии «Играть»: `core` и `mods` сверяются с манифестом по хешу, повреждённые/изменённые
/// файлы докачиваются, лишние `.jar` удаляются. Версия манифеста здесь не учитывается — проверяется всегда.
#[allow(clippy::too_many_arguments)]
pub async fn verify_mods(
    client: &reqwest::Client,
    raw_base_url: &str,
    client_dir: &Path,
    manifest: &Manifest,
    hashes: &HashMapFile,
    remove_unauthorized: bool,
    opts: &DownloadOptions,
    cancel: &CancelToken,
    progress: Option<ProgressFn>,
) -> Result<VerifyReport> {
    let base = raw_base_url.trim_end_matches('/');
    let core = resolve(&manifest.groups.core, base, hashes);
    let mods = resolve(&manifest.groups.mods, base, hashes);
    for f in core.iter().chain(&mods) {
        safe_join(client_dir, &f.path)?;
    }
    let mut report = VerifyReport::default();
    if remove_unauthorized {
        let allowed: HashSet<String> = mods.iter().filter_map(|f| Path::new(&f.path).file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        report.removed_mods = remove_unauthorized_jars(&client_dir.join("mods"), &allowed).await?;
    }
    let hc = opts.concurrency.max(4);
    let mut plan = DeltaPlan::default();
    for (files, tag) in [(&core, TAG_CORE), (&mods, TAG_MODS)] {
        let p = plan_delta(client_dir, files, SyncPolicy::Strict, tag, hc).await?;
        plan.up_to_date += p.up_to_date;
        plan.to_download.extend(p.to_download);
    }
    report.up_to_date = plan.up_to_date;
    let summary = download_all(client, &plan, opts, cancel, progress).await?;
    report.downloaded = summary.downloaded;
    if let Some(first) = summary.failed.first() {
        return Err(CoreError::SyncFailed { count: summary.failed.len(), first: format!("{}: {}", first.path, first.error) });
    }
    Ok(report)
}

// ───────────────────────────── кэш манифеста (оффлайн-проверка) ─────────────────────────────

#[derive(Serialize, Deserialize)]
struct CacheFile {
    manifest: Manifest,
    hashes: HashMapFile,
    defaults: Vec<RemoteFile>,
}

fn cache_path(client_dir: &Path) -> std::path::PathBuf {
    client_dir.join(".vilauncher-manifest-cache.json")
}

/// Запоминает последний успешно полученный манифест: при нажатии «Играть» без сети моды всё равно
/// проверяются по нему.
pub async fn save_cache(client_dir: &Path, manifest: &Manifest, hashes: &HashMapFile, defaults: &[RemoteFile]) {
    let data = CacheFile { manifest: manifest.clone(), hashes: hashes.clone(), defaults: defaults.to_vec() };
    if let Ok(text) = serde_json::to_string(&data) {
        let _ = tokio::fs::create_dir_all(client_dir).await;
        let _ = tokio::fs::write(cache_path(client_dir), text).await;
    }
}

pub async fn load_cache(client_dir: &Path) -> Option<(Manifest, HashMapFile, Vec<RemoteFile>)> {
    let text = tokio::fs::read_to_string(cache_path(client_dir)).await.ok()?;
    let c: CacheFile = serde_json::from_str(&text).ok()?;
    Some((c.manifest, c.hashes, c.defaults))
}
