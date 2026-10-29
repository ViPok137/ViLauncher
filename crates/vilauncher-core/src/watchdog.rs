//! Периодическая проверка во время игры: чёрный список читов + целостность списка модов.
//!
//! Вызывается раз в минуту. Полный пересчёт хешей делается только для файлов, у которых изменились размер
//! или время правки (или которые появились после запуска), поэтому проверка почти бесплатна.

use crate::anticheat::{scan_for_cheats, Blocklist};
use crate::hash::{hash_eq, hash_file, HashAlgo};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ViolationKind {
    /// Имя файла содержит ключевое слово из чёрного манифеста.
    Cheat,
    /// `.jar` в mods/, которого нет в манифесте.
    Unauthorized,
    /// Мод из манифеста, но содержимое отличается от эталонного хеша.
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub kind: ViolationKind,
    pub name: String,
}

#[derive(Default, Debug, Clone)]
pub struct Snapshot {
    files: HashMap<String, (u64, Option<SystemTime>)>,
}

async fn list_jars(mods_dir: &Path) -> Vec<(String, u64, Option<SystemTime>)> {
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(mods_dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jar") || !e.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if let Ok(m) = e.metadata().await {
                out.push((name, m.len(), m.modified().ok()));
            }
        }
    }
    out.sort();
    out
}

/// Снимок mods/ сразу после успешной проверки перед запуском.
pub async fn build_snapshot(mods_dir: &Path) -> Snapshot {
    Snapshot { files: list_jars(mods_dir).await.into_iter().map(|(n, s, m)| (n, (s, m))).collect() }
}

/// Один проход проверки. `allowed` — имя мода → SHA-1 из манифеста (`None` — манифест недоступен,
/// проверка целостности пропускается, остаётся только чёрный список).
pub async fn check_tick(client_dir: &Path, allowed: Option<&HashMap<String, String>>, snapshot: &mut Snapshot, blocklist: Option<&Blocklist>) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();

    if let Some(bl) = blocklist {
        for t in scan_for_cheats(client_dir, bl).await {
            out.push(Violation { kind: ViolationKind::Cheat, name: t.name });
        }
    }

    if let Some(allowed) = allowed {
        for (name, size, mtime) in list_jars(&client_dir.join("mods")).await {
            let Some(want) = allowed.get(&name) else {
                out.push(Violation { kind: ViolationKind::Unauthorized, name });
                continue;
            };
            if snapshot.files.get(&name) == Some(&(size, mtime)) {
                continue; // не менялся с момента проверки
            }
            if want.is_empty() {
                snapshot.files.insert(name, (size, mtime)); // хеша в манифесте нет — проверить нечем
                continue;
            }
            match hash_file(client_dir.join("mods").join(&name), HashAlgo::Sha1).await {
                Ok(h) if hash_eq(&h, want) => {
                    snapshot.files.insert(name, (size, mtime)); // только время правки, содержимое то же
                }
                Ok(_) => out.push(Violation { kind: ViolationKind::Modified, name }),
                Err(_) => {} // файл заблокирован/исчез между чтениями — решим на следующем проходе
            }
        }
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::Digest;

    fn sha(b: &[u8]) -> String {
        hex::encode(sha1::Sha1::digest(b))
    }

    #[tokio::test]
    async fn detects_unauthorized_modified_and_cheats() {
        let d = tempfile::tempdir().unwrap();
        let mods = d.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("create.jar"), b"create").unwrap();
        std::fs::write(mods.join("sodium.jar"), b"sodium").unwrap();
        let allowed: HashMap<String, String> = [("create.jar", sha(b"create")), ("sodium.jar", sha(b"sodium")), ("nohash.jar", String::new())].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let bl: Blocklist = serde_json::from_str(r#"{"mods":["killaura"]}"#).unwrap();
        let mut snap = build_snapshot(&mods).await;

        // чисто
        assert!(check_tick(d.path(), Some(&allowed), &mut snap, Some(&bl)).await.is_empty());

        // только время правки изменилось, содержимое то же — не нарушение
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(mods.join("create.jar"), b"create").unwrap();
        assert!(check_tick(d.path(), Some(&allowed), &mut snap, Some(&bl)).await.is_empty());

        // подмена мода, неизвестный jar и чит по ключевому слову
        std::fs::write(mods.join("sodium.jar"), b"sodium-with-cheat").unwrap();
        std::fs::write(mods.join("extra.jar"), b"x").unwrap();
        std::fs::write(mods.join("my-KillAura.jar"), b"x").unwrap();
        let v = check_tick(d.path(), Some(&allowed), &mut snap, Some(&bl)).await;
        let has = |k, n: &str| v.iter().any(|x| x.kind == k && x.name == n);
        assert!(has(ViolationKind::Modified, "sodium.jar"), "{v:?}");
        assert!(has(ViolationKind::Unauthorized, "extra.jar"), "{v:?}");
        assert!(has(ViolationKind::Cheat, "my-KillAura.jar"), "{v:?}");
        assert!(has(ViolationKind::Unauthorized, "my-KillAura.jar"), "{v:?}");
        assert!(!v.iter().any(|x| x.name == "create.jar"));
    }

    #[tokio::test]
    async fn without_manifest_only_blocklist_is_checked() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("mods")).unwrap();
        std::fs::write(d.path().join("mods/anything.jar"), b"x").unwrap();
        let mut snap = Snapshot::default();
        assert!(check_tick(d.path(), None, &mut snap, None).await.is_empty());
        std::fs::write(d.path().join("mods/wurst.jar"), b"x").unwrap();
        let bl: Blocklist = serde_json::from_str(r#"{"mods":["wurst"]}"#).unwrap();
        let v = check_tick(d.path(), None, &mut snap, Some(&bl)).await;
        assert_eq!(v, vec![Violation { kind: ViolationKind::Cheat, name: "wurst.jar".into() }]);
    }

    #[tokio::test]
    async fn deleted_mods_and_missing_dir_are_not_violations() {
        let d = tempfile::tempdir().unwrap();
        let allowed: HashMap<String, String> = [("a.jar".to_string(), sha(b"a"))].into_iter().collect();
        let mut snap = Snapshot::default();
        assert!(check_tick(d.path(), Some(&allowed), &mut snap, None).await.is_empty());
    }
}
