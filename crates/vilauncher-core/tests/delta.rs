mod common;

use common::*;
use std::collections::HashMap;
use vilauncher_core::downloader::*;
use vilauncher_core::sync::*;
use vilauncher_core::CoreError;

fn rf(path: &str, url: String, body: &[u8]) -> RemoteFile {
    RemoteFile { path: path.into(), url, sha1: Some(sha1_hex(body)), ..Default::default() }
}

#[tokio::test]
async fn downloads_only_missing_and_changed_files() {
    let (a, b, c) = (b"AAAA-jar".to_vec(), b"BBBB-jar-new".to_vec(), b"CCCC-jar".to_vec());
    let mut routes = HashMap::new();
    routes.insert("/a.jar".to_string(), a.clone());
    routes.insert("/b.jar".to_string(), b.clone());
    routes.insert("/c.jar".to_string(), c.clone());
    let srv = start(routes, HashMap::new()).await;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mods")).unwrap();
    std::fs::write(dir.path().join("mods/a.jar"), &a).unwrap(); // актуален
    std::fs::write(dir.path().join("mods/b.jar"), b"OLD").unwrap(); // изменён

    let files = vec![
        rf("mods/a.jar", srv.url("/a.jar"), &a),
        rf("mods/b.jar", srv.url("/b.jar"), &b),
        rf("mods/c.jar", srv.url("/c.jar"), &c), // отсутствует
    ];
    let plan = plan_delta(dir.path(), &files, SyncPolicy::Strict, 1, 4).await.unwrap();
    assert_eq!(plan.up_to_date, 1);
    assert_eq!(plan.to_download.len(), 2);
    let reasons: HashMap<_, _> = plan.to_download.iter().map(|p| (p.file.path.as_str(), p.reason)).collect();
    assert_eq!(reasons["mods/b.jar"], DownloadReason::HashMismatch);
    assert_eq!(reasons["mods/c.jar"], DownloadReason::Missing);

    let client = build_client("test").unwrap();
    let sum = download_all(&client, &plan, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(sum.downloaded, 2);
    assert!(sum.failed.is_empty());

    assert_eq!(srv.hits("/a.jar"), 0, "актуальный файл не должен скачиваться повторно");
    assert_eq!(srv.hits("/b.jar"), 1);
    assert_eq!(srv.hits("/c.jar"), 1);
    assert_eq!(std::fs::read(dir.path().join("mods/b.jar")).unwrap(), b);
    assert_eq!(std::fs::read(dir.path().join("mods/c.jar")).unwrap(), c);
    assert!(!dir.path().join("mods/b.jar.part").exists());

    // Повторный план — всё актуально.
    let plan2 = plan_delta(dir.path(), &files, SyncPolicy::Strict, 1, 4).await.unwrap();
    assert!(plan2.is_empty());
    assert_eq!(plan2.up_to_date, 3);
}

#[tokio::test]
async fn md5_and_size_are_supported() {
    let body = b"md5-body".to_vec();
    let srv = start(HashMap::from([("/m.bin".to_string(), body.clone())]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let f = RemoteFile { path: "res/m.bin".into(), url: srv.url("/m.bin"), md5: Some(md5_hex(&body).to_uppercase()), size: Some(body.len() as u64), ..Default::default() };
    let plan = plan_delta(dir.path(), &[f.clone()], SyncPolicy::Strict, 0, 2).await.unwrap();
    let client = build_client("test").unwrap();
    let sum = download_all(&client, &plan, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(sum.downloaded, 1);
    // Размер не совпал → перекачка, даже без проверки хеша.
    std::fs::write(dir.path().join("res/m.bin"), b"short").unwrap();
    let plan = plan_delta(dir.path(), &[f], SyncPolicy::Strict, 0, 2).await.unwrap();
    assert_eq!(plan.to_download[0].reason, DownloadReason::SizeMismatch);
}

#[tokio::test]
async fn corrupted_download_is_rejected_and_never_lands_on_disk() {
    let srv = start(HashMap::from([("/x.jar".to_string(), b"EVIL".to_vec())]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let f = rf("mods/x.jar", srv.url("/x.jar"), b"GOOD"); // хеш от другого содержимого
    let plan = plan_delta(dir.path(), &[f], SyncPolicy::Strict, 1, 2).await.unwrap();
    let client = build_client("test").unwrap();
    let opts = DownloadOptions { retries: 2, ..fast_opts() };
    let sum = download_all(&client, &plan, &opts, &CancelToken::new(), None).await.unwrap();
    assert_eq!(sum.failed.len(), 1);
    assert!(sum.failed[0].error.contains("Хеш не совпал"), "{}", sum.failed[0].error);
    assert_eq!(srv.hits("/x.jar"), 3, "1 попытка + 2 повтора");
    assert!(!dir.path().join("mods/x.jar").exists());
    assert!(!dir.path().join("mods/x.jar.part").exists());
}

#[tokio::test]
async fn retries_on_server_errors_then_succeeds() {
    let body = b"flaky".to_vec();
    let srv = start(HashMap::from([("/f.jar".to_string(), body.clone())]), HashMap::from([("/f.jar".to_string(), 2)])).await;
    let dir = tempfile::tempdir().unwrap();
    let plan = plan_delta(dir.path(), &[rf("mods/f.jar", srv.url("/f.jar"), &body)], SyncPolicy::Strict, 1, 2).await.unwrap();
    let client = build_client("test").unwrap();
    let sum = download_all(&client, &plan, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(sum.downloaded, 1);
    assert_eq!(srv.hits("/f.jar"), 3);
}

#[tokio::test]
async fn http_404_is_not_retried() {
    let srv = start(HashMap::new(), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let plan = plan_delta(dir.path(), &[rf("mods/g.jar", srv.url("/g.jar"), b"x")], SyncPolicy::Strict, 1, 2).await.unwrap();
    let client = build_client("test").unwrap();
    let sum = download_all(&client, &plan, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(sum.failed.len(), 1);
    assert_eq!(srv.hits("/g.jar"), 1);
}

#[tokio::test]
async fn parallel_download_of_many_files_with_progress() {
    let mut routes = HashMap::new();
    let mut files = Vec::new();
    for i in 0..40 {
        let body = vec![i as u8; 20_000 + i * 100];
        routes.insert(format!("/f{i}.bin"), body.clone());
        let mut f = rf(&format!("mods/f{i}.bin"), String::new(), &body);
        f.size = Some(body.len() as u64);
        files.push((f, body));
    }
    let srv = start(routes, HashMap::new()).await;
    let files: Vec<RemoteFile> = files.into_iter().enumerate().map(|(i, (mut f, _))| { f.url = srv.url(&format!("/f{i}.bin")); f }).collect();

    let dir = tempfile::tempdir().unwrap();
    let plan = plan_delta(dir.path(), &files, SyncPolicy::Strict, 1, 8).await.unwrap();
    assert_eq!(plan.to_download.len(), 40);

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ProgressEvent>::new()));
    let ev2 = events.clone();
    let cb: ProgressFn = std::sync::Arc::new(move |e| ev2.lock().unwrap().push(e));
    let client = build_client("test").unwrap();
    let sum = download_all(&client, &plan, &DownloadOptions { concurrency: 8, ..fast_opts() }, &CancelToken::new(), Some(cb)).await.unwrap();
    assert_eq!(sum.downloaded, 40);

    let ev = events.lock().unwrap();
    assert_eq!(ev.iter().filter(|e| e.done).count(), 40);
    assert_eq!(ev.last().unwrap().completed, 40);
    assert_eq!(ev.iter().map(|e| e.overall_done).max().unwrap(), plan.total_known_bytes());
}

#[tokio::test]
async fn lazy_policy_keeps_user_files() {
    let srv = start(HashMap::from([("/o.txt".to_string(), b"default".to_vec())]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("options.txt"), b"user-edited").unwrap();
    let files = vec![
        RemoteFile { path: "options.txt".into(), url: srv.url("/o.txt"), sha1: Some(sha1_hex(b"default")), ..Default::default() },
        RemoteFile { path: "config/new.txt".into(), url: srv.url("/o.txt"), ..Default::default() },
    ];
    let plan = plan_delta(dir.path(), &files, SyncPolicy::Lazy, 2, 2).await.unwrap();
    assert_eq!(plan.to_download.len(), 1);
    assert_eq!(plan.to_download[0].file.path, "config/new.txt");
    assert_eq!(std::fs::read(dir.path().join("options.txt")).unwrap(), b"user-edited");
}

#[tokio::test]
async fn path_traversal_in_manifest_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for evil in ["../evil.jar", "mods/../../evil.jar", "/etc/passwd", "C:\\Windows\\evil.dll"] {
        let f = RemoteFile { path: evil.into(), url: "http://127.0.0.1:1/x".into(), ..Default::default() };
        let err = plan_delta(dir.path(), &[f], SyncPolicy::Strict, 1, 2).await.unwrap_err();
        assert!(matches!(err, CoreError::UnsafePath(_)), "{evil}: {err:?}");
    }
}

#[tokio::test]
async fn full_sync_flow_with_groups_wipe_and_version_skip() {
    let (m1, m2, c1, d1) = (b"mod-one".to_vec(), b"mod-two".to_vec(), b"core-bin".to_vec(), b"default-cfg".to_vec());
    let manifest = serde_json::json!({
        "version": "1.2.6", "mcVersion": "1.20.1", "clientRevision": 2,
        "groups": {
            "core": [ { "path": "bin/core.dat" } ],
            "mods": [ { "path": "mods/one+x.jar" }, { "path": "mods/two.jar" } ],
            "defaults": [ { "path": "config/def.toml" } ]
        }
    });
    let hashes = serde_json::json!({
        "bin/core.dat": sha1_hex(&c1), "mods/one+x.jar": sha1_hex(&m1), "mods/two.jar": sha1_hex(&m2)
    });
    let mut routes = HashMap::new();
    routes.insert("/manifest.json".to_string(), manifest.to_string().into_bytes());
    routes.insert("/hashes.json".to_string(), hashes.to_string().into_bytes());
    routes.insert("/bin/core.dat".to_string(), c1.clone());
    routes.insert("/mods/one%2Bx.jar".to_string(), m1.clone()); // '+' должен уйти как %2B
    routes.insert("/mods/two.jar".to_string(), m2.clone());
    routes.insert("/config/def.toml".to_string(), d1.clone());
    let srv = start(routes, HashMap::new()).await;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mods")).unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("mods/cheat-client.jar"), b"hax").unwrap();
    std::fs::write(dir.path().join("mods/two.jar"), &m2).unwrap();
    std::fs::write(dir.path().join("bin/stale.dat"), b"old").unwrap(); // уйдёт при Smart Wipe (rev 1→2)

    let client = build_client("test").unwrap();
    let (manifest, hashes) = fetch_manifest(&client, &srv.base()).await.unwrap();
    let mut cfg = SyncConfig::new(dir.path(), srv.base());
    cfg.previous_version = Some("1.2.5".into());
    cfg.previous_client_revision = Some(1);

    let rep = run_sync(&client, &cfg, &manifest, &hashes, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert!(rep.updated);
    assert_eq!(rep.removed_mods, vec!["cheat-client.jar"]);
    assert_eq!(rep.downloaded, 3, "core + one+x.jar + def.toml; two.jar уже актуален");
    assert_eq!(rep.up_to_date, 1);
    assert!(!dir.path().join("bin/stale.dat").exists(), "Smart Wipe");
    assert!(!dir.path().join("mods/cheat-client.jar").exists());
    assert_eq!(std::fs::read(dir.path().join("mods/one+x.jar")).unwrap(), m1);
    assert_eq!(srv.hits("/mods/two.jar"), 0);

    // Та же версия → ничего не делаем и не ходим в сеть за файлами.
    let before = srv.total_hits();
    cfg.previous_version = Some(rep.version.clone());
    cfg.previous_client_revision = rep.client_revision;
    let rep2 = run_sync(&client, &cfg, &manifest, &hashes, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert!(!rep2.updated);
    assert_eq!(srv.total_hits(), before);

    // force=true: проверка хешей, но докачивать нечего.
    cfg.force = true;
    std::fs::write(dir.path().join("mods/two.jar"), b"tampered").unwrap();
    let rep3 = run_sync(&client, &cfg, &manifest, &hashes, &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(rep3.downloaded, 1);
    assert_eq!(std::fs::read(dir.path().join("mods/two.jar")).unwrap(), m2);
}

#[tokio::test]
async fn jars_missing_from_manifest_are_removed_like_in_electron_version() {
    let manifest: Manifest = serde_json::from_str(r#"{"version":"9","groups":{"mods":[]}}"#).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mods")).unwrap();
    std::fs::write(dir.path().join("mods/extra.jar"), b"k").unwrap();
    std::fs::write(dir.path().join("mods/readme.txt"), b"t").unwrap();
    let client = build_client("test").unwrap();
    let cfg = SyncConfig::new(dir.path(), "http://127.0.0.1:1");
    run_sync(&client, &cfg, &manifest, &HashMapFile::new(), &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert!(!dir.path().join("mods/extra.jar").exists());
    assert!(dir.path().join("mods/readme.txt").exists(), "не-jar файлы не трогаем");
}

#[tokio::test]
async fn failure_in_mods_group_is_fatal_but_defaults_is_not() {
    let srv = start(HashMap::new(), HashMap::new()).await; // всё отдаёт 404
    let dir = tempfile::tempdir().unwrap();
    let client = build_client("test").unwrap();

    let only_defaults: Manifest = serde_json::from_str(r#"{"version":"1","groups":{"defaults":[{"path":"config/a.toml"}]}}"#).unwrap();
    let cfg = SyncConfig::new(dir.path(), srv.base());
    let rep = run_sync(&client, &cfg, &only_defaults, &HashMapFile::new(), &fast_opts(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(rep.optional_failures.len(), 1);

    let with_mod: Manifest = serde_json::from_str(r#"{"version":"2","groups":{"mods":[{"path":"mods/a.jar"}]}}"#).unwrap();
    let err = run_sync(&client, &cfg, &with_mod, &HashMapFile::new(), &fast_opts(), &CancelToken::new(), None).await.unwrap_err();
    assert!(matches!(err, CoreError::SyncFailed { .. }), "{err:?}");
}

#[tokio::test]
async fn cancel_stops_download() {
    let srv = start(HashMap::from([("/c.bin".to_string(), vec![1u8; 1000])]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let plan = plan_delta(dir.path(), &[RemoteFile { path: "c.bin".into(), url: srv.url("/c.bin"), ..Default::default() }], SyncPolicy::Strict, 0, 1).await.unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let client = build_client("test").unwrap();
    let err = download_all(&client, &plan, &fast_opts(), &cancel, None).await.unwrap_err();
    assert!(matches!(err, CoreError::Cancelled));
    assert!(!dir.path().join("c.bin").exists());
}

#[tokio::test]
async fn manifest_generated_by_tools_script_is_understood() {
    // Формат, который пишет tools/gen_manifest.py: sha1/size в записях, раздел loader, числовая clientRevision.
    let text = r#"{
      "version": "2.0.0", "mcVersion": "1.21.1", "clientRevision": 2,
      "loader": { "type": "neoforge", "version": "21.1.172", "installer": "neoforge-21.1.172-installer.jar", "sha1": "abc" },
      "groups": {
        "core": [],
        "mods": [ { "path": "mods/a.jar", "sha1": "da39a3ee5e6b4b0d3255bfef95601890afd80709", "size": 0 } ],
        "defaults": [ { "path": "options.txt", "sha1": "x", "size": 1 } ]
      }
    }"#;
    let m: Manifest = serde_json::from_str(text).unwrap();
    assert_eq!(m.version, "2.0.0");
    assert_eq!(m.mc_version.as_deref(), Some("1.21.1"));
    assert_eq!(m.client_revision, Some(2));
    let l = m.loader.unwrap();
    assert_eq!((l.installer.as_str(), l.version.as_deref(), l.sha1.as_deref()), ("neoforge-21.1.172-installer.jar", Some("21.1.172"), Some("abc")));
    assert_eq!(m.groups.mods[0].size, Some(0));
    // манифест без loader (старый формат) тоже читается
    let old: Manifest = serde_json::from_str(r#"{"version":"1","groups":{}}"#).unwrap();
    assert!(old.loader.is_none());
}
