//! Модуль 3: изолированная портативная JRE (Eclipse Temurin), по одной на каждую нужную major-версию.
//! Minecraft 1.20.1 → Java 17, 1.21.x и актуальная vanilla → Java 21 (`runtime/java17`, `runtime/java21`);
//! уже установленная сборка переиспользуется всеми instance'ами с той же версией.
//!
//! * `JavaRuntime::ensure` — асинхронно проверяет, скачивает (SHA-256 от Adoptium API),
//!   безопасно распаковывает и атомарно подменяет `<launcher>/runtime/java21`.
//! * `JavaRuntime::command` — ЕДИНСТВЕННЫЙ способ получить `Command` для Java в лаунчере:
//!   путь к бинарнику жёстко внутри каталога рантайма, а `JAVA_HOME`, `JDK_HOME`, `JRE_HOME`,
//!   `CLASSPATH`, `JAVA_TOOL_OPTIONS`, `_JAVA_OPTIONS`, `JDK_JAVA_OPTIONS` пользователя
//!   вычищаются. Системная Java не ищется нигде (ни по PATH, ни в Program Files).

use crate::downloader::{
    download_all, fetch_text, CancelToken, DeltaPlan, DownloadOptions, DownloadReason, PlannedFile, ProgressFn,
    RemoteFile,
};
use crate::error::{CoreError, Result};
use crate::fsutil::safe_join;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Major-версия JRE по умолчанию (Minecraft 1.21.x и NeoForge 21.1 требуют Java 21).
pub const JAVA_MAJOR: u32 = 21;

/// Какая Java нужна версии Minecraft (запасной вариант, когда version.json Mojang недоступен).
/// 1.20.5+ и 1.21.x → 21, 1.18–1.20.4 → 17, 1.17 → 16, старше → 8; нераспознанное (новая нумерация) → 21.
pub fn java_major_for_mc(mc: &str) -> u32 {
    let nums: Vec<u32> = mc.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    if mc.starts_with("1.") && nums.len() >= 2 {
        let (minor, patch) = (nums[1], nums.get(2).copied().unwrap_or(0));
        return match minor {
            0..=16 => 8,
            17 => 16,
            18 | 19 => 17,
            20 if patch >= 5 => 21,
            20 => 17,
            _ => 21,
        };
    }
    JAVA_MAJOR
}
const MARKER: &str = ".vilauncher-runtime.json";
/// Защита от zip-бомб: распакованная JRE занимает ~150 МБ.
const MAX_UNPACKED_BYTES: u64 = 1_500_000_000;
const MAX_ENTRIES: usize = 50_000;

/// Переменные окружения, которые способны подменить Java или её параметры.
pub const STRIPPED_ENV: &[&str] = &[
    "JAVA_HOME",
    "JDK_HOME",
    "JRE_HOME",
    "CLASSPATH",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInfo {
    pub major: u32,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaProgress {
    /// check | resolve | download | extract | verify | done
    pub phase: &'static str,
    pub pct: u8,
    pub message: String,
}
pub type JavaProgressFn = Arc<dyn Fn(JavaProgress) + Send + Sync>;

fn report(cb: &Option<JavaProgressFn>, phase: &'static str, pct: u8, message: impl Into<String>) {
    if let Some(cb) = cb {
        cb(JavaProgress { phase, pct, message: message.into() });
    }
}

#[derive(Clone, Debug)]
pub struct JavaRuntime {
    root: PathBuf,
    api_base: String,
    major: u32,
}

#[derive(Deserialize)]
struct Asset {
    binary: AssetBinary,
}
#[derive(Deserialize)]
struct AssetBinary {
    package: AssetPackage,
}
#[derive(Deserialize)]
struct AssetPackage {
    link: String,
    name: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    package: String,
    sha256: Option<String>,
    java_version: String,
}

impl JavaRuntime {
    /// `launcher_dir` — корень лаунчера (`.mc-launcher`). Рантайм живёт в `runtime/java17`.
    pub fn new(launcher_dir: &Path) -> Self {
        Self::with_major(launcher_dir, JAVA_MAJOR)
    }

    /// JRE нужной major-версии: `runtime/java<major>`.
    pub fn with_major(launcher_dir: &Path, major: u32) -> Self {
        Self { root: launcher_dir.join("runtime").join(format!("java{major}")), api_base: "https://api.adoptium.net".into(), major }
    }

    pub fn major(&self) -> u32 {
        self.major
    }

    /// Только для тестов: подмена базового URL Adoptium API.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into().trim_end_matches('/').to_string();
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn java_exe(&self) -> PathBuf {
        exe_in(&self.root)
    }

    /// Команда запуска Java: жёстко наш бинарник + очищенное окружение.
    pub fn command(&self) -> std::process::Command {
        sanitized_command(&self.root)
    }

    /// Быстрая проверка: бинарник на месте?
    pub fn exe_exists(&self) -> bool {
        self.java_exe().is_file()
    }

    /// Полная проверка: бинарник запускается и это именно Java 17.
    pub async fn verify(&self) -> Result<JavaInfo> {
        probe(&self.root, self.major).await
    }

    /// Гарантирует наличие рабочей Java 17 и возвращает путь к `java(.exe)`.
    pub async fn ensure(
        &self,
        client: &reqwest::Client,
        cancel: &CancelToken,
        progress: Option<JavaProgressFn>,
    ) -> Result<PathBuf> {
        report(&progress, "check", 0, format!("Проверяю изолированную Java {}...", self.major));
        if self.exe_exists() {
            match self.verify().await {
                Ok(info) => {
                    report(&progress, "done", 100, format!("Java {} готова", info.version));
                    return Ok(self.java_exe());
                }
                Err(e) => log::warn!("[java] установленный рантайм не прошёл проверку ({e}) — переустанавливаем"),
            }
        }
        self.install(client, cancel, progress).await
    }

    async fn resolve_package(&self, client: &reqwest::Client) -> Result<(String, String, Option<String>, Option<u64>)> {
        let os = adoptium_os()?;
        let arch = adoptium_arch()?;
        let url = format!(
            "{}/v3/assets/latest/{}/hotspot?architecture={arch}&image_type=jre&os={os}&vendor=eclipse",
            self.api_base, self.major
        );
        let want_zip = cfg!(windows);
        match fetch_text(client, &url, 2).await {
            Ok(text) => {
                let assets: Vec<Asset> = serde_json::from_str(&text)?;
                let pkg = assets
                    .into_iter()
                    .map(|a| a.binary.package)
                    .find(|p| if want_zip { p.name.ends_with(".zip") } else { p.name.ends_with(".tar.gz") })
                    .ok_or_else(|| CoreError::other(format!("Adoptium: подходящий архив JRE {} не найден", self.major)))?;
                Ok((pkg.link, pkg.name, pkg.checksum, pkg.size))
            }
            Err(e) => {
                // Запасной путь: прямая ссылка «latest». Контрольной суммы нет — целостность
                // проверяют распаковка архива и запуск `java -version`.
                log::warn!("[java] Adoptium assets API недоступен ({e}); прямая ссылка без SHA-256");
                let name = format!("jre{}.{}", self.major, if want_zip { "zip" } else { "tar.gz" });
                let link = format!(
                    "{}/v3/binary/latest/{}/ga/{os}/{arch}/jre/hotspot/normal/eclipse",
                    self.api_base, self.major
                );
                Ok((link, name.to_string(), None, None))
            }
        }
    }

    async fn install(&self, client: &reqwest::Client, cancel: &CancelToken, progress: Option<JavaProgressFn>) -> Result<PathBuf> {
        let runtime_dir = self.root.parent().ok_or_else(|| CoreError::other("bad runtime path"))?.to_path_buf();
        tokio::fs::create_dir_all(&runtime_dir).await?;

        report(&progress, "resolve", 0, format!("Получаю информацию о Temurin {}...", self.major));
        let (link, name, checksum, size) = self.resolve_package(client).await?;

        // 1. Скачивание с проверкой SHA-256 (через общий движок загрузки).
        let archive = runtime_dir.join(format!("{name}.download"));
        let file = RemoteFile { path: name.clone(), url: link, sha256: checksum.clone(), size, ..Default::default() };
        let plan = DeltaPlan {
            to_download: vec![PlannedFile { file, dest: archive.clone(), reason: DownloadReason::Missing, tag: 0 }],
            up_to_date: 0,
        };
        let pr = progress.clone();
        let dl_progress: ProgressFn = Arc::new(move |e| {
            report(&pr, "download", e.file_pct, format!("Скачиваю Java {JAVA_MAJOR}: {}%", e.file_pct));
        });
        let opts = DownloadOptions { concurrency: 1, retries: 3, ..Default::default() };
        let summary = download_all(client, &plan, &opts, cancel, Some(dl_progress)).await?;
        if let Some(f) = summary.failed.first() {
            return Err(CoreError::other(format!("Не удалось скачать Java {}: {}", self.major, f.error)));
        }

        // 2. Распаковка + проверка + подмена.
        let result = self.install_from_archive(&archive, &name, checksum, &progress).await;
        let _ = tokio::fs::remove_file(&archive).await;
        result
    }

    /// Распаковывает уже скачанный архив в staging, проверяет `java -version` и атомарно
    /// подменяет рабочую директорию. При любой ошибке прежний рантайм остаётся нетронутым.
    pub async fn install_from_archive(
        &self,
        archive: &Path,
        package_name: &str,
        sha256: Option<String>,
        progress: &Option<JavaProgressFn>,
    ) -> Result<PathBuf> {
        let runtime_dir = self.root.parent().ok_or_else(|| CoreError::other("bad runtime path"))?.to_path_buf();
        let staging = runtime_dir.join(format!("java{}.staging", self.major));
        let backup = runtime_dir.join(format!("java{}.old", self.major));

        let _ = tokio::fs::remove_dir_all(&staging).await;
        let _ = tokio::fs::remove_dir_all(&backup).await;
        tokio::fs::create_dir_all(&staging).await?;

        report(progress, "extract", 0, format!("Распаковываю Java {}...", self.major));
        {
            let (a, s) = (archive.to_path_buf(), staging.clone());
            let is_zip = package_name.ends_with(".zip") || is_zip_file(archive);
            let res = tokio::task::spawn_blocking(move || if is_zip { extract_zip(&a, &s) } else { extract_tar_gz(&a, &s) })
                .await
                .map_err(|e| CoreError::other(format!("extract task: {e}")))?;
            if let Err(e) = res {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(e);
            }
        }

        report(progress, "verify", 90, format!("Проверяю Java {}...", self.major));
        let info = match probe(&staging, self.major).await {
            Ok(i) => i,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(e);
            }
        };

        let marker = Marker { package: package_name.to_string(), sha256, java_version: info.version.clone() };
        tokio::fs::write(staging.join(MARKER), serde_json::to_vec_pretty(&marker)?).await?;

        // Подмена: старый → .old, staging → java<N>; при сбое возвращаем старый.
        let had_old = tokio::fs::try_exists(&self.root).await.unwrap_or(false);
        if had_old {
            tokio::fs::rename(&self.root, &backup).await?;
        }
        if let Err(e) = tokio::fs::rename(&staging, &self.root).await {
            if had_old {
                let _ = tokio::fs::rename(&backup, &self.root).await;
            }
            return Err(e.into());
        }
        let _ = tokio::fs::remove_dir_all(&backup).await;
        report(progress, "done", 100, format!("Java {} установлена в изолированную директорию", info.version));
        Ok(self.java_exe())
    }
}

// ───────────────────────────── команда и проверка ─────────────────────────────

fn exe_in(root: &Path) -> PathBuf {
    root.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" })
}

fn sanitized_command(root: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe_in(root));
    for var in STRIPPED_ENV {
        cmd.env_remove(var);
    }
    cmd.env("JAVA_HOME", root);
    // PATH: наш bin первым, чтобы дочерние процессы (если игра/мод вызовет "java") тоже
    // попали на изолированную сборку.
    let mut paths = vec![root.join("bin")];
    if let Some(old) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&old));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Запускает `<root>/bin/java -version` с очищенным окружением.
async fn probe(root: &Path, major: u32) -> Result<JavaInfo> {
    let exe = exe_in(root);
    if !exe.is_file() {
        return Err(CoreError::other(format!("Не найден {}", exe.display())));
    }
    let mut cmd = tokio::process::Command::from(sanitized_command(root));
    cmd.arg("-version").stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .map_err(|_| CoreError::other("java -version завис (таймаут 20 с)"))??;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    let info = parse_java_version(&text).ok_or_else(|| CoreError::other(format!("Не удалось определить версию Java: {}", text.trim())))?;
    if info.major != major {
        return Err(CoreError::other(format!("Ожидалась Java {major}, найдена {}", info.version)));
    }
    Ok(info)
}

/// `openjdk version "17.0.12" 2024-07-16` → (17, "17.0.12"); `"1.8.0_292"` → 8.
pub fn parse_java_version(output: &str) -> Option<JavaInfo> {
    let start = output.find("version \"")? + "version \"".len();
    let rest = &output[start..];
    let version = rest[..rest.find('"')?].to_string();
    let mut parts = version.split(|c: char| c == '.' || c == '_' || c == '-' || c == '+');
    let first: u32 = parts.next()?.parse().ok()?;
    let major = if first == 1 { parts.next()?.parse().ok()? } else { first };
    Some(JavaInfo { major, version })
}

fn adoptium_os() -> Result<&'static str> {
    match std::env::consts::OS {
        "windows" => Ok("windows"),
        "linux" => Ok("linux"),
        "macos" => Ok("mac"),
        other => Err(CoreError::other(format!("Неподдерживаемая ОС: {other}"))),
    }
}
fn adoptium_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("aarch64"),
        other => Err(CoreError::other(format!("Неподдерживаемая архитектура: {other}"))),
    }
}

fn is_zip_file(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"PK\x03\x04"
}

// ───────────────────────────── распаковка ─────────────────────────────

/// Срезает первый компонент пути (`jdk-17.0.12+7-jre/bin/java` → `bin/java`).
/// `Ok(None)` — запись относится только к корневой папке архива.
fn strip_first(path: &Path) -> Result<Option<PathBuf>> {
    let mut comps = path.components();
    match comps.next() {
        None => return Ok(None),
        Some(Component::Normal(_)) | Some(Component::CurDir) => {}
        Some(_) => return Err(CoreError::UnsafePath(path.display().to_string())),
    }
    let rest: PathBuf = comps.collect();
    if rest.as_os_str().is_empty() {
        return Ok(None);
    }
    Ok(Some(rest))
}

fn dest_for(dest_root: &Path, rel: &Path) -> Result<PathBuf> {
    safe_join(dest_root, &rel.to_string_lossy())
}

pub fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    if zip.len() > MAX_ENTRIES {
        return Err(CoreError::other("Архив содержит слишком много файлов"));
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let enclosed = entry.enclosed_name().map(Path::to_path_buf).ok_or_else(|| CoreError::UnsafePath(entry.name().to_string()))?;
        let Some(rel) = strip_first(&enclosed)? else { continue };
        let target = dest_for(dest, &rel)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        let copied = std::io::copy(&mut (&mut entry).take(MAX_UNPACKED_BYTES - total + 1), &mut out)?;
        total += copied;
        if total > MAX_UNPACKED_BYTES {
            return Err(CoreError::other("Распакованный размер превышает допустимый предел"));
        }
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

pub fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    let (mut total, mut count) = (0u64, 0usize);
    for entry in tar.entries()? {
        let mut entry = entry?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(CoreError::other("Архив содержит слишком много файлов"));
        }
        let path = entry.path()?.into_owned();
        let Some(rel) = strip_first(&path)? else { continue };
        let target = dest_for(dest, &rel)?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            total += entry.header().size()?;
            if total > MAX_UNPACKED_BYTES {
                return Err(CoreError::other("Распакованный размер превышает допустимый предел"));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?; // права (в т.ч. +x) берутся из архива
        } else if kind.is_symlink() || kind.is_hard_link() {
            // Ссылки допускаем только относительные и без выхода за пределы дерева.
            let link = entry.link_name()?.ok_or_else(|| CoreError::UnsafePath(path.display().to_string()))?.into_owned();
            let escapes = link.is_absolute() || link.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir));
            if escapes {
                return Err(CoreError::UnsafePath(format!("{} -> {}", path.display(), link.display())));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }
        // прочие типы (устройства, fifo, pax-заголовки) пропускаем
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_java_versions() {
        let j17 = parse_java_version("openjdk version \"17.0.12\" 2024-07-16\nOpenJDK Runtime Environment Temurin-17.0.12+7").unwrap();
        assert_eq!((j17.major, j17.version.as_str()), (17, "17.0.12"));
        assert_eq!(parse_java_version("java version \"1.8.0_292\"").unwrap().major, 8);
        assert_eq!(parse_java_version("openjdk version \"21\" 2023-09-19").unwrap().major, 21);
        assert!(parse_java_version("command not found").is_none());
    }

    #[test]
    fn java_for_minecraft_versions() {
        assert_eq!(java_major_for_mc("1.20.1"), 17);
        assert_eq!(java_major_for_mc("1.20.4"), 17);
        assert_eq!(java_major_for_mc("1.20.5"), 21);
        assert_eq!(java_major_for_mc("1.20.6"), 21);
        assert_eq!(java_major_for_mc("1.21"), 21);
        assert_eq!(java_major_for_mc("1.21.1"), 21);
        assert_eq!(java_major_for_mc("1.18.2"), 17);
        assert_eq!(java_major_for_mc("1.17.1"), 16);
        assert_eq!(java_major_for_mc("1.16.5"), 8);
        assert_eq!(java_major_for_mc("1.12.2"), 8);
        assert_eq!(java_major_for_mc("26.1"), 21, "новая нумерация → запасное значение");
    }

    #[test]
    fn runtimes_of_different_majors_live_side_by_side() {
        let dir = Path::new("/l");
        assert_eq!(JavaRuntime::with_major(dir, 17).root(), Path::new("/l/runtime/java17"));
        assert_eq!(JavaRuntime::new(dir).root(), Path::new("/l/runtime/java21"));
        assert_eq!(JavaRuntime::with_major(dir, 17).major(), 17);
    }

    #[test]
    fn strip_first_component() {
        assert_eq!(strip_first(Path::new("jdk-17+7-jre/bin/java")).unwrap(), Some(PathBuf::from("bin/java")));
        assert_eq!(strip_first(Path::new("jdk-17+7-jre/")).unwrap(), None);
        assert!(strip_first(Path::new("/abs/bin/java")).is_err());
    }
}
