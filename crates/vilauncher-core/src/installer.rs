//! Установка Minecraft + загрузчика модов NeoForge/Forge (порт `installer.js`).
//!
//! Версия Minecraft и installer загрузчика задаются снаружи (`InstallTarget`): installer-jar лежит
//! на GitHub (позже — на сервере), его имя/SHA-1/версия описаны в `manifest.json` → `loader`.
//!
//! Порядок: Java 21 → манифест Mojang → client.jar (SHA-1) → библиотеки + natives →
//! assets → installer загрузчика (запуск через изолированную JRE) → чтение version.json загрузчика →
//! `launch.json`. Все загрузки идут через общий движок `downloader` (параллельно, с проверкой хеша).

use crate::downloader::{
    download_all, fetch_text, plan_delta, CancelToken, DeltaPlan, DownloadOptions, DownloadReason, HttpClient,
    PlannedFile, ProgressFn, RemoteFile, SyncPolicy,
};
use crate::error::{CoreError, Result};
use crate::fsutil::{encode_url_path, with_suffix};
use crate::java_runtime::{JavaProgressFn, JavaRuntime};
use crate::launch::zip_tail_ok;
use crate::sync::LoaderEntry;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Источники Mojang (в тестах подменяются на mock-сервер).
#[derive(Clone, Debug)]
pub struct InstallSources {
    pub mojang_manifest: String,
    pub resources_base: String,
}

impl Default for InstallSources {
    fn default() -> Self {
        Self {
            mojang_manifest: "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json".into(),
            resources_base: "https://resources.download.minecraft.net".into(),
        }
    }
}

/// Загрузчик модов: installer-jar берётся с GitHub (позже — с сервера), других зеркал нет.
#[derive(Clone, Debug)]
pub struct LoaderSource {
    /// Метка версии (например `21.1.172`) — по ней лаунчер понимает, что загрузчик обновился.
    pub version: String,
    pub installer_url: String,
    pub installer_sha1: Option<String>,
}

impl LoaderSource {
    /// Имя файла installer'а (последний сегмент URL).
    pub fn installer_file_name(&self) -> String {
        let name = self.installer_url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
        if name.is_empty() { "loader-installer.jar".to_string() } else { name.to_string() }
    }

    /// Собирает источник из раздела `loader` манифеста; `base` — корень репозитория/сервера.
    pub fn from_manifest(entry: &LoaderEntry, base: &str) -> Result<Self> {
        let installer = entry.installer.trim();
        if installer.is_empty() {
            return Err(CoreError::other("В manifest.json нет loader.installer (имя installer-jar в репозитории)"));
        }
        let url = if installer.starts_with("https://") || installer.starts_with("http://") {
            installer.to_string()
        } else {
            format!("{}/{}", base.trim_end_matches('/'), encode_url_path(installer))
        };
        let sha1 = entry.sha1.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let mut loader = Self { version: entry.version.clone().unwrap_or_default().trim().to_string(), installer_url: url, installer_sha1: sha1 };
        if loader.version.is_empty() {
            loader.version = loader.installer_file_name();
        }
        Ok(loader)
    }
}

/// Что устанавливать в instance. `loader: None` — чистая vanilla без модов;
/// `mc_version: "latest"` — актуальный релиз Minecraft (определяется по манифесту Mojang).
#[derive(Clone, Debug)]
pub struct InstallTarget {
    pub mc_version: String,
    pub loader: Option<LoaderSource>,
}

impl InstallTarget {
    /// Ключ установленной связки; если он в `launch.json` не совпадает с ожидаемым — нужна (пере)установка.
    /// Для `latest` до установки ключ содержит слово `latest`, после — реальную версию (см. `LaunchData::install_key`).
    pub fn key(&self) -> String {
        install_key(&self.mc_version, self.loader.as_ref().map(|l| l.version.as_str()).unwrap_or("vanilla"))
    }
}

pub fn install_key(mc_version: &str, loader_version: &str) -> String {
    format!("{mc_version}|{loader_version}")
}

#[derive(Clone, Debug, Serialize)]
pub struct InstallProgress {
    /// java | manifest | client | libraries | assets | forge | done
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
    pub name: String,
}
pub type InstallProgressFn = Arc<dyn Fn(InstallProgress) + Send + Sync>;

fn emit(cb: &Option<InstallProgressFn>, phase: &'static str, done: u64, total: u64, name: impl Into<String>) {
    if let Some(cb) = cb {
        cb(InstallProgress { phase, done, total, name: name.into() });
    }
}

/// Содержимое `launch.json` (формат прежний).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchData {
    pub main_class: String,
    pub classpath: Vec<String>,
    pub module_path: Vec<String>,
    pub forge_jvm_args: Vec<String>,
    pub forge_game_args: Vec<String>,
    pub assets_dir: String,
    pub asset_index: String,
    pub natives_dir: String,
    pub game_dir: String,
    pub forge_version_id: String,
    pub libraries_dir: String,
    pub mc_version: String,
    pub install_key: String,
    /// Major-версия Java, с которой установлен/запускается клиент.
    pub java_major: u32,
}

#[derive(Clone, Debug)]
struct Dirs {
    root: PathBuf,
    libraries: PathBuf,
    natives: PathBuf,
    assets: PathBuf,
    indexes: PathBuf,
    objects: PathBuf,
    versions: PathBuf,
}

impl Dirs {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            libraries: root.join("libraries"),
            natives: root.join("natives"),
            assets: root.join("assets"),
            indexes: root.join("assets").join("indexes"),
            objects: root.join("assets").join("objects"),
            versions: root.join("versions"),
        }
    }
}

// ───────────────────────────── чистые функции (логика classpath) ─────────────────────────────

fn sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// Присоединяет `a/b/c` к базе с родными разделителями ОС (как `path.join` в Node).
fn join_norm(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".") {
        p.push(part);
    }
    p
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

/// Суффикс нативных jar для текущей платформы/архитектуры.
pub fn native_suffix() -> &'static str {
    native_suffix_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn native_suffix_for(os: &str, arch: &str) -> &'static str {
    match os {
        "windows" => match arch {
            "aarch64" => "natives-windows-arm64",
            "x86" => "natives-windows-x86",
            _ => "natives-windows",
        },
        "macos" => if arch == "aarch64" { "natives-macos-arm64" } else { "natives-macos" },
        _ => if arch == "aarch64" { "natives-linux-arm64" } else { "natives-linux" },
    }
}

/// Правила Mojang для библиотек (семантика оригинала).
pub fn is_lib_allowed_for(lib: &Value, os: &str) -> bool {
    let Some(rules) = lib.get("rules").and_then(|r| r.as_array()).filter(|r| !r.is_empty()) else {
        return true;
    };
    let mut result = false;
    for rule in rules {
        let rule_os = rule.get("os").and_then(|o| o.get("name")).and_then(|n| n.as_str());
        if rule.get("os").is_none() || rule_os == Some(os) {
            result = rule.get("action").and_then(|a| a.as_str()) == Some("allow");
        }
    }
    result
}

pub fn is_lib_allowed(lib: &Value) -> bool {
    is_lib_allowed_for(lib, os_name())
}

/// `net.minecraftforge:forge:1.20.1-47.3.0:universal` → `libs/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-universal.jar`
pub fn maven_to_path(name: &str, libs_dir: &Path) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let file = match parts.get(3).filter(|c| !c.is_empty()) {
        Some(c) => format!("{artifact}-{version}-{c}.jar"),
        None => format!("{artifact}-{version}.jar"),
    };
    let mut p = libs_dir.to_path_buf();
    for seg in group.split('.') {
        p.push(seg);
    }
    p.push(artifact);
    p.push(version);
    p.push(file);
    Some(p)
}

fn find_loader_jar(client_dir: &Path) -> Option<PathBuf> {
    let libs = client_dir.join("libraries");
    for base in [libs.join("net").join("neoforged").join("neoforge"), libs.join("net").join("minecraftforge").join("forge")] {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let mut versions: Vec<_> = rd.flatten().collect();
        versions.sort_by_key(|e| e.file_name());
        for ver in versions {
            let dir = ver.path();
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            files.sort();
            let pick = files
                .iter()
                .find(|f| f.contains("universal"))
                .or_else(|| files.iter().find(|f| f.contains("client")))
                .or_else(|| files.iter().find(|f| f.ends_with(".jar") && !f.ends_with("-installer.jar")));
            if let Some(f) = pick {
                return Some(dir.join(f));
            }
        }
    }
    None
}

/// Находит версию загрузчика, созданную installer'ом для данного Minecraft.
/// Приоритет: каталог, оканчивающийся на `loader_version` → `inheritsFrom == mc_version` (самый свежий)
/// → имя содержит `mc_version` (старый стиль Forge `1.20.1-forge-47.4.16`).
/// Версии прежних установок (другой MC) игнорируются.
pub fn find_loader_version_id(versions_dir: &Path, mc_version: &str, loader_version: &str) -> Option<String> {
    let mut cands: Vec<(std::time::SystemTime, String, bool)> = Vec::new();
    for e in std::fs::read_dir(versions_dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().contains("forge") {
            continue;
        }
        let json_path = e.path().join(format!("{name}.json"));
        let inherits = std::fs::read_to_string(&json_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("inheritsFrom").and_then(|x| x.as_str()).map(String::from));
        let matches_mc = inherits.as_deref() == Some(mc_version) || name.contains(mc_version);
        if !matches_mc {
            continue;
        }
        let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        cands.push((mtime, name, inherits.as_deref() == Some(mc_version)));
    }
    if !loader_version.is_empty() {
        if let Some(c) = cands.iter().find(|c| c.1.ends_with(loader_version)) {
            return Some(c.1.clone());
        }
    }
    cands.sort_by(|a, b| (b.2, b.0, &b.1).cmp(&(a.2, a.0, &a.1)));
    cands.into_iter().next().map(|c| c.1)
}

/// Classpath из списка библиотек (порт `buildClasspath`).
#[allow(clippy::too_many_arguments)]
pub fn build_classpath(libs: &[Value], libs_dir: &Path, versions_dir: &Path, client_dir: &Path, mc_version: &str, os: &str, native_sfx: &str) -> Vec<String> {
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut add = |p: PathBuf, entries: &mut Vec<PathBuf>| {
        if seen.insert(p.clone()) {
            entries.push(p);
        }
    };

    for lib in libs {
        if !is_lib_allowed_for(lib, os) {
            continue;
        }
        if let Some(art_path) = lib.pointer("/downloads/artifact/path").and_then(|v| v.as_str()) {
            if art_path.contains("natives-") {
                let base = Path::new(art_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if !base.ends_with(native_sfx) {
                    continue;
                }
            }
            let p = join_norm(libs_dir, art_path);
            if p.exists() {
                add(p, &mut entries);
                continue;
            }
        }
        if let Some(name) = lib.get("name").and_then(|v| v.as_str()) {
            let parts: Vec<&str> = name.split(':').collect();
            if parts.len() >= 4 {
                let classifier = parts[3];
                if classifier.starts_with("natives-") && classifier != native_sfx {
                    continue;
                }
            }
            if let Some(p) = maven_to_path(name, libs_dir) {
                if p.exists() {
                    add(p, &mut entries);
                    continue;
                }
            }
        }
    }

    let client_jar = versions_dir.join(mc_version).join(format!("{mc_version}.jar"));
    if client_jar.exists() {
        add(client_jar, &mut entries);
    }
    if let Some(fj) = find_loader_jar(client_dir) {
        add(fj, &mut entries);
    }
    entries.into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

fn subst(arg: &str, pairs: &[(&str, String)]) -> String {
    let mut out = arg.to_string();
    for (k, v) in pairs {
        out = out.replace(k, v);
    }
    out
}

/// JVM-аргументы из `arguments.jvm` Forge с подстановкой переменных (только строковые элементы).
pub fn extract_forge_jvm_args(forge: &Value, libs_dir: &Path, client_dir: &Path, mc_version: &str) -> Vec<String> {
    let libs = libs_dir.to_string_lossy().into_owned();
    let pairs: Vec<(&str, String)> = vec![
        ("${library_directory}", libs.clone()),
        ("${libraries_directory}", libs),
        ("${classpath_separator}", sep().to_string()),
        ("${version_name}", mc_version.to_string()),
        ("${game_directory}", client_dir.to_string_lossy().into_owned()),
        ("${assets_root}", client_dir.join("assets").to_string_lossy().into_owned()),
        ("${natives_directory}", client_dir.join("natives").to_string_lossy().into_owned()),
    ];
    forge
        .pointer("/arguments/jvm")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| subst(s, &pairs)).collect())
        .unwrap_or_default()
}

/// Игровые аргументы Forge (`--launchTarget forgeclient` и др.).
pub fn extract_forge_game_args(forge: &Value, client_dir: &Path, libs_dir: &Path, assets_dir: &Path, mc_version: &str) -> Vec<String> {
    let pairs: Vec<(&str, String)> = vec![
        ("${game_directory}", client_dir.to_string_lossy().into_owned()),
        ("${assets_root}", assets_dir.to_string_lossy().into_owned()),
        ("${assets_index_name}", "unknown".to_string()),
        ("${version_name}", mc_version.to_string()),
        ("${library_directory}", libs_dir.to_string_lossy().into_owned()),
        ("${classpath_separator}", sep().to_string()),
    ];
    forge
        .pointer("/arguments/game")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| subst(s, &pairs)).collect())
        .unwrap_or_default()
}

/// Сборка `launch.json` из vanilla/loader version.json (порт шага 7 оригинала).
pub fn compute_launch_data(vanilla: &Value, forge: &Value, forge_version_id: &str, client_dir: &Path, target: &InstallTarget) -> LaunchData {
    compute_launch_data_for(vanilla, forge, forge_version_id, client_dir, target, os_name(), native_suffix())
}

pub fn compute_launch_data_for(
    vanilla: &Value,
    forge: &Value,
    forge_version_id: &str,
    client_dir: &Path,
    target: &InstallTarget,
    os: &str,
    native_sfx: &str,
) -> LaunchData {
    let mc_version = target.mc_version.as_str();
    let d = Dirs::new(client_dir);
    let client_jar = d.versions.join(mc_version).join(format!("{mc_version}.jar"));
    let sep = sep();

    let mut all_libs: Vec<Value> = vanilla.get("libraries").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    all_libs.extend(forge.get("libraries").and_then(|v| v.as_array()).cloned().unwrap_or_default());

    let raw = extract_forge_jvm_args(forge, &d.libraries, client_dir, mc_version);
    let mut module_path: Vec<String> = Vec::new();
    let mut legacy_cp: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let arg = &raw[i];
        if (arg == "-p" || arg == "--module-path") && i + 1 < raw.len() {
            module_path = raw[i + 1].split(sep).filter(|s| !s.is_empty()).map(String::from).collect();
            i += 2;
            continue;
        }
        if let Some(v) = arg.strip_prefix("-DlegacyClassPath=") {
            legacy_cp = v.split(sep).filter(|s| !s.is_empty()).map(String::from).collect();
        }
        other.push(arg.clone());
        i += 1;
    }

    // Fallback только если Forge совсем не прописал -p.
    if module_path.is_empty() {
        module_path = [
            "cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar",
            "cpw/mods/securejarhandler/2.1.10/securejarhandler-2.1.10.jar",
        ]
        .iter()
        .map(|p| join_norm(&d.libraries, p))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    }

    let client_jar_s = client_jar.to_string_lossy().into_owned();
    let classpath: Vec<String> = if !legacy_cp.is_empty() {
        let mut cp: Vec<String> = legacy_cp
            .into_iter()
            .filter(|p| {
                let base = Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                !base.contains("natives-") || base.ends_with(native_sfx)
            })
            .collect();
        if !cp.iter().any(|p| p.contains(&format!("{mc_version}.jar"))) && client_jar.exists() {
            cp.push(client_jar_s);
        }
        cp
    } else {
        let mut cp = build_classpath(&all_libs, &d.libraries, &d.versions, client_dir, mc_version, os, native_sfx);
        if !cp.contains(&client_jar_s) && client_jar.exists() {
            cp.push(client_jar_s);
        }
        cp
    };

    let main_class = forge
        .get("mainClass")
        .and_then(|v| v.as_str())
        .or_else(|| vanilla.get("mainClass").and_then(|v| v.as_str()))
        .unwrap_or_default()
        .to_string();

    LaunchData {
        main_class,
        classpath,
        module_path,
        forge_jvm_args: other,
        forge_game_args: extract_forge_game_args(forge, client_dir, &d.libraries, &d.assets, mc_version),
        assets_dir: d.assets.to_string_lossy().into_owned(),
        asset_index: vanilla.pointer("/assetIndex/id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        natives_dir: d.natives.to_string_lossy().into_owned(),
        game_dir: client_dir.to_string_lossy().into_owned(),
        forge_version_id: forge_version_id.to_string(),
        libraries_dir: d.libraries.to_string_lossy().into_owned(),
        mc_version: mc_version.to_string(),
        install_key: target.key(),
        java_major: 0,
    }
}

/// `launch.json` для чистой vanilla: classpath из библиотек Mojang, без модулей и аргументов загрузчика.
pub fn compute_vanilla_launch_data(vanilla: &Value, client_dir: &Path, target: &InstallTarget) -> LaunchData {
    compute_vanilla_launch_data_for(vanilla, client_dir, target, os_name(), native_suffix())
}

pub fn compute_vanilla_launch_data_for(vanilla: &Value, client_dir: &Path, target: &InstallTarget, os: &str, native_sfx: &str) -> LaunchData {
    let d = Dirs::new(client_dir);
    let mc = target.mc_version.as_str();
    let libs: Vec<Value> = vanilla.get("libraries").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    LaunchData {
        main_class: vanilla.get("mainClass").and_then(|v| v.as_str()).unwrap_or("net.minecraft.client.main.Main").to_string(),
        classpath: build_classpath(&libs, &d.libraries, &d.versions, client_dir, mc, os, native_sfx),
        module_path: Vec::new(),
        forge_jvm_args: Vec::new(),
        forge_game_args: Vec::new(),
        assets_dir: d.assets.to_string_lossy().into_owned(),
        asset_index: vanilla.pointer("/assetIndex/id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        natives_dir: d.natives.to_string_lossy().into_owned(),
        game_dir: client_dir.to_string_lossy().into_owned(),
        forge_version_id: mc.to_string(),
        libraries_dir: d.libraries.to_string_lossy().into_owned(),
        mc_version: mc.to_string(),
        install_key: target.key(),
        java_major: 0,
    }
}

// ───────────────────────────── natives ─────────────────────────────

/// Распаковывает нативные библиотеки (`.dll/.so/.dylib/.jnilib`) в плоскую папку (порт `extractNativesNode`).
pub fn extract_natives(zip_path: &Path, dest_dir: &Path, exclude: &[String]) -> Result<usize> {
    std::fs::create_dir_all(dest_dir)?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut count = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if exclude.iter().any(|ex| name.starts_with(ex.as_str())) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".dll") || lower.ends_with(".so") || lower.ends_with(".dylib") || lower.ends_with(".jnilib")) {
            continue;
        }
        // Только имя файла — вложенные пути архива (в т.ч. `../`) на диск не попадают.
        let Some(base) = Path::new(&name).file_name() else { continue };
        let mut out = std::fs::File::create(dest_dir.join(base))?;
        let mut buf = Vec::new();
        (&mut entry).take(200 * 1024 * 1024).read_to_end(&mut buf)?;
        std::io::Write::write_all(&mut out, &buf)?;
        count += 1;
    }
    Ok(count)
}

// ───────────────────────────── загрузки ─────────────────────────────

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).filter(|s| !s.is_empty())
}

fn remote_from_artifact(art: &Value, libs_rel: &str) -> Option<RemoteFile> {
    let url = str_of(art, "url")?;
    let path = str_of(art, "path")?;
    Some(RemoteFile {
        path: format!("{libs_rel}/{path}"),
        url: url.to_string(),
        sha1: str_of(art, "sha1").map(String::from),
        size: art.get("size").and_then(|s| s.as_u64()),
        ..Default::default()
    })
}

/// План + параллельная загрузка группы файлов с событиями прогресса `phase`.
#[allow(clippy::too_many_arguments)]
async fn sync_group(
    client: &HttpClient,
    base: &Path,
    files: Vec<RemoteFile>,
    policy: SyncPolicy,
    opts: &DownloadOptions,
    cancel: &CancelToken,
    phase: &'static str,
    progress: &Option<InstallProgressFn>,
) -> Result<()> {
    let total = files.len() as u64;
    let plan = plan_delta(base, &files, policy, 0, 16).await?;
    let up = plan.up_to_date as u64;
    emit(progress, phase, up, total, "");
    let pr = progress.clone();
    let cb: ProgressFn = Arc::new(move |e| emit(&pr, phase, up + e.completed as u64, total, e.name));
    let summary = download_all(client, &plan, opts, cancel, Some(cb)).await?;
    if let Some(f) = summary.failed.first() {
        return Err(CoreError::other(format!("Не удалось скачать {}: {}", f.path, f.error)));
    }
    emit(progress, phase, total, total, "");
    Ok(())
}

async fn download_single(client: &HttpClient, url: &str, dest: &Path, sha1: Option<&str>, retries: u32, cancel: &CancelToken) -> Result<()> {
    let plan = DeltaPlan {
        to_download: vec![PlannedFile {
            file: RemoteFile { path: dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), url: url.to_string(), sha1: sha1.map(String::from), ..Default::default() },
            dest: dest.to_path_buf(),
            reason: DownloadReason::Missing,
            tag: 0,
        }],
        up_to_date: 0,
    };
    let opts = DownloadOptions { concurrency: 1, retries, ..Default::default() };
    let summary = download_all(client, &plan, &opts, cancel, None).await?;
    match summary.failed.first() {
        Some(f) => Err(CoreError::other(f.error.clone())),
        None => Ok(()),
    }
}

async fn run_forge_installer(
    java: &JavaRuntime,
    installer: &Path,
    client_dir: &Path,
    progress: &Option<InstallProgressFn>,
) -> (String, String, i32) {
    let mut cmd = tokio::process::Command::from(java.command());
    cmd.args([
        "-Dmaven.wagon.http.retryHandler.class=standard",
        "-Dmaven.wagon.http.retryHandler.count=5",
        "-Dhttp.maxRedirects=10",
        "-jar",
    ])
    .arg(installer)
    .arg("--installClient")
    .arg(client_dir)
    .current_dir(client_dir)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (String::new(), e.to_string(), -1),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let pr = progress.clone();
    let out_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(s) = stdout {
            let mut lines = BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let l = line.trim();
                if l.contains("Downloading") || l.contains("Considering") {
                    emit(&pr, "forge", 1, 3, format!("⬇ {}", l.chars().take(80).collect::<String>()));
                }
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });
    let err_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(s) = stderr {
            let mut lines = BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });

    // Таймаут 10 минут, как в оригинале.
    let (code, timed_out) = match tokio::time::timeout(Duration::from_secs(600), child.wait()).await {
        Ok(Ok(st)) => (st.code().unwrap_or(-1), false),
        Ok(Err(_)) => (-1, false),
        Err(_) => {
            let _ = child.kill().await;
            (-1, true)
        }
    };
    let out = out_task.await.unwrap_or_default();
    let mut err = err_task.await.unwrap_or_default();
    if timed_out {
        err = "TIMEOUT: Forge installer превысил 10 минут".to_string();
    }
    (out, err, code)
}

/// Актуальный релиз Minecraft по манифесту Mojang (для vanilla-сервера с `mcVersion: "latest"`).
pub async fn latest_release(client: &HttpClient, mojang_manifest: &str) -> Result<String> {
    let manifest: Value = serde_json::from_str(&fetch_text(client, mojang_manifest, 1).await?)?;
    manifest.pointer("/latest/release").and_then(|v| v.as_str()).map(String::from).ok_or_else(|| CoreError::other("В манифесте Mojang нет latest.release"))
}

// ───────────────────────────── главная функция ─────────────────────────────

pub async fn install(
    client: &HttpClient,
    launcher_dir: &Path,
    client_dir: &Path,
    sources: &InstallSources,
    target: &InstallTarget,
    cancel: &CancelToken,
    progress: Option<InstallProgressFn>,
) -> Result<LaunchData> {
    let d = Dirs::new(client_dir);
    for dir in [&d.root, &d.libraries, &d.natives, &d.assets, &d.indexes, &d.objects, &d.versions] {
        tokio::fs::create_dir_all(dir).await?;
    }
    let fast = DownloadOptions { concurrency: 8, retries: 3, ..Default::default() };
    let many = DownloadOptions { concurrency: 16, retries: 3, ..Default::default() };

    // 1. Манифест Mojang (версия `latest` → актуальный релиз)
    emit(&progress, "manifest", 0, 1, "Получаю манифест Mojang...");
    let manifest: Value = serde_json::from_str(&fetch_text(client, &sources.mojang_manifest, 3).await?)?;
    let mc_owned: String = if target.mc_version == "latest" {
        manifest
            .pointer("/latest/release")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| CoreError::other("В манифесте Mojang нет latest.release"))?
    } else {
        target.mc_version.clone()
    };
    let mc = mc_owned.as_str();
    let resolved = InstallTarget { mc_version: mc_owned.clone(), loader: target.loader.clone() };
    let entry = manifest
        .get("versions")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|v| v.get("id").and_then(|i| i.as_str()) == Some(mc)))
        .ok_or_else(|| CoreError::other(format!("Версия {mc} не найдена")))?;
    let version_url = str_of(entry, "url").ok_or_else(|| CoreError::other("В манифесте нет url версии"))?;
    let vanilla: Value = serde_json::from_str(&fetch_text(client, version_url, 3).await?)?;
    let vanilla_dir = d.versions.join(mc);
    tokio::fs::create_dir_all(&vanilla_dir).await?;
    tokio::fs::write(vanilla_dir.join(format!("{mc}.json")), serde_json::to_string_pretty(&vanilla)?).await?;
    emit(&progress, "manifest", 1, 1, "Манифест получен");

    // 0. Изолированная Java нужной версии (по данным Mojang; 1.20.1 → 17, 1.21.x → 21). Уже установленная — переиспользуется.
    let java_major = vanilla.pointer("/javaVersion/majorVersion").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or_else(|| crate::java_runtime::java_major_for_mc(mc));
    let java = JavaRuntime::with_major(launcher_dir, java_major);
    {
        let pr = progress.clone();
        let jp: JavaProgressFn = Arc::new(move |p| emit(&pr, "java", p.pct as u64, 100, p.message));
        java.ensure(client, cancel, Some(jp)).await?;
    }

    // 2. client.jar — SHA-1 проверяется всегда, при несовпадении скачивается заново
    let client_info = vanilla.pointer("/downloads/client").ok_or_else(|| CoreError::other("Нет downloads.client"))?;
    emit(&progress, "client", 0, 1, "Проверяю client.jar (SHA1)...");
    let cj = RemoteFile {
        path: format!("versions/{mc}/{mc}.jar"),
        url: str_of(client_info, "url").unwrap_or_default().to_string(),
        sha1: str_of(client_info, "sha1").map(String::from),
        size: client_info.get("size").and_then(|s| s.as_u64()),
        ..Default::default()
    };
    sync_group(client, client_dir, vec![cj], SyncPolicy::Strict, &fast, cancel, "client", &progress)
        .await
        .map_err(|e| CoreError::other(format!("client.jar скачался повреждённым или недоступен. Проверь интернет. ({e})")))?;
    emit(&progress, "client", 1, 1, "client.jar готов ✓");

    // 3. Библиотеки + natives
    let libs: Vec<&Value> = vanilla
        .get("libraries")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter(|l| is_lib_allowed(l)).collect())
        .unwrap_or_default();
    let plat_key = os_name();
    let mut lib_files: Vec<RemoteFile> = Vec::new();
    let mut natives_to_extract: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for lib in &libs {
        if let Some(art) = lib.pointer("/downloads/artifact") {
            if let Some(rf) = remote_from_artifact(art, "libraries") {
                lib_files.push(rf);
            }
        }
        let arch_bits = if std::env::consts::ARCH == "x86_64" || std::env::consts::ARCH == "aarch64" { "64" } else { "32" };
        let native_key = lib
            .pointer(&format!("/natives/{plat_key}"))
            .and_then(|v| v.as_str())
            .map(|k| k.replace("${arch}", arch_bits));
        if let Some(key) = native_key {
            if let Some(nat) = lib.pointer(&format!("/downloads/classifiers/{key}")) {
                if let Some(rf) = remote_from_artifact(nat, "libraries") {
                    let exclude: Vec<String> = lib
                        .pointer("/extract/exclude")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    natives_to_extract.push((client_dir.join(rf.path.replace('/', std::path::MAIN_SEPARATOR_STR)), exclude));
                    lib_files.push(rf);
                }
            }
        }
    }
    emit(&progress, "libraries", 0, lib_files.len() as u64, "Загружаю библиотеки...");
    sync_group(client, client_dir, lib_files, SyncPolicy::Strict, &fast, cancel, "libraries", &progress).await?;
    for (jar, exclude) in natives_to_extract {
        let natives_dir = d.natives.clone();
        let jar2 = jar.clone();
        match tokio::task::spawn_blocking(move || extract_natives(&jar2, &natives_dir, &exclude)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => log::warn!("[install] natives {}: {e}", jar.display()),
            Err(e) => log::warn!("[install] natives task: {e}"),
        }
    }

    // 4. Assets
    let idx = vanilla.get("assetIndex").ok_or_else(|| CoreError::other("Нет assetIndex"))?;
    let idx_id = str_of(idx, "id").unwrap_or_default();
    let idx_rel = format!("assets/indexes/{idx_id}.json");
    emit(&progress, "assets", 0, 1, "Скачиваю индекс ресурсов...");
    let idx_file = RemoteFile { path: idx_rel, url: str_of(idx, "url").unwrap_or_default().to_string(), sha1: str_of(idx, "sha1").map(String::from), ..Default::default() };
    // Индекс скачивается только если его ещё нет (как в оригинале).
    sync_group(client, client_dir, vec![idx_file], SyncPolicy::Lazy, &fast, cancel, "assets", &progress).await?;
    let index_json: Value = serde_json::from_str(&tokio::fs::read_to_string(d.indexes.join(format!("{idx_id}.json"))).await?)?;
    let mut asset_files: Vec<RemoteFile> = Vec::new();
    if let Some(objs) = index_json.get("objects").and_then(|o| o.as_object()) {
        for obj in objs.values() {
            let Some(hash) = str_of(obj, "hash") else { continue };
            if hash.len() < 2 {
                continue;
            }
            let prefix = &hash[..2];
            asset_files.push(RemoteFile {
                path: format!("assets/objects/{prefix}/{hash}"),
                url: format!("{}/{prefix}/{hash}", sources.resources_base.trim_end_matches('/')),
                sha1: Some(hash.to_string()),
                size: obj.get("size").and_then(|s| s.as_u64()),
                ..Default::default()
            });
        }
    }
    emit(&progress, "assets", 0, asset_files.len() as u64, "Загружаю ресурсы...");
    sync_group(client, client_dir, asset_files, SyncPolicy::Strict, &many, cancel, "assets", &progress).await?;

    // 5. Installer загрузчика (NeoForge). Единственный источник — GitHub/сервер из manifest.json.
    //    Для vanilla (loader = None) шаги 5–6 пропускаются.
    let mut launch: LaunchData = if let Some(loader) = resolved.loader.as_ref() {
    let jar_name = loader.installer_file_name();
    let installer_path = client_dir.join(&jar_name);
    let needs_download = match &loader.installer_sha1 {
        // С SHA-1 любое расхождение (в т.ч. обновлённый на GitHub jar) вызывает перекачку.
        Some(want) => match crate::hash::hash_file(installer_path.clone(), crate::hash::HashAlgo::Sha1).await {
            Ok(h) => !crate::hash::hash_eq(&h, want),
            Err(_) => true,
        },
        None => !zip_tail_ok(&installer_path),
    };
    if needs_download {
        let _ = tokio::fs::remove_file(&installer_path).await;
        const MAX_ATTEMPTS: u32 = 3;
        let mut last_err = String::new();
        let mut downloaded = false;
        for attempt in 1..=MAX_ATTEMPTS {
            emit(
                &progress,
                "forge",
                0,
                3,
                if attempt == 1 { "Скачиваю installer загрузчика...".to_string() } else { format!("Повторная загрузка installer (попытка {attempt}/{MAX_ATTEMPTS})...") },
            );
            match download_single(client, &loader.installer_url, &installer_path, loader.installer_sha1.as_deref(), 1, cancel).await {
                Ok(()) if zip_tail_ok(&installer_path) => {
                    downloaded = true;
                    break;
                }
                Ok(()) => {
                    last_err = "файл повреждён (это не ZIP/JAR)".to_string();
                    let _ = tokio::fs::remove_file(&installer_path).await;
                }
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(e) => {
                    log::error!("Loader installer: {} {e}", loader.installer_url);
                    last_err = e.to_string();
                    let _ = tokio::fs::remove_file(&installer_path).await;
                }
            }
            if attempt < MAX_ATTEMPTS {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        if !downloaded {
            return Err(CoreError::other(format!(
                "Не удалось скачать installer загрузчика ({jar_name}) после {MAX_ATTEMPTS} попыток.\n{last_err}\nПроверь, что файл загружен в репозиторий и указан в manifest.json (loader.installer).\nПопробуй нажать ПОВТОРИТЬ через несколько минут."
            )));
        }
    }

    // Старые installer'ы прежних версий загрузчика больше не нужны.
    if let Ok(mut rd) = tokio::fs::read_dir(client_dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with("-installer.jar") && n != jar_name {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }

    emit(&progress, "forge", 1, 3, "Проверяю изолированную Java для installer...");
    // NeoForge 21.1 требует Java 21 — изолированная сборка уже проверена на шаге 0.

    // Forge installer требует launcher_profiles.json — создаём заглушку.
    let profiles = client_dir.join("launcher_profiles.json");
    if !tokio::fs::try_exists(&profiles).await.unwrap_or(false) {
        let stub = serde_json::json!({
            "profiles": { "forge": { "name": "forge", "type": "custom", "lastVersionId": mc, "gameDir": client_dir } },
            "selectedProfile": "forge",
            "clientToken": "mc-launcher",
            "authenticationDatabase": {},
            "launcherVersion": { "name": "2.12.2", "format": 21, "profilesFormat": 2 },
        });
        tokio::fs::write(&profiles, serde_json::to_string_pretty(&stub)?).await?;
    }
    // Структура versions/<mc>/<mc>.{jar,json}, которую ждёт installer, уже создана шагами 1-2.

    emit(&progress, "forge", 1, 3, &format!("Запускаю installer загрузчика (Java {java_major}, 2-3 мин)..."));
    let forge_log = client_dir.join("forge-install.log");
    let (f_out, f_err, f_code) = run_forge_installer(&java, &installer_path, client_dir, &progress).await;
    tokio::fs::write(&forge_log, format!("=== STDOUT ===\n{f_out}\n=== STDERR ===\n{f_err}")).await?;
    emit(&progress, "forge", 2, 3, "Installer завершён, строю classpath...");

    // 6. version.json загрузчика
    let versions_dir = d.versions.clone();
    let (mc_s, lv_s) = (mc.to_string(), loader.version.clone());
    let forge_id = tokio::task::spawn_blocking(move || find_loader_version_id(&versions_dir, &mc_s, &lv_s))
        .await
        .map_err(|e| CoreError::other(format!("task: {e}")))?;
    let Some(forge_id) = forge_id else {
        let err_lines: Vec<&str> = f_err
            .lines()
            .filter(|l| l.contains("ERROR") || l.contains("Exception") || l.contains("FAILED") || l.contains("error"))
            .collect();
        let summary = if err_lines.is_empty() {
            f_err.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect::<String>()
        } else {
            err_lines[err_lines.len().saturating_sub(5)..].join("\n")
        };
        return Err(CoreError::other(format!(
            "Installer загрузчика завершился с кодом {f_code}.\n{}{}\n\nПолный лог: {}",
            if summary.is_empty() { "" } else { "\n" },
            summary,
            forge_log.display()
        )));
    };
    let forge_json: Value = serde_json::from_str(&tokio::fs::read_to_string(d.versions.join(&forge_id).join(format!("{forge_id}.json"))).await?)?;

    // 7. launch.json
    let (v2, f2, id2, cd2, t2) = (vanilla.clone(), forge_json, forge_id, client_dir.to_path_buf(), resolved.clone());
    let l = tokio::task::spawn_blocking(move || compute_launch_data(&v2, &f2, &id2, &cd2, &t2))
        .await
        .map_err(|e| CoreError::other(format!("task: {e}")))?;
    emit(&progress, "forge", 3, 3, "Загрузчик установлен");
    l
    } else {
        // Чистая vanilla: classpath из библиотек Mojang.
        let (v2, cd2, t2) = (vanilla.clone(), client_dir.to_path_buf(), resolved.clone());
        tokio::task::spawn_blocking(move || compute_vanilla_launch_data(&v2, &cd2, &t2))
            .await
            .map_err(|e| CoreError::other(format!("task: {e}")))?
    };
    launch.java_major = java_major;
    let tmp = with_suffix(&client_dir.join("launch.json"), ".tmp");
    tokio::fs::write(&tmp, serde_json::to_string_pretty(&launch)?).await?;
    tokio::fs::rename(&tmp, client_dir.join("launch.json")).await?;

    emit(&progress, "done", 1, 1, "Установка завершена!");
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(mc: &str, loader_version: &str) -> InstallTarget {
        InstallTarget {
            mc_version: mc.to_string(),
            loader: Some(LoaderSource { version: loader_version.to_string(), installer_url: "https://x/y/neoforge-installer.jar".into(), installer_sha1: None }),
        }
    }

    #[test]
    fn lib_rules_follow_original_semantics() {
        let no_rules = json!({"name":"a:b:1"});
        assert!(is_lib_allowed_for(&no_rules, "windows"));
        let only_osx = json!({"rules":[{"action":"allow","os":{"name":"osx"}}]});
        assert!(!is_lib_allowed_for(&only_osx, "windows"));
        assert!(is_lib_allowed_for(&only_osx, "osx"));
        let all_but_osx = json!({"rules":[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]});
        assert!(is_lib_allowed_for(&all_but_osx, "windows"));
        assert!(!is_lib_allowed_for(&all_but_osx, "osx"));
    }

    #[test]
    fn maven_coordinates() {
        let p = maven_to_path("net.minecraftforge:forge:1.20.1-47.3.0:universal", Path::new("/l")).unwrap();
        assert_eq!(p, Path::new("/l/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-universal.jar"));
        let p = maven_to_path("org.ow2.asm:asm:9.5", Path::new("/l")).unwrap();
        assert_eq!(p, Path::new("/l/org/ow2/asm/asm/9.5/asm-9.5.jar"));
        assert!(maven_to_path("bad", Path::new("/l")).is_none());
    }

    #[test]
    fn native_suffixes() {
        assert_eq!(native_suffix_for("windows", "x86_64"), "natives-windows");
        assert_eq!(native_suffix_for("windows", "aarch64"), "natives-windows-arm64");
        assert_eq!(native_suffix_for("macos", "aarch64"), "natives-macos-arm64");
        assert_eq!(native_suffix_for("linux", "x86_64"), "natives-linux");
    }

    #[test]
    fn forge_args_substitution_and_split() {
        let dir = tempfile::tempdir().unwrap();
        let cd = dir.path();
        let s = sep();
        let forge = json!({
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "arguments": {
                "jvm": [
                    "-DlibraryDirectory=${library_directory}",
                    "-p", format!("${{library_directory}}/a/one.jar${{classpath_separator}}${{library_directory}}/b/two.jar"),
                    "--add-modules", "ALL-MODULE-PATH",
                    {"rules": [], "value": "-Dskipped=1"},
                    "-DlegacyClassPath=x1.jar".to_string() + s + "y-natives-windows-arm64.jar" + s + "z-natives-linux.jar"
                ],
                "game": ["--launchTarget", "forgeclient", "--assetsDir", "${assets_root}", "--assetIndex", "${assets_index_name}"]
            }
        });
        let vanilla = json!({"mainClass": "net.minecraft.Main", "assetIndex": {"id": "5"}, "libraries": []});
        let ld = compute_launch_data_for(&vanilla, &forge, "neoforge-21.1.172", cd, &target("1.21.1", "21.1.172"), "linux", "natives-linux");
        assert_eq!(ld.main_class, "cpw.mods.bootstraplauncher.BootstrapLauncher");
        assert_eq!(ld.module_path.len(), 2);
        assert!(ld.module_path[0].ends_with("one.jar") && !ld.module_path[0].contains("${"));
        // -p и его значение не остаются в jvm-аргументах; -Dskipped (объект) отброшен
        assert!(!ld.forge_jvm_args.iter().any(|a| a == "-p" || a.contains("one.jar") || a.contains("skipped")));
        assert!(ld.forge_jvm_args.iter().any(|a| a.starts_with("-DlibraryDirectory=") && !a.contains("${")));
        assert!(ld.forge_jvm_args.contains(&"ALL-MODULE-PATH".to_string()));
        // legacyClassPath: natives чужой архитектуры отфильтрованы
        assert_eq!(ld.classpath, vec!["x1.jar".to_string(), "z-natives-linux.jar".to_string()]);
        assert_eq!(ld.forge_game_args[3], cd.join("assets").to_string_lossy());
        assert_eq!(ld.forge_game_args[5], "unknown");
        assert_eq!(ld.asset_index, "5");
        assert_eq!(ld.forge_version_id, "neoforge-21.1.172");
        assert_eq!(ld.mc_version, "1.21.1");
        assert_eq!(ld.install_key, "1.21.1|21.1.172");
    }

    #[test]
    fn classpath_built_from_libraries_when_no_legacy_cp() {
        let dir = tempfile::tempdir().unwrap();
        let cd = dir.path();
        let libs = cd.join("libraries");
        for rel in ["com/a/a/1/a-1.jar", "org/lwjgl/lwjgl/3/lwjgl-3-natives-linux.jar", "org/lwjgl/lwjgl/3/lwjgl-3-natives-linux-arm64.jar", "net/neoforged/neoforge/21.1.172/neoforge-21.1.172-universal.jar", "org/m/m/2/m-2.jar"] {
            let p = join_norm(&libs, rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let cj = cd.join("versions/1.21.1/1.21.1.jar");
        std::fs::create_dir_all(cj.parent().unwrap()).unwrap();
        std::fs::write(&cj, b"c").unwrap();

        let vanilla = json!({"assetIndex":{"id":"5"},"libraries":[
            {"name":"com.a:a:1","downloads":{"artifact":{"path":"com/a/a/1/a-1.jar"}}},
            {"name":"org.lwjgl:lwjgl:3","downloads":{"artifact":{"path":"org/lwjgl/lwjgl/3/lwjgl-3-natives-linux.jar"}}},
            {"name":"org.lwjgl:lwjgl:3:natives-linux-arm64","downloads":{"artifact":{"path":"org/lwjgl/lwjgl/3/lwjgl-3-natives-linux-arm64.jar"}}},
            {"name":"win:only","rules":[{"action":"allow","os":{"name":"windows"}}],"downloads":{"artifact":{"path":"com/a/a/1/a-1.jar"}}}
        ]});
        let forge = json!({"libraries":[{"name":"org.m:m:2"}, {"name":"missing:lib:1"}], "arguments":{"jvm":[],"game":[]}});
        let ld = compute_launch_data_for(&vanilla, &forge, "f", cd, &target("1.21.1", "21.1.172"), "linux", "natives-linux");
        let names: Vec<String> = ld.classpath.iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a-1.jar", "lwjgl-3-natives-linux.jar", "m-2.jar", "1.21.1.jar", "neoforge-21.1.172-universal.jar"]);
    }

    fn write_version(versions: &Path, id: &str, inherits: Option<&str>, age_secs: u64) {
        let dir = versions.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let body = match inherits {
            Some(i) => json!({"id": id, "inheritsFrom": i}),
            None => json!({"id": id}),
        };
        let f = dir.join(format!("{id}.json"));
        std::fs::write(&f, body.to_string()).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        let d = std::fs::File::open(&dir).unwrap();
        let _ = d.set_modified(t);
    }

    #[test]
    fn loader_version_selection_ignores_old_installs() {
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path();
        std::fs::create_dir_all(v.join("1.21.1")).unwrap(); // ванильная — не загрузчик
        write_version(v, "1.20.1-forge-47.4.16", Some("1.20.1"), 1000); // прежняя установка под 1.20.1
        write_version(v, "neoforge-21.1.100", Some("1.21.1"), 500);
        write_version(v, "neoforge-21.1.172", Some("1.21.1"), 10);
        // по метке версии
        assert_eq!(find_loader_version_id(v, "1.21.1", "21.1.100").as_deref(), Some("neoforge-21.1.100"));
        // без метки — самая свежая для нужного MC, старый Forge 1.20.1 не мешает
        assert_eq!(find_loader_version_id(v, "1.21.1", "").as_deref(), Some("neoforge-21.1.172"));
        assert_eq!(find_loader_version_id(v, "1.21.1", "unknown").as_deref(), Some("neoforge-21.1.172"));
        // старый стиль Forge определяется и по имени
        assert_eq!(find_loader_version_id(v, "1.20.1", "").as_deref(), Some("1.20.1-forge-47.4.16"));
        assert_eq!(find_loader_version_id(v, "1.19.4", ""), None);
    }

    #[test]
    fn loader_source_from_manifest() {
        let base = "https://raw.githubusercontent.com/ViPok137/ViLauncher/main/";
        let e = LoaderEntry { installer: "neoforge-21.1.172-installer.jar".into(), sha1: Some(" ABC ".into()), version: Some("21.1.172".into()), kind: Some("neoforge".into()) };
        let l = LoaderSource::from_manifest(&e, base).unwrap();
        assert_eq!(l.installer_url, "https://raw.githubusercontent.com/ViPok137/ViLauncher/main/neoforge-21.1.172-installer.jar");
        assert_eq!(l.installer_sha1.as_deref(), Some("ABC"));
        assert_eq!(l.version, "21.1.172");
        assert_eq!(l.installer_file_name(), "neoforge-21.1.172-installer.jar");

        // без метки версии — по имени файла; полный URL (сервер) принимается как есть; пробелы кодируются
        let e2 = LoaderEntry { installer: "https://srv.example/loader/nf installer.jar".into(), ..Default::default() };
        let l2 = LoaderSource::from_manifest(&e2, base).unwrap();
        assert_eq!(l2.installer_url, "https://srv.example/loader/nf installer.jar");
        assert_eq!(l2.version, "nf installer.jar");
        let e3 = LoaderEntry { installer: "my loader+1.jar".into(), ..Default::default() };
        assert!(LoaderSource::from_manifest(&e3, base).unwrap().installer_url.ends_with("/my%20loader%2B1.jar"));

        assert!(LoaderSource::from_manifest(&LoaderEntry::default(), base).is_err());
    }

    #[test]
    fn natives_extraction_is_flat_filtered_and_safe() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("n.jar");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
            let o = zip::write::FileOptions::default();
            for (name, data) in [("lwjgl.dll", &b"d"[..]), ("sub/dir/openal.dll", b"o"), ("META-INF/MANIFEST.MF", b"m"), ("../evil.dll", b"e"), ("skip/this.dll", b"s")] {
                z.start_file(name, o).unwrap();
                z.write_all(data).unwrap();
            }
            z.finish().unwrap();
        }
        let out = dir.path().join("natives");
        let n = extract_natives(&jar, &out, &["skip/".to_string()]).unwrap();
        let mut got: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        got.sort();
        assert_eq!(got, vec!["evil.dll", "lwjgl.dll", "openal.dll"]); // `../` срезан до имени файла
        assert_eq!(n, 3);
        assert!(!dir.path().join("evil.dll").exists());
    }
}
