//! Сквозная проверка: программа `vilauncher-manifest` → manifest.json → реальный код лаунчера.

mod common;

use common::*;
use std::collections::HashMap;
use vilauncher_core::downloader::{build_client, CancelToken};
use vilauncher_core::installer::LoaderSource;
use vilauncher_core::sync::*;

fn serve_repo(repo: &std::path::Path) -> HashMap<String, Vec<u8>> {
    let mut routes = HashMap::new();
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut HashMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                // GitHub/сервер принимают percent-кодированные пути (`+` → %2B, пробел → %20).
                out.insert(format!("/{}", vilauncher_core::fsutil::encode_url_path(&rel)), std::fs::read(&p).unwrap());
            }
        }
    }
    walk(repo, repo, &mut routes);
    routes
}

#[tokio::test]
async fn manifest_from_tool_drives_the_launcher_end_to_end() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    std::fs::create_dir_all(r.join("mods")).unwrap();
    std::fs::create_dir_all(r.join("config")).unwrap();
    std::fs::write(r.join("mods/create+neoforge-1.21.1.jar"), b"create-mod").unwrap(); // '+' в имени
    std::fs::write(r.join("mods/Sodium Extra.jar"), b"sodium-extra").unwrap(); // пробел в имени
    std::fs::write(r.join("config/client.toml"), b"[client]").unwrap();
    std::fs::write(r.join("options.txt"), b"fov:70").unwrap();
    std::fs::write(r.join("neoforge-21.1.172-installer.jar"), b"installer-bytes").unwrap();

    // 1. Программа строит манифесты: manifest.json (моды), defaults.json (настройки), blocklist.json (шаблон).
    let rep = vilauncher_manifest::build(&vilauncher_manifest::Options { repo: r.to_path_buf(), ..Default::default() }).unwrap();
    vilauncher_manifest::write(r, &rep).unwrap();
    assert!(vilauncher_manifest::check(r).unwrap().is_clean());
    assert!(r.join("defaults.json").is_file() && r.join("blocklist.json").is_file());

    // 2. «GitHub»: отдаёт все файлы репозитория.
    let srv = start(serve_repo(r), HashMap::new()).await;
    let client = build_client("t").unwrap();

    // 3. Лаунчер читает манифест и синхронизирует чистую папку клиента — только МОДЫ.
    let (manifest, hashes) = fetch_manifest(&client, &srv.base()).await.unwrap();
    assert_eq!(manifest.mc_version.as_deref(), Some("1.21.1"));
    assert_eq!(manifest.groups.mods.len(), 2);
    assert!(manifest.groups.defaults.is_empty(), "настройки больше не в manifest.json");
    let dir = tempfile::tempdir().unwrap();
    let cfg = SyncConfig::new(dir.path(), srv.base());
    let report = run_sync(&client, &cfg, &manifest, &hashes, &Default::default(), &CancelToken::new(), None).await.unwrap();
    assert!(report.updated);
    assert_eq!(report.downloaded, 2, "только моды");
    assert_eq!(std::fs::read(dir.path().join("mods/create+neoforge-1.21.1.jar")).unwrap(), b"create-mod");
    assert_eq!(std::fs::read(dir.path().join("mods/Sodium Extra.jar")).unwrap(), b"sodium-extra");
    assert!(!dir.path().join("options.txt").exists(), "настройки ставятся отдельным шагом");

    // 4. Настройки по умолчанию: только отсутствующие файлы.
    let defaults = fetch_defaults(&client, &srv.base(), &manifest).await.unwrap();
    assert_eq!(defaults.len(), 2);
    let lazy = |only: Option<&'static str>, reset: bool| {
        let (client, base, dir, defaults, hashes) = (client.clone(), srv.base(), dir.path().to_path_buf(), defaults.clone(), hashes.clone());
        async move { sync_defaults(&client, &base, &dir, &defaults, &hashes, reset, only, &Default::default(), &CancelToken::new(), None).await.unwrap() }
    };
    let d1 = lazy(None, false).await;
    assert_eq!((d1.downloaded, d1.failed.len()), (2, 0));
    assert_eq!(std::fs::read(dir.path().join("options.txt")).unwrap(), b"fov:70");
    // игрок изменил настройки → при обычном запуске они сохраняются
    std::fs::write(dir.path().join("options.txt"), b"fov:0.9").unwrap();
    std::fs::write(dir.path().join("config/client.toml"), b"[client]\nmine = true").unwrap();
    let d2 = lazy(None, false).await;
    assert_eq!((d2.downloaded, d2.up_to_date), (0, 2));
    assert_eq!(std::fs::read(dir.path().join("options.txt")).unwrap(), b"fov:0.9", "правки игрока целы");
    // файл удалён → восстанавливается
    std::fs::remove_file(dir.path().join("config/client.toml")).unwrap();
    assert_eq!(lazy(None, false).await.downloaded, 1);
    // «Вернуть по умолчанию» для одного файла
    std::fs::write(dir.path().join("config/client.toml"), b"junk").unwrap();
    let one = lazy(Some("config/client.toml"), true).await;
    assert_eq!(one.downloaded, 1);
    assert_eq!(std::fs::read(dir.path().join("config/client.toml")).unwrap(), b"[client]");
    assert_eq!(std::fs::read(dir.path().join("options.txt")).unwrap(), b"fov:0.9", "остальное не тронуто");
    // «Вернуть по умолчанию» для всех
    let all = lazy(None, true).await;
    assert_eq!(all.downloaded, 2);
    assert_eq!(std::fs::read(dir.path().join("options.txt")).unwrap(), b"fov:70");
    // файла нет в defaults.json → понятная ошибка
    let err = sync_defaults(&client, &srv.base(), dir.path(), &defaults, &hashes, true, Some("config/none.toml"), &Default::default(), &CancelToken::new(), None).await.unwrap_err();
    assert!(err.to_string().contains("нет в настройках по умолчанию"), "{err}");

    // 5. Раздел loader даёт рабочий источник installer'а с верным SHA-1.
    let loader = LoaderSource::from_manifest(manifest.loader.as_ref().unwrap(), &srv.base()).unwrap();
    assert_eq!(loader.version, "21.1.172");
    assert_eq!(loader.installer_url, srv.url("/neoforge-21.1.172-installer.jar"));
    assert_eq!(loader.installer_sha1.as_deref(), Some(sha1_hex(b"installer-bytes").as_str()));
    assert_eq!(srv.hits("/neoforge-21.1.172-installer.jar"), 0);

    // 6. Повторная синхронизация той же версии ничего не качает.
    let mut cfg2 = SyncConfig::new(dir.path(), srv.base());
    cfg2.previous_version = Some(report.version.clone());
    cfg2.previous_client_revision = report.client_revision;
    let before = srv.total_hits();
    let rep2 = run_sync(&client, &cfg2, &manifest, &hashes, &Default::default(), &CancelToken::new(), None).await.unwrap();
    assert!(!rep2.updated);
    assert_eq!(srv.total_hits(), before);
}

#[tokio::test]
async fn play_time_verification_repairs_mods_and_works_offline_from_cache() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    std::fs::create_dir_all(r.join("mods")).unwrap();
    std::fs::write(r.join("mods/a.jar"), b"mod-a").unwrap();
    std::fs::write(r.join("mods/b.jar"), b"mod-b").unwrap();
    std::fs::write(r.join("neoforge-21.1.172-installer.jar"), b"i").unwrap();
    let rep = vilauncher_manifest::build(&vilauncher_manifest::Options { repo: r.to_path_buf(), ..Default::default() }).unwrap();
    vilauncher_manifest::write(r, &rep).unwrap();
    let srv = start(serve_repo(r), HashMap::new()).await;
    let client = build_client("t").unwrap();
    let (manifest, hashes) = fetch_manifest(&client, &srv.base()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mods")).unwrap();
    std::fs::write(dir.path().join("mods/a.jar"), b"mod-a").unwrap();
    std::fs::write(dir.path().join("mods/b.jar"), b"HACKED").unwrap(); // подмена
    std::fs::write(dir.path().join("mods/cheat.jar"), b"x").unwrap(); // лишний мод

    let v = verify_mods(&client, &srv.base(), dir.path(), &manifest, &hashes, true, &Default::default(), &CancelToken::new(), None).await.unwrap();
    assert_eq!((v.downloaded, v.up_to_date), (1, 1));
    assert_eq!(v.removed_mods, vec!["cheat.jar"]);
    assert_eq!(std::fs::read(dir.path().join("mods/b.jar")).unwrap(), b"mod-b", "подмена откачена");
    assert_eq!(srv.hits("/mods/a.jar"), 0, "целые моды не качаются");

    // кэш: без сети проверка идёт по последнему известному манифесту
    save_cache(dir.path(), &manifest, &hashes, &[]).await;
    let (m2, h2, d2) = load_cache(dir.path()).await.expect("кэш");
    assert_eq!((m2.version.as_str(), m2.groups.mods.len(), d2.len()), (manifest.version.as_str(), 2, 0));
    assert_eq!(m2.loader.as_ref().unwrap().installer, "neoforge-21.1.172-installer.jar");
    std::fs::write(dir.path().join("mods/a.jar"), b"HACKED-AGAIN").unwrap();
    // сервер недоступен, но мод из кэша не скачать → ошибка проверки (запуск будет заблокирован)
    let err = verify_mods(&client, "http://127.0.0.1:1", dir.path(), &m2, &h2, true, &Default::default(), &CancelToken::new(), None).await.unwrap_err();
    assert!(matches!(err, vilauncher_core::CoreError::SyncFailed { .. }), "{err:?}");
    // allowed_mods для сторожа
    let allowed = allowed_mods(&m2, &h2);
    assert_eq!(allowed.get("a.jar").map(String::as_str), Some(sha1_hex(b"mod-a").as_str()));
    assert_eq!(allowed.len(), 2);
}

#[tokio::test]
async fn updating_a_mod_in_repo_updates_only_that_mod_on_the_client() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    std::fs::create_dir_all(r.join("mods")).unwrap();
    std::fs::write(r.join("mods/a.jar"), b"a-v1").unwrap();
    std::fs::write(r.join("mods/b.jar"), b"b-v1").unwrap();
    std::fs::write(r.join("neoforge-21.1.172-installer.jar"), b"i").unwrap();
    let build = || {
        let rep = vilauncher_manifest::build(&vilauncher_manifest::Options { repo: r.to_path_buf(), ..Default::default() }).unwrap();
        vilauncher_manifest::write(r, &rep).unwrap();
    };
    build();
    let client = build_client("t").unwrap();
    let dir = tempfile::tempdir().unwrap();

    let srv1 = start(serve_repo(r), HashMap::new()).await;
    let (m1, h1) = fetch_manifest(&client, &srv1.base()).await.unwrap();
    let rep1 = run_sync(&client, &SyncConfig::new(dir.path(), srv1.base()), &m1, &h1, &Default::default(), &CancelToken::new(), None).await.unwrap();

    // Вы обновили один мод и перегенерировали манифест → версия выросла сама.
    std::fs::write(r.join("mods/b.jar"), b"b-v2").unwrap();
    build();
    let srv2 = start(serve_repo(r), HashMap::new()).await;
    let (m2, h2) = fetch_manifest(&client, &srv2.base()).await.unwrap();
    assert_ne!(m1.version, m2.version);
    let mut cfg = SyncConfig::new(dir.path(), srv2.base());
    cfg.previous_version = Some(rep1.version);
    let rep2 = run_sync(&client, &cfg, &m2, &h2, &Default::default(), &CancelToken::new(), None).await.unwrap();
    assert_eq!((rep2.updated, rep2.downloaded, rep2.up_to_date), (true, 1, 1));
    assert_eq!(srv2.hits("/mods/a.jar"), 0, "неизменённый мод не качается");
    assert_eq!(std::fs::read(dir.path().join("mods/b.jar")).unwrap(), b"b-v2");
}
