//! Сборка аргументов и запуск Minecraft ТОЛЬКО через изолированную JRE.
//!
//! Здесь нет параметра «путь к Java»: единственный источник бинарника —
//! [`JavaRuntime`], поэтому ни `JAVA_HOME`, ни PATH, ни пользовательские настройки не могут
//! подменить интерпретатор.

use crate::auth::McSession;
use crate::error::{CoreError, Result};
use crate::java_runtime::JavaRuntime;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Структура `client/launch.json`, который пишет установщик Forge.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchJson {
    pub main_class: String,
    pub classpath: Vec<String>,
    #[serde(default)]
    pub module_path: Vec<String>,
    #[serde(default)]
    pub forge_jvm_args: Vec<String>,
    #[serde(default)]
    pub forge_game_args: Vec<String>,
    pub assets_dir: String,
    pub asset_index: String,
    pub natives_dir: String,
    pub game_dir: String,
    /// Версия Minecraft, для которой собран `launch.json` (у старых файлов пусто).
    #[serde(default)]
    pub mc_version: String,
    /// Ключ установленной связки «MC + загрузчик» (см. `installer::install_key`).
    #[serde(default)]
    pub install_key: String,
    /// Какая Java нужна этому клиенту (0 — не записано, определяется по версии Minecraft).
    #[serde(default)]
    pub java_major: u32,
}

impl LaunchJson {
    pub fn java_major_or_default(&self) -> u32 {
        if self.java_major > 0 { self.java_major } else { crate::java_runtime::java_major_for_mc(&self.mc_version) }
    }
}

impl LaunchJson {
    pub async fn load(path: &Path) -> Result<Self> {
        let text = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamPlan {
    pub min_g: u32,
    pub max_g: u32,
}

/// Подбор памяти по объёму ОЗУ (порт `autoRam()`); пользовательские значения приоритетнее.
pub fn ram_plan(total_gb: f64, user_max_g: Option<u32>, user_min_g: Option<u32>) -> RamPlan {
    let (mut min_g, mut max_g) = if total_gb <= 4.2 {
        (1, 2)
    } else if total_gb <= 8.0 {
        (2, 3)
    } else if total_gb <= 16.0 {
        (3, 6.min((total_gb * 0.4).floor() as u32))
    } else if total_gb <= 32.0 {
        (4, 8.min((total_gb * 0.35).floor() as u32))
    } else {
        (4, 10.min((total_gb * 0.3).floor() as u32))
    };
    if let Some(v) = user_max_g.filter(|v| *v > 0) {
        max_g = v;
    }
    if let Some(v) = user_min_g.filter(|v| *v > 0) {
        min_g = v;
    }
    if min_g >= max_g {
        min_g = max_g.saturating_sub(1).max(1);
    }
    RamPlan { min_g, max_g }
}

#[derive(Clone, Debug)]
pub struct LaunchParams<'a> {
    pub nickname: &'a str,
    pub default_username: &'a str,
    pub mc_version: &'a str,
    pub server_address: Option<&'a str>,
    pub ram: RamPlan,
    /// Лицензионная сессия; `None` — офлайн-режим (`--accessToken null --userType legacy`).
    pub session: Option<&'a McSession>,
}

fn path_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// Порт сборки аргументов из Electron-версии (порядок и состав сохранены).
pub fn build_args(l: &LaunchJson, p: &LaunchParams) -> Vec<String> {
    let sep = path_sep();
    let module_path = l.module_path.join(sep);
    let client_jar_name = l
        .classpath
        .iter()
        .find(|c| c.contains(&format!("{}.jar", p.mc_version)))
        .and_then(|c| Path::new(c).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut a: Vec<String> = Vec::new();
    let s = |x: &str| x.to_string();

    // Память + флаги G1 (Aikar)
    a.push(format!("-Xms{}G", p.ram.min_g));
    a.push(format!("-Xmx{}G", p.ram.max_g));
    for f in [
        "-XX:+UseG1GC", "-XX:+ParallelRefProcEnabled", "-XX:MaxGCPauseMillis=200", "-XX:+UnlockExperimentalVMOptions",
        "-XX:+DisableExplicitGC", "-XX:+AlwaysPreTouch", "-XX:G1NewSizePercent=30", "-XX:G1MaxNewSizePercent=40",
        "-XX:G1HeapRegionSize=8M", "-XX:G1ReservePercent=20", "-XX:G1HeapWastePercent=5", "-XX:G1MixedGCCountTarget=4",
        "-XX:InitiatingHeapOccupancyPercent=15", "-XX:G1MixedGCLiveThresholdPercent=90", "-XX:SurvivorRatio=32",
        "-XX:+PerfDisableSharedMem", "-XX:MaxTenuringThreshold=1",
    ] {
        a.push(s(f));
    }
    a.push(format!("-Djava.library.path={}", l.natives_dir));
    a.push(s("-Dminecraft.launcher.brand=mc-launcher"));
    a.push(s("-Dminecraft.launcher.version=1.0"));
    a.push(s("-Dfml.earlyprogresswindow=true"));
    a.push(format!("-DignoreList={client_jar_name}"));
    for open in [
        "java.base/java.util.jar=ALL-UNNAMED", "java.base/java.lang.invoke=ALL-UNNAMED", "java.base/java.lang=ALL-UNNAMED",
        "java.desktop/sun.awt.image=ALL-UNNAMED", "java.base/sun.security.util=ALL-UNNAMED", "java.base/java.net=ALL-UNNAMED",
    ] {
        a.push(s("--add-opens"));
        a.push(s(open));
    }
    if !module_path.is_empty() {
        a.push(s("-p"));
        a.push(module_path);
        a.push(s("--add-modules"));
        a.push(s("ALL-MODULE-PATH"));
    }
    a.extend(l.forge_jvm_args.iter().cloned());
    a.push(s("-cp"));
    a.push(l.classpath.join(sep));
    a.push(l.main_class.clone());

    // Игровые аргументы
    let username = match p.session {
        Some(sess) => sess.username.clone(),
        None if p.nickname.trim().is_empty() => p.default_username.to_string(),
        None => p.nickname.to_string(),
    };
    a.push(s("--username"));
    a.push(username);
    a.push(s("--version"));
    a.push(p.mc_version.to_string());
    a.push(s("--gameDir"));
    a.push(l.game_dir.clone());
    a.push(s("--assetsDir"));
    a.push(l.assets_dir.clone());
    a.push(s("--assetIndex"));
    a.push(l.asset_index.clone());
    match p.session {
        Some(sess) => {
            a.push(s("--uuid"));
            a.push(sess.uuid.clone());
            a.push(s("--accessToken"));
            a.push(sess.access_token.clone());
            a.push(s("--userType"));
            a.push(s("msa"));
        }
        None => {
            a.push(s("--accessToken"));
            a.push(s("null"));
            a.push(s("--userType"));
            a.push(s("legacy"));
        }
    }
    a.extend(l.forge_game_args.iter().cloned());
    if let Some(addr) = p.server_address.filter(|x| !x.trim().is_empty()) {
        a.push(s("--quickPlayMultiplayer"));
        a.push(addr.to_string());
    }
    dedupe_paired_flags(a.into_iter().filter(|x| !x.is_empty()).collect())
}

/// Убирает дубли парных JVM-флагов (`--add-opens X`, `-p X`...), не разрывая «флаг + значение».
fn dedupe_paired_flags(raw: Vec<String>) -> Vec<String> {
    const PAIRED: &[&str] = &["--add-modules", "--add-opens", "--add-exports", "--add-reads", "-p", "--module-path"];
    let mut out = Vec::with_capacity(raw.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i < raw.len() {
        let a = &raw[i];
        if PAIRED.contains(&a.as_str()) && i + 1 < raw.len() {
            let key = format!("{a}|{}", raw[i + 1]);
            if seen.insert(key) {
                out.push(a.clone());
                out.push(raw[i + 1].clone());
            }
            i += 2;
        } else {
            out.push(a.clone());
            i += 1;
        }
    }
    out
}

/// Есть ли в хвосте файла сигнатура конца ZIP (`PK\x05\x06`)? Читается только последние 64 КиБ —
/// в отличие от прежней версии, файл не загружается в память целиком.
pub fn zip_tail_ok(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    if len < 22 {
        return false;
    }
    let tail_len = len.min(65_557);
    if f.seek(SeekFrom::Start(len - tail_len)).is_err() {
        return false;
    }
    let mut buf = vec![0u8; tail_len as usize];
    f.read_exact(&mut buf).is_ok() && buf.windows(4).rev().any(|w| w == b"PK\x05\x06")
}

#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub missing_classpath: Vec<String>,
    pub corrupt_client_jar: Option<String>,
    pub corrupt_mods: Vec<String>,
}

impl Preflight {
    pub fn is_ok(&self) -> bool {
        self.missing_classpath.is_empty() && self.corrupt_client_jar.is_none() && self.corrupt_mods.is_empty()
    }
}

/// Предстартовая проверка целостности (classpath, client.jar, моды). Ничего не удаляет —
/// решение о ремонте принимает вызывающий код.
pub async fn preflight(l: &LaunchJson, mc_version: &str) -> Result<Preflight> {
    let l = l.clone();
    let mc_version = mc_version.to_string();
    tokio::task::spawn_blocking(move || {
        let mut rep = Preflight::default();
        for p in &l.classpath {
            if !Path::new(p).exists() {
                rep.missing_classpath.push(p.clone());
            }
        }
        let jar_suffix = format!("{mc_version}.jar");
        if let Some(cj) = l.classpath.iter().find(|p| p.ends_with(&jar_suffix) && p.contains("versions")) {
            if Path::new(cj).exists() && !zip_tail_ok(Path::new(cj)) {
                rep.corrupt_client_jar = Some(cj.clone());
            }
        }
        let mods = Path::new(&l.game_dir).join("mods");
        if let Ok(rd) = std::fs::read_dir(&mods) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.to_ascii_lowercase().ends_with(".jar") && !zip_tail_ok(&e.path()) {
                    rep.corrupt_mods.push(name);
                }
            }
            rep.corrupt_mods.sort();
        }
        Ok(rep)
    })
    .await
    .map_err(|e| CoreError::other(format!("preflight task: {e}")))?
}

/// Запуск игры. Бинарник — всегда `JavaRuntime::java_exe()`; окружение очищено от `JAVA_HOME` и др.
/// stdout/stderr игры пишутся в `log_path`. Требует tokio-runtime.
pub fn spawn_game(rt: &JavaRuntime, l: &LaunchJson, args: &[String], log_path: &Path) -> Result<tokio::process::Child> {
    if !rt.exe_exists() {
        return Err(CoreError::other(format!(
            "Изолированная Java {} не найдена: {}\nНажми «Переустановить клиент» в настройках.",
            rt.major(),
            rt.java_exe().display()
        )));
    }
    let log = std::fs::File::create(log_path)?;
    let log_err = log.try_clone()?;
    let mut cmd = tokio::process::Command::from(rt.command());
    cmd.args(args)
        .current_dir(PathBuf::from(&l.game_dir))
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .kill_on_drop(false);
    Ok(cmd.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LaunchJson {
        LaunchJson {
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".into(),
            classpath: vec!["/c/libraries/a.jar".into(), "/c/versions/1.20.1/1.20.1.jar".into()],
            module_path: vec!["/c/libraries/securejarhandler.jar".into(), "/c/libraries/asm.jar".into()],
            forge_jvm_args: vec!["-DlegacyClassPath=x".into(), "--add-opens".into(), "java.base/java.lang=ALL-UNNAMED".into()],
            forge_game_args: vec!["--launchTarget".into(), "forgeclient".into()],
            assets_dir: "/c/assets".into(),
            asset_index: "5".into(),
            natives_dir: "/c/natives".into(),
            game_dir: "/c".into(),
            mc_version: "1.20.1".into(),
            install_key: String::new(),
            java_major: 0,
        }
    }

    fn pos(args: &[String], needle: &str) -> usize {
        args.iter().position(|a| a == needle).unwrap_or_else(|| panic!("нет {needle}"))
    }

    #[test]
    fn ram_table_matches_electron_version() {
        assert_eq!(ram_plan(4.0, None, None), RamPlan { min_g: 1, max_g: 2 });
        assert_eq!(ram_plan(8.0, None, None), RamPlan { min_g: 2, max_g: 3 });
        assert_eq!(ram_plan(16.0, None, None), RamPlan { min_g: 3, max_g: 6 });
        assert_eq!(ram_plan(12.0, None, None), RamPlan { min_g: 3, max_g: 4 }); // floor(12*0.4)=4
        assert_eq!(ram_plan(32.0, None, None), RamPlan { min_g: 4, max_g: 8 });
        assert_eq!(ram_plan(64.0, None, None), RamPlan { min_g: 4, max_g: 10 });
        assert_eq!(ram_plan(16.0, Some(5), Some(5)), RamPlan { min_g: 4, max_g: 5 }); // min < max всегда
        assert_eq!(ram_plan(16.0, Some(8), None), RamPlan { min_g: 3, max_g: 8 });
    }

    #[test]
    fn offline_args() {
        let p = LaunchParams { nickname: "", default_username: "Player", mc_version: "1.20.1", server_address: Some("host:25565"), ram: RamPlan { min_g: 3, max_g: 6 }, session: None };
        let a = build_args(&sample(), &p);
        assert_eq!(a[0], "-Xms3G");
        assert_eq!(a[1], "-Xmx6G");
        assert!(a.contains(&"-DignoreList=1.20.1.jar".to_string()));
        assert_eq!(a[pos(&a, "--username") + 1], "Player");
        assert_eq!(a[pos(&a, "--accessToken") + 1], "null");
        assert_eq!(a[pos(&a, "--userType") + 1], "legacy");
        assert_eq!(a[pos(&a, "--quickPlayMultiplayer") + 1], "host:25565");
        // main class стоит после -cp и перед игровыми аргументами
        assert!(pos(&a, "-cp") < pos(&a, "cpw.mods.bootstraplauncher.BootstrapLauncher"));
        assert!(pos(&a, "cpw.mods.bootstraplauncher.BootstrapLauncher") < pos(&a, "--username"));
        assert!(pos(&a, "--username") < pos(&a, "--launchTarget"));
    }

    #[test]
    fn microsoft_session_args() {
        let sess = McSession { username: "Notch".into(), uuid: "069a79f4".into(), access_token: "TOKEN".into(), expires_at: u64::MAX };
        let p = LaunchParams { nickname: "ignored", default_username: "Player", mc_version: "1.20.1", server_address: None, ram: RamPlan { min_g: 2, max_g: 3 }, session: Some(&sess) };
        let a = build_args(&sample(), &p);
        assert_eq!(a[pos(&a, "--username") + 1], "Notch");
        assert_eq!(a[pos(&a, "--uuid") + 1], "069a79f4");
        assert_eq!(a[pos(&a, "--accessToken") + 1], "TOKEN");
        assert_eq!(a[pos(&a, "--userType") + 1], "msa");
        assert!(!a.contains(&"--quickPlayMultiplayer".to_string()));
    }

    #[test]
    fn duplicate_paired_flags_are_removed_without_breaking_pairs() {
        let p = LaunchParams { nickname: "x", default_username: "P", mc_version: "1.20.1", server_address: None, ram: RamPlan { min_g: 2, max_g: 3 }, session: None };
        let a = build_args(&sample(), &p);
        // `--add-opens java.base/java.lang=ALL-UNNAMED` задан и нами, и Forge — должен остаться один раз.
        let n = a.windows(2).filter(|w| w[0] == "--add-opens" && w[1] == "java.base/java.lang=ALL-UNNAMED").count();
        assert_eq!(n, 1);
        // а каждый --add-opens по-прежнему имеет значение
        for (i, x) in a.iter().enumerate() {
            if x == "--add-opens" {
                assert!(a[i + 1].contains('='), "{x} без значения");
            }
        }
    }

    #[test]
    fn zip_tail_detection() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.jar");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&good).unwrap());
        z.start_file("a.txt", zip::write::FileOptions::default()).unwrap();
        std::io::Write::write_all(&mut z, b"hello").unwrap();
        z.finish().unwrap();
        assert!(zip_tail_ok(&good));

        let bad = dir.path().join("bad.jar");
        let bytes = std::fs::read(&good).unwrap();
        std::fs::write(&bad, &bytes[..bytes.len() - 30]).unwrap(); // обрезанная загрузка
        assert!(!zip_tail_ok(&bad));
        assert!(!zip_tail_ok(&dir.path().join("missing.jar")));
    }
}
