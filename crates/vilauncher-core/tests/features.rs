mod common;

use common::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vilauncher_core::downloader::build_client;
use vilauncher_core::fsutil::encode_component;
use vilauncher_core::{anticheat, modrinth, skins};

#[tokio::test]
async fn modrinth_search_and_versions_map_fields() {
    let facets = encode_component(&json!([["project_type:shader"], ["versions:1.21.1"]]).to_string());
    let search_path = format!("/search?query=bsl%20shaders&facets={facets}&limit=5");
    let versions_path = format!("/project/AbC12/version?game_versions={}", encode_component(&json!(["1.21.1"]).to_string()));
    let hits = json!({"hits":[{"project_id":"AbC12","title":"BSL","description":"d","icon_url":"https://i/x.png","downloads":42,"author":"capttatsu","extra":1}]});
    let many: Vec<_> = (0..14).map(|i| json!({"id": format!("v{i}"), "version_number": format!("1.{i}"), "loaders": ["iris"],
        "files": [{"url": "https://cdn/x.zip", "filename": "x.zip", "size": 10, "primary": true}]})).collect();
    let srv = start(HashMap::from([(search_path, hits.to_string().into_bytes()), (versions_path, json!(many).to_string().into_bytes())]), HashMap::new()).await;
    let c = build_client("t").unwrap();

    let found = modrinth::search(&c, &srv.base(), "bsl shaders", "shader", Some(5), "1.21.1").await.unwrap();
    assert_eq!(found.len(), 1);
    let j = serde_json::to_value(&found[0]).unwrap();
    assert_eq!((j["id"].as_str(), j["title"].as_str(), j["iconUrl"].as_str(), j["downloads"].as_u64(), j["author"].as_str()), (Some("AbC12"), Some("BSL"), Some("https://i/x.png"), Some(42), Some("capttatsu")));
    assert!(j.get("extra").is_none());

    let v = modrinth::versions(&c, &srv.base(), "AbC12", "1.21.1").await.unwrap();
    assert_eq!(v.len(), 10, "первые 10 версий");
    let j = serde_json::to_value(&v[0]).unwrap();
    assert_eq!(j["versionName"], "1.0");
    assert_eq!(j["files"][0]["filename"], "x.zip");

    // HTTP-ошибка → Err("HTTP 404")
    let err = modrinth::versions(&c, &srv.base(), "Missing", "1.21.1").await.unwrap_err();
    assert_eq!(err.to_string(), "HTTP 404");
}

#[tokio::test]
async fn pack_download_is_safe_and_reports_progress() {
    let srv = start(HashMap::from([("/p/pack.zip".to_string(), vec![7u8; 50_000])]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared/shaderpacks");
    let c = build_client("t").unwrap();

    let pcts = Arc::new(Mutex::new(Vec::<u8>::new()));
    let p2 = pcts.clone();
    // https обязателен: mock — http, поэтому такой URL отвергается
    let err = modrinth::download_pack(&c, &srv.url("/p/pack.zip"), &shared, "pack.zip", &CancelToken::new(), |_| {}).await.unwrap_err();
    assert!(err.to_string().contains("https"), "{err}");
    for bad in ["../evil.zip", "a/b.zip", "..\\x.zip", ""] {
        assert!(modrinth::download_pack(&c, "https://x/y.zip", &shared, bad, &CancelToken::new(), |_| {}).await.is_err(), "{bad}");
    }
    assert!(!dir.path().join("evil.zip").exists());

    // Сам движок скачивания проверяем через публичный API на http-источнике (в проде URL https).
    use vilauncher_core::downloader::*;
    let plan = DeltaPlan {
        to_download: vec![PlannedFile { file: RemoteFile { path: "pack.zip".into(), url: srv.url("/p/pack.zip"), ..Default::default() }, dest: shared.join("pack.zip"), reason: DownloadReason::Missing, tag: 0 }],
        up_to_date: 0,
    };
    std::fs::create_dir_all(&shared).unwrap();
    let cb: ProgressFn = Arc::new(move |e| p2.lock().unwrap().push(e.file_pct));
    let sum = download_all(&c, &plan, &DownloadOptions::default(), &CancelToken::new(), Some(cb)).await.unwrap();
    assert_eq!(sum.downloaded, 1);
    assert_eq!(pcts.lock().unwrap().last().copied(), Some(100));
    assert_eq!(modrinth::list_installed(&shared).await, vec!["pack.zip"]);
}

#[tokio::test]
async fn blocklist_and_skin_fetch_over_http() {
    let png = {
        let mut b = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        b.extend([1, 2, 3, 4]);
        b
    };
    let srv = start(
        HashMap::from([
            ("/blocklist.json".to_string(), br#"{"_version":2,"mods":["xray"],"resourcepacks":[]}"#.to_vec()),
            ("/skin/Notch".to_string(), png.clone()),
        ]),
        HashMap::new(),
    )
    .await;
    let c = build_client("t").unwrap();
    let bl = anticheat::load_blocklist(&c, &srv.base()).await.unwrap();
    assert_eq!(bl.mods, vec!["xray"]);
    assert!(anticheat::load_blocklist(&c, "http://127.0.0.1:1").await.is_err());

    let url = skins::fetch_skin(&c, &format!("{}/skin/", srv.base()), "Notch").await.unwrap();
    assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
    assert!(skins::fetch_skin(&c, &format!("{}/skin/", srv.base()), "Nobody").await.is_none(), "404 → скин не задан");
}
