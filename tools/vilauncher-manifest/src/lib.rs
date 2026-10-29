//! Логика генерации `manifest.json` и `hashes.json` для ViLauncher.
//!
//! Принцип: файлы в репозитории — источник истины, манифесты подстраиваются под них.
//!
//! `manifest.json` — список модов (проверяется при запуске и во время игры):
//!  * `groups.mods` ← `mods/*.jar`
//!  * `groups.core` ← сохраняется из прежнего манифеста
//!  * `loader`      ← installer NeoForge/Forge (`*-installer.jar` в корне)
//!  * `version`     ← растёт только если содержимое изменилось
//! `defaults.json` — настройки по умолчанию (`options.txt`, `config/**`): игрок получает их, только если
//! файла нет или нажал «Вернуть настройки по умолчанию».
//! `blocklist.json` — «чёрный манифест» с ключевыми словами читов; создаётся шаблон, если файла ещё нет.
//! Все прочие ключи прежнего манифеста сохраняются как есть.

use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub repo: PathBuf,
    pub mc: Option<String>,
    pub version: Option<String>,
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    pub client_revision: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub sha1: String,
    pub size: u64,
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({ "path": self.path, "sha1": self.sha1, "size": self.size })
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub manifest: Value,
    pub hashes: BTreeMap<String, String>,
    /// Изменилось ли содержимое манифеста (без учёта `version`).
    pub changed: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
    pub mods: usize,
    pub defaults: usize,
    /// Содержимое `defaults.json`.
    pub defaults_manifest: Value,
    pub defaults_changed: bool,
}

// ───────────────────────────── хеширование ─────────────────────────────

pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Параллельно считает SHA-1 списка файлов (`rel` — путь в репозитории через `/`).
pub fn hash_entries(repo: &Path, rels: &[String]) -> Result<Vec<Entry>, String> {
    let results: Mutex<Vec<Option<Result<Entry, String>>>> = Mutex::new((0..rels.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8).max(1);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= rels.len() {
                    break;
                }
                let p = repo.join(rels[i].replace('/', std::path::MAIN_SEPARATOR_STR));
                let r = sha1_file(&p)
                    .and_then(|h| std::fs::metadata(&p).map(|m| Entry { path: rels[i].clone(), sha1: h, size: m.len() }))
                    .map_err(|e| format!("{}: {e}", rels[i]));
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

// ───────────────────────────── сканирование ─────────────────────────────

fn rel_of(repo: &Path, p: &Path) -> String {
    p.strip_prefix(repo).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk_files(&p, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

fn scan_mods(repo: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(repo.join("mods"))
        .map(|rd| rd.flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.retain(|n| n.to_lowercase().ends_with(".jar"));
    v.sort_by_key(|n| n.to_lowercase());
    v.into_iter().map(|n| format!("mods/{n}")).collect()
}

fn scan_defaults(repo: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if repo.join("options.txt").is_file() {
        v.push("options.txt".to_string());
    }
    let mut files = Vec::new();
    walk_files(&repo.join("config"), &mut files);
    v.extend(files.iter().map(|p| rel_of(repo, p)));
    v
}

/// Числа из имени файла для сравнения версий (`neoforge-21.1.172-installer.jar` → [21,1,172]).
fn version_key(name: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in name.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(cur.parse().unwrap_or(0));
            cur.clear();
        }
    }
    out
}

fn find_installers(repo: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(repo)
        .map(|rd| rd.flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.retain(|n| n.to_lowercase().ends_with("-installer.jar"));
    // NeoForge приоритетнее Forge, внутри — самая свежая версия.
    v.sort_by(|a, b| {
        let (na, nb) = (a.to_lowercase().contains("neoforge"), b.to_lowercase().contains("neoforge"));
        (nb, version_key(b)).cmp(&(na, version_key(a)))
    });
    v
}

/// Метка версии загрузчика из имени: `neoforge-21.1.172-installer.jar` → `21.1.172`.
pub fn loader_version_from_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(".jar").unwrap_or(&lower);
    let stem = stem.strip_suffix("-installer").unwrap_or(stem);
    let tail = stem.strip_prefix("neoforge-").or_else(|| stem.strip_prefix("forge-")).unwrap_or(stem);
    // сохраняем исходный регистр суффиксов вроде `-beta`
    if tail.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { tail.to_string() } else { stem.to_string() }
}

/// Версия Minecraft по версии загрузчика: NeoForge `21.1.172` → `1.21.1`, `21.0.5` → `1.21`;
/// Forge `1.20.1-47.4.16` → `1.20.1`.
pub fn mc_from_loader(installer: &str, version: &str) -> Option<String> {
    let neo = installer.to_lowercase().contains("neoforge");
    let nums: Vec<&str> = version.split(['.', '-']).collect();
    if neo {
        let a: u32 = nums.first()?.parse().ok()?;
        let b: u32 = nums.get(1)?.parse().ok()?;
        return Some(if b == 0 { format!("1.{a}") } else { format!("1.{a}.{b}") });
    }
    let head = version.split('-').next()?;
    (head.starts_with("1.") && head.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| head.to_string())
}

fn bump_version(old: &str) -> String {
    let mut parts: Vec<u64> = match old.trim().split('.').map(|p| p.parse::<u64>()).collect::<Result<Vec<_>, _>>() {
        Ok(p) if !p.is_empty() => p,
        _ => return "1.0.0".to_string(),
    };
    *parts.last_mut().unwrap() += 1;
    parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".")
}

// ───────────────────────────── генерация ─────────────────────────────

fn read_json(path: &Path) -> Option<Map<String, Value>> {
    let t = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn entry_map(m: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(groups) = m.get("groups").and_then(|g| g.as_object()) {
        for list in groups.values().filter_map(|l| l.as_array()) {
            for e in list {
                if let Some(p) = e.get("path").and_then(|p| p.as_str()) {
                    out.insert(p.to_string(), e.get("sha1").and_then(|h| h.as_str()).unwrap_or("").to_string());
                }
            }
        }
    }
    out
}

pub fn build(opts: &Options) -> Result<Report, String> {
    let repo = &opts.repo;
    let mut rep = Report::default();
    let old = read_json(&repo.join("manifest.json")).unwrap_or_default();

    let mod_rels = scan_mods(repo);
    if mod_rels.is_empty() {
        return Err("В папке mods/ нет ни одного .jar — манифест не создаётся (иначе лаунчер удалит у игроков все моды).".into());
    }
    let default_rels = scan_defaults(repo);
    let mods = hash_entries(repo, &mod_rels)?;
    let defaults = hash_entries(repo, &default_rels)?;
    rep.mods = mods.len();
    rep.defaults = defaults.len();

    // ── загрузчик ──
    let old_loader = old.get("loader").and_then(|l| l.as_object()).cloned();
    let installers = find_installers(repo);
    let chosen: Option<String> = match &opts.loader {
        Some(l) => {
            if !repo.join(l).is_file() {
                return Err(format!("Не найден installer: {}", repo.join(l).display()));
            }
            Some(l.clone())
        }
        None => {
            let existing = old_loader.as_ref().and_then(|l| l.get("installer")).and_then(|i| i.as_str()).map(String::from);
            match existing {
                Some(e) if !e.starts_with("http") && repo.join(&e).is_file() && installers.iter().all(|i| version_key(i) <= version_key(&e) || !i.to_lowercase().contains("neoforge") && e.to_lowercase().contains("neoforge")) => Some(e),
                Some(e) if e.starts_with("http") => {
                    rep.notes.push(format!("loader.installer — внешний URL, оставлен без изменений: {e}"));
                    None
                }
                _ => installers.first().cloned(),
            }
        }
    };
    let mut loader_value: Option<Value> = old_loader.map(Value::Object);
    let mut loader_version = String::new();
    let mut loader_name = String::new();
    if let Some(name) = &chosen {
        let sha1 = sha1_file(&repo.join(name)).map_err(|e| format!("{name}: {e}"))?;
        loader_version = opts.loader_version.clone().unwrap_or_else(|| loader_version_from_name(name));
        loader_name = name.clone();
        let kind = if name.to_lowercase().contains("neoforge") { "neoforge" } else { "forge" };
        loader_value = Some(json!({ "type": kind, "version": loader_version, "installer": name, "sha1": sha1 }));
        if old.get("loader").and_then(|l| l.get("installer")).and_then(|i| i.as_str()) != Some(name.as_str()) {
            rep.notes.push(format!("Загрузчик: {name} (версия {loader_version})"));
        }
    } else if loader_value.is_none() {
        rep.warnings.push("Не найден installer загрузчика (*-installer.jar в корне) — укажи --loader. Без раздела loader установка клиента не сработает.".into());
    }

    // ── версия Minecraft ──
    let old_mc = old.get("mcVersion").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let derived = if !loader_name.is_empty() { mc_from_loader(&loader_name, &loader_version) } else { None };
    let mc = opts.mc.clone().or(derived.clone()).unwrap_or_else(|| old_mc.clone());
    if mc.is_empty() {
        rep.warnings.push("Не удалось определить версию Minecraft — укажи --mc.".into());
    } else if opts.mc.is_none() && derived.is_some() && !old_mc.is_empty() && old_mc != mc {
        rep.notes.push(format!("Версия Minecraft изменена по имени installer'а: {old_mc} → {mc}"));
    }
    if let Some(wrong) = opts.mc.as_ref().zip(derived.as_ref()).filter(|(a, b)| a != b) {
        rep.warnings.push(format!("--mc {} не совпадает с версией по имени installer'а ({}) — проверь!", wrong.0, wrong.1));
    }

    // ── сборка манифеста (прежние ключи сохраняются) ──
    let old_defaults = read_json(&repo.join("defaults.json")).unwrap_or_default();
    let mut m = old.clone();
    m.insert("mcVersion".into(), json!(mc));
    match opts.client_revision.map(Value::from).or_else(|| old.get("clientRevision").cloned()) {
        Some(r) => {
            m.insert("clientRevision".into(), r);
        }
        None => {
            m.shift_remove("clientRevision");
        }
    }
    if let Some(l) = &loader_value {
        m.insert("loader".into(), l.clone());
    }
    let core = old.get("groups").and_then(|g| g.get("core")).cloned().unwrap_or_else(|| json!([]));
    m.insert("groups".into(), json!({ "core": core, "mods": mods.iter().map(Entry::to_json).collect::<Vec<_>>() }));

    // ── defaults.json ──
    let new_files: Vec<Value> = defaults.iter().map(Entry::to_json).collect();
    let old_files: Vec<Value> = old_defaults.get("files").and_then(|f| f.as_array()).cloned().unwrap_or_default();
    let legacy_defaults = old.get("groups").and_then(|g| g.get("defaults")).and_then(|d| d.as_array()).map(|d| !d.is_empty()).unwrap_or(false);
    rep.defaults_changed = new_files != old_files || (legacy_defaults && old_defaults.is_empty());
    if legacy_defaults {
        rep.notes.push("Настройки по умолчанию вынесены из manifest.json в defaults.json".into());
    }
    let old_defaults_version = old_defaults.get("version").and_then(|v| v.as_str()).map(String::from);
    let defaults_version = match (rep.defaults_changed, old_defaults_version) {
        (true, Some(v)) => bump_version(&v),
        (false, Some(v)) => v,
        _ => "1.0.0".to_string(),
    };
    rep.defaults_manifest = json!({ "version": defaults_version, "files": new_files });

    // ── что изменилось ──
    let (mut a, mut b) = (old.clone(), m.clone());
    a.shift_remove("version");
    b.shift_remove("version");
    rep.changed = a != b;
    let (mut before, mut after) = (entry_map(&old), entry_map(&m));
    for e in &old_files {
        if let Some(p) = e.get("path").and_then(|p| p.as_str()) {
            before.insert(p.to_string(), e.get("sha1").and_then(|h| h.as_str()).unwrap_or("").to_string());
        }
    }
    for e in &defaults {
        after.insert(e.path.clone(), e.sha1.clone());
    }
    for (p, h) in &after {
        match before.get(p) {
            None => rep.added.push(p.clone()),
            Some(old_h) if old_h != h => rep.modified.push(p.clone()),
            _ => {}
        }
    }
    rep.removed = before.keys().filter(|p| !after.contains_key(*p)).cloned().collect();

    let old_version = old.get("version").and_then(|v| v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string())));
    let version = match (&opts.version, rep.changed, old_version) {
        (Some(v), _, _) => v.clone(),
        (None, true, Some(ov)) => bump_version(&ov),
        (None, true, None) => "1.0.0".into(),
        (None, false, Some(ov)) => ov,
        (None, false, None) => "1.0.0".into(),
    };
    if opts.version.is_some() && Some(version.as_str()) != old.get("version").and_then(|v| v.as_str()) {
        rep.changed = true;
    }
    // version — первым ключом
    let mut ordered = Map::new();
    ordered.insert("version".into(), json!(version));
    for (k, v) in m {
        if k != "version" {
            ordered.insert(k, v);
        }
    }
    rep.hashes = after.into_iter().filter(|(_, h)| !h.is_empty()).collect();
    rep.changed = rep.changed || rep.defaults_changed;
    rep.manifest = Value::Object(ordered);
    Ok(rep)
}

fn write_if_changed(path: &Path, text: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).map(|t| t == text).unwrap_or(false) {
        return Ok(false);
    }
    std::fs::write(path, text)?;
    Ok(true)
}

/// Записывает manifest.json и hashes.json (только если содержимое отличается). Возвращает, что записано.
pub fn write(repo: &Path, rep: &Report) -> std::io::Result<Vec<&'static str>> {
    let mut written = Vec::new();
    let m = serde_json::to_string_pretty(&rep.manifest).map_err(std::io::Error::other)? + "\n";
    if write_if_changed(&repo.join("manifest.json"), &m)? {
        written.push("manifest.json");
    }
    let d = serde_json::to_string_pretty(&rep.defaults_manifest).map_err(std::io::Error::other)? + "\n";
    if write_if_changed(&repo.join("defaults.json"), &d)? {
        written.push("defaults.json");
    }
    let h = serde_json::to_string_pretty(&rep.hashes).map_err(std::io::Error::other)? + "\n";
    if write_if_changed(&repo.join("hashes.json"), &h)? {
        written.push("hashes.json");
    }
    // «Чёрный манифест»: существующий blocklist.json не трогаем, при отсутствии создаём шаблон.
    if !repo.join("blocklist.json").exists() {
        std::fs::write(repo.join("blocklist.json"), BLOCKLIST_TEMPLATE)?;
        written.push("blocklist.json (создан шаблон — проверь список слов)");
    }
    Ok(written)
}

/// Шаблон `blocklist.json`: ключевые слова сверяются с именами файлов (без расширения, без учёта регистра).
pub const BLOCKLIST_TEMPLATE: &str = r#"{
  "_version": 1,
  "_comment": "Чёрный манифест: если имя файла содержит одно из слов, лаунчер не запустит игру, а во время игры закроет её с предупреждением. Слова — подстроки имени файла без расширения, регистр не важен.",
  "mods": ["wurst", "meteor-client", "meteorclient", "liquidbounce", "aristois", "bleachhack", "rusherhack", "futureclient", "killaura", "xray", "x-ray", "freecam"],
  "resourcepacks": ["xray", "x-ray"],
  "shaderpacks": [],
  "any": []
}
"#;

// ───────────────────────────── проверка ─────────────────────────────

#[derive(Debug, Default)]
pub struct CheckReport {
    pub ok: usize,
    pub missing: Vec<String>,
    pub mismatched: Vec<String>,
    pub not_in_manifest: Vec<String>,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.mismatched.is_empty() && self.not_in_manifest.is_empty()
    }
}

/// Сверяет файлы с manifest.json: пропавшие, изменившиеся и «лишние» моды/installer.
pub fn check(repo: &Path) -> Result<CheckReport, String> {
    let m = read_json(&repo.join("manifest.json")).ok_or("manifest.json не найден или повреждён")?;
    let mut rep = CheckReport::default();
    let mut expected: Vec<(String, String, Option<u64>)> = Vec::new();
    if let Some(groups) = m.get("groups").and_then(|g| g.as_object()) {
        for list in groups.values().filter_map(|l| l.as_array()) {
            for e in list {
                if let Some(p) = e.get("path").and_then(|p| p.as_str()) {
                    expected.push((p.to_string(), e.get("sha1").and_then(|h| h.as_str()).unwrap_or("").to_string(), e.get("size").and_then(|s| s.as_u64())));
                }
            }
        }
    }
    if let Some(files) = read_json(&repo.join("defaults.json")).and_then(|d| d.get("files").and_then(|f| f.as_array()).cloned()) {
        for e in files {
            if let Some(p) = e.get("path").and_then(|p| p.as_str()) {
                expected.push((p.to_string(), e.get("sha1").and_then(|h| h.as_str()).unwrap_or("").to_string(), e.get("size").and_then(|s| s.as_u64())));
            }
        }
    }
    if let Some(l) = m.get("loader") {
        if let Some(i) = l.get("installer").and_then(|i| i.as_str()).filter(|i| !i.starts_with("http")) {
            expected.push((i.to_string(), l.get("sha1").and_then(|h| h.as_str()).unwrap_or("").to_string(), None));
        }
    }
    let present: Vec<String> = expected.iter().filter(|(p, _, _)| repo.join(p.replace('/', std::path::MAIN_SEPARATOR_STR)).is_file()).map(|(p, _, _)| p.clone()).collect();
    rep.missing = expected.iter().filter(|(p, _, _)| !present.contains(p)).map(|(p, _, _)| p.clone()).collect();
    for e in hash_entries(repo, &present)? {
        let (_, want, size) = expected.iter().find(|(p, _, _)| *p == e.path).unwrap();
        if (!want.is_empty() && !want.eq_ignore_ascii_case(&e.sha1)) || size.map(|s| s != e.size).unwrap_or(false) {
            rep.mismatched.push(e.path);
        } else {
            rep.ok += 1;
        }
    }
    let listed: std::collections::HashSet<&String> = expected.iter().map(|(p, _, _)| p).collect();
    rep.not_in_manifest = scan_mods(repo).into_iter().filter(|p| !listed.contains(p)).collect();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("mods")).unwrap();
        std::fs::create_dir_all(d.path().join("config/sub")).unwrap();
        std::fs::write(d.path().join("mods/B-mod.jar"), b"bbb").unwrap();
        std::fs::write(d.path().join("mods/a-mod.JAR"), b"aaa").unwrap();
        std::fs::write(d.path().join("mods/notes.txt"), b"x").unwrap();
        std::fs::write(d.path().join("options.txt"), b"opt").unwrap();
        std::fs::write(d.path().join("config/c.toml"), b"c").unwrap();
        std::fs::write(d.path().join("config/sub/d.json"), b"d").unwrap();
        std::fs::write(d.path().join("neoforge-21.1.172-installer.jar"), b"inst172").unwrap();
        d
    }

    fn run(dir: &Path, f: impl FnOnce(&mut Options)) -> Report {
        let mut o = Options { repo: dir.to_path_buf(), ..Default::default() };
        f(&mut o);
        let r = build(&o).unwrap();
        write(dir, &r).unwrap();
        r
    }

    #[test]
    fn name_parsing() {
        assert_eq!(loader_version_from_name("neoforge-21.1.172-installer.jar"), "21.1.172");
        assert_eq!(loader_version_from_name("neoforge-21.1.0-beta-installer.jar"), "21.1.0-beta");
        assert_eq!(loader_version_from_name("forge-1.20.1-47.4.16-installer.jar"), "1.20.1-47.4.16");
        assert_eq!(mc_from_loader("neoforge-21.1.172-installer.jar", "21.1.172").as_deref(), Some("1.21.1"));
        assert_eq!(mc_from_loader("neoforge-21.0.9-installer.jar", "21.0.9").as_deref(), Some("1.21"));
        assert_eq!(mc_from_loader("neoforge-20.4.237-installer.jar", "20.4.237").as_deref(), Some("1.20.4"));
        assert_eq!(mc_from_loader("forge-1.20.1-47.4.16-installer.jar", "1.20.1-47.4.16").as_deref(), Some("1.20.1"));
        assert_eq!(bump_version("2.0.9"), "2.0.10");
        assert_eq!(bump_version("weird"), "1.0.0");
    }

    #[test]
    fn first_run_builds_everything_and_matches_launcher_format() {
        let d = repo();
        let repo_path = d.path().to_path_buf();
        let r = run(d.path(), |_| {});
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        let _ = &m;
        assert_eq!(m["version"], "1.0.0");
        assert_eq!(m["mcVersion"], "1.21.1", "определена по имени installer'а");
        assert_eq!(m["loader"]["installer"], "neoforge-21.1.172-installer.jar");
        assert_eq!(m["loader"]["version"], "21.1.172");
        assert_eq!(m["loader"]["sha1"], sha1_file(&d.path().join("neoforge-21.1.172-installer.jar")).unwrap());
        let mods: Vec<&str> = m["groups"]["mods"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
        assert_eq!(mods, vec!["mods/a-mod.JAR", "mods/B-mod.jar"], "только .jar, по алфавиту без учёта регистра");
        assert!(m["groups"].get("defaults").is_none(), "настройки по умолчанию — в defaults.json, не в manifest.json");
        let dm: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("defaults.json")).unwrap()).unwrap();
        let defaults: Vec<&str> = dm["files"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
        assert_eq!(defaults, vec!["options.txt", "config/c.toml", "config/sub/d.json"]);
        assert_eq!(dm["version"], "1.0.0");
        let bl: Value = serde_json::from_str(&std::fs::read_to_string(repo_path.join("blocklist.json")).unwrap()).unwrap();
        assert!(bl["mods"].as_array().unwrap().len() > 3 && bl["any"].is_array(), "создан шаблон чёрного манифеста");
        assert_eq!(m["groups"]["mods"][0]["size"], 3);
        let h: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("hashes.json")).unwrap()).unwrap();
        assert_eq!(h["mods/B-mod.jar"], m["groups"]["mods"][1]["sha1"]);
        assert_eq!((r.mods, r.defaults, r.added.len()), (2, 3, 5));
        // порядок ключей: version первым
        assert_eq!(m.as_object().unwrap().keys().next().unwrap(), "version");
    }

    #[test]
    fn second_run_is_idempotent_and_keeps_version() {
        let d = repo();
        run(d.path(), |_| {});
        let before = std::fs::read_to_string(d.path().join("manifest.json")).unwrap();
        let r = run(d.path(), |_| {});
        assert!(!r.changed && r.added.is_empty() && r.removed.is_empty() && r.modified.is_empty());
        assert_eq!(std::fs::read_to_string(d.path().join("manifest.json")).unwrap(), before);
    }

    #[test]
    fn change_bumps_version_and_reports_diff() {
        let d = repo();
        run(d.path(), |_| {});
        std::fs::write(d.path().join("mods/B-mod.jar"), b"bbb-v2").unwrap(); // изменён
        std::fs::remove_file(d.path().join("mods/a-mod.JAR")).unwrap(); // удалён
        std::fs::write(d.path().join("mods/new.jar"), b"n").unwrap(); // добавлен
        let r = run(d.path(), |_| {});
        assert!(r.changed);
        assert_eq!((r.added.as_slice(), r.removed.as_slice(), r.modified.as_slice()), (&["mods/new.jar".to_string()][..], &["mods/a-mod.JAR".to_string()][..], &["mods/B-mod.jar".to_string()][..]));
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["version"], "1.0.1");
        // явная версия приоритетнее
        let r = run(d.path(), |o| o.version = Some("3.0.0".into()));
        assert!(r.changed);
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["version"], "3.0.0");
    }

    #[test]
    fn loader_update_picks_newest_installer_and_derives_mc() {
        let d = repo();
        run(d.path(), |_| {});
        std::fs::write(d.path().join("neoforge-21.1.200-installer.jar"), b"inst200").unwrap();
        std::fs::write(d.path().join("forge-1.20.1-47.4.16-installer.jar"), b"old forge").unwrap();
        let r = run(d.path(), |_| {});
        assert!(r.changed);
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["loader"]["installer"], "neoforge-21.1.200-installer.jar", "NeoForge новее и приоритетнее Forge");
        assert_eq!(m["loader"]["version"], "21.1.200");
        assert_eq!(m["version"], "1.0.1");
        // перезапуск — без изменений
        assert!(!run(d.path(), |_| {}).changed);
    }

    #[test]
    fn explicit_options_and_preserved_keys() {
        let d = repo();
        std::fs::write(d.path().join("manifest.json"), r#"{"version":"1.2.7","clientRevision":4,"custom":{"keep":true},"groups":{"core":[{"path":"bin/x.dat","sha1":"aa"}]}}"#).unwrap();
        let r = run(d.path(), |o| {
            o.mc = Some("1.21.1".into());
            o.client_revision = Some(5);
            o.loader = Some("neoforge-21.1.172-installer.jar".into());
        });
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["custom"]["keep"], true, "чужие ключи сохраняются");
        assert_eq!(m["clientRevision"], 5);
        assert_eq!(m["groups"]["core"][0]["path"], "bin/x.dat", "core сохраняется");
        assert_eq!(m["version"], "1.2.8");
        // несовпадение --mc и installer'а → предупреждение
        let r = run(d.path(), |o| o.mc = Some("1.20.1".into()));
        assert!(r.warnings.iter().any(|w| w.contains("не совпадает")), "{:?}", r.warnings);
    }

    #[test]
    fn defaults_change_bumps_only_defaults_version_and_existing_blocklist_is_kept() {
        let d = repo();
        std::fs::write(d.path().join("blocklist.json"), r#"{"mods":["mine"]}"#).unwrap();
        run(d.path(), |_| {});
        assert_eq!(std::fs::read_to_string(d.path().join("blocklist.json")).unwrap(), r#"{"mods":["mine"]}"#, "чужой blocklist не перезаписывается");
        let man = |f: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(d.path().join(f)).unwrap()).unwrap() };
        let (mv, dv) = (man("manifest.json")["version"].clone(), man("defaults.json")["version"].clone());
        std::fs::write(d.path().join("config/c.toml"), b"changed").unwrap();
        let r = run(d.path(), |_| {});
        assert!(r.defaults_changed && r.modified == vec!["config/c.toml"]);
        assert_eq!(man("manifest.json")["version"], mv, "версия списка модов не меняется от правки настроек");
        assert_ne!(man("defaults.json")["version"], dv);
        assert_eq!(man("defaults.json")["version"], "1.0.1");
    }

    #[test]
    fn legacy_manifest_with_defaults_group_is_migrated() {
        let d = repo();
        std::fs::write(d.path().join("manifest.json"), r#"{"version":"1.2.7","groups":{"mods":[],"defaults":[{"path":"options.txt","sha1":"x"}]}}"#).unwrap();
        let r = run(d.path(), |_| {});
        assert!(r.notes.iter().any(|n| n.contains("defaults.json")), "{:?}", r.notes);
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        assert!(m["groups"].get("defaults").is_none());
        assert!(d.path().join("defaults.json").is_file());
        assert_eq!(m["version"], "1.2.8");
        assert!(!run(d.path(), |_| {}).changed);
    }

    #[test]
    fn refuses_empty_mods_and_missing_installer() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("mods")).unwrap();
        let err = build(&Options { repo: d.path().to_path_buf(), ..Default::default() }).unwrap_err();
        assert!(err.contains("нет ни одного .jar"), "{err}");
        std::fs::write(d.path().join("mods/a.jar"), b"a").unwrap();
        let err = build(&Options { repo: d.path().to_path_buf(), loader: Some("nope-installer.jar".into()), ..Default::default() }).unwrap_err();
        assert!(err.contains("Не найден installer"), "{err}");
        // без installer — предупреждение, но манифест строится
        let r = build(&Options { repo: d.path().to_path_buf(), mc: Some("1.21.1".into()), ..Default::default() }).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("installer")));
    }

    #[test]
    fn check_detects_drift() {
        let d = repo();
        run(d.path(), |_| {});
        assert!(check(d.path()).unwrap().is_clean());
        std::fs::write(d.path().join("mods/B-mod.jar"), b"tampered").unwrap();
        std::fs::remove_file(d.path().join("config/c.toml")).unwrap();
        std::fs::write(d.path().join("mods/extra.jar"), b"e").unwrap();
        std::fs::write(d.path().join("neoforge-21.1.172-installer.jar"), b"swapped").unwrap();
        let c = check(d.path()).unwrap();
        assert_eq!(c.missing, vec!["config/c.toml"]);
        let mut mm = c.mismatched.clone();
        mm.sort();
        assert_eq!(mm, vec!["mods/B-mod.jar", "neoforge-21.1.172-installer.jar"]);
        assert_eq!(c.not_in_manifest, vec!["mods/extra.jar"]);
        assert!(!c.is_clean());
    }

    #[test]
    fn manifest_is_readable_by_launcher_format() {
        // Структура должна совпадать с тем, что читает vilauncher-core::sync::Manifest.
        let d = repo();
        run(d.path(), |_| {});
        let m: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("manifest.json")).unwrap()).unwrap();
        for k in ["version", "mcVersion", "loader", "groups"] {
            assert!(m.get(k).is_some(), "нет {k}");
        }
        for k in ["core", "mods"] {
            assert!(m["groups"][k].is_array(), "groups.{k}");
        }
        for k in ["type", "version", "installer", "sha1"] {
            assert!(m["loader"].get(k).is_some(), "loader.{k}");
        }
    }
}
