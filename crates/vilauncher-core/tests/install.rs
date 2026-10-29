#![cfg(unix)] // фейковая JRE — shell-скрипт

mod common;

use common::*;
use flate2::{write::GzEncoder, Compression};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use vilauncher_core::downloader::{build_client, CancelToken};
use vilauncher_core::installer::*;
use vilauncher_core::java_runtime::JavaRuntime;
use vilauncher_core::launch::LaunchJson;

const MC: &str = "1.21.1";

/// Фейковая java: `-version` → 21; `-jar <installer> --installClient <dir>` → как настоящий NeoForge
/// создаёт versions/neoforge-<ver>/…json и universal-jar. Версия берётся из имени installer-jar.
fn fake_java(loader_ok: bool, ver: &str) -> Vec<u8> {
    let loader_part = if loader_ok {
        r#"
echo "Considering library net.neoforged:neoforge"
echo "Downloading something.jar"
for a; do case "$a" in *-installer.jar) INST="$a";; esac; done
VER="$(basename "$INST" | sed -e 's/^neoforge-//' -e 's/-installer.jar$//')"
ID="neoforge-$VER"
mkdir -p "$LAST/versions/$ID" "$LAST/libraries/net/neoforged/neoforge/$VER"
echo x > "$LAST/libraries/net/neoforged/neoforge/$VER/neoforge-$VER-universal.jar"
cat > "$LAST/versions/$ID/$ID.json" <<JSON
{
  "id": "$ID",
  "inheritsFrom": "1.21.1",
  "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
  "libraries": [ { "name": "net.neoforged:neoforge:$VER:universal" } ],
  "arguments": {
    "jvm": [ "-DlibraryDirectory=\${library_directory}", "-p",
             "\${library_directory}/cpw/mods/bootstraplauncher/2.0.2/b.jar\${classpath_separator}\${library_directory}/cpw/mods/securejarhandler/3.0.8/s.jar",
             "--add-modules", "ALL-MODULE-PATH" ],
    "game": [ "--launchTarget", "neoforgeclient", "--fml.neoForgeVersion", "$VER", "--fml.mcVersion", "1.21.1" ]
  }
}
JSON
"#
    } else {
        "\necho 'ERROR: boom' >&2\nexit 3\n"
    };
    format!(
        r#"#!/bin/sh
if [ "$1" = "-version" ]; then
  echo 'openjdk version "{ver}" 2024-07-16' >&2
  exit 0
fi
for a; do LAST="$a"; done
{loader_part}"#
    )
    .into_bytes()
}

fn tar_with_java(script: Vec<u8>) -> Vec<u8> {
    let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut h = tar::Header::new_gnu();
    h.set_size(script.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    b.append_data(&mut h, "jdk-21.0.4+7-jre/bin/java", &script[..]).unwrap();
    b.into_inner().unwrap().finish().unwrap()
}

fn installer_zip(marker: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    z.start_file("install_profile.json", zip::write::FileOptions::default()).unwrap();
    z.write_all(marker.as_bytes()).unwrap();
    z.finish().unwrap().into_inner()
}

struct Fixture {
    srv: MockServer,
    sources: InstallSources,
    asset: Vec<u8>,
    zip_v172: Vec<u8>,
    zip_v173: Vec<u8>,
}

impl Fixture {
    fn target(&self, ver: &str, with_sha1: bool) -> InstallTarget {
        let zip = if ver == "21.1.172" { &self.zip_v172 } else { &self.zip_v173 };
        InstallTarget {
            mc_version: MC.into(),
            loader: Some(LoaderSource {
                version: ver.into(),
                installer_url: self.srv.url(&format!("/neoforge-{ver}-installer.jar")),
                installer_sha1: with_sha1.then(|| sha1_hex(zip)),
            }),
        }
    }
}

async fn fixture() -> Fixture {
    let (client_bytes, lib_bytes, asset) = (b"CLIENT-JAR-BYTES".to_vec(), b"LIB-BYTES".to_vec(), b"ASSET-PNG".to_vec());
    let asset_hash = sha1_hex(&asset);
    let idx_json = format!(r#"{{"objects":{{"icons/a.png":{{"hash":"{asset_hash}","size":{}}}}}}}"#, asset.len());
    let version_json = format!(
        r#"{{"id":"1.21.1","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":21}},
        "downloads":{{"client":{{"url":"{{{{BASE}}}}/client.jar","sha1":"{}","size":{}}}}},
        "assetIndex":{{"id":"17","url":"{{{{BASE}}}}/idx.json","sha1":"{}","size":1}},
        "libraries":[
          {{"name":"org.x:lib:1.0","downloads":{{"artifact":{{"path":"org/x/lib/1.0/lib-1.0.jar","url":"{{{{BASE}}}}/lib.jar","sha1":"{}","size":{}}}}}}},
          {{"name":"only:mac:1","rules":[{{"action":"allow","os":{{"name":"osx"}}}}],"downloads":{{"artifact":{{"path":"only/mac/1/mac-1.jar","url":"{{{{BASE}}}}/mac.jar","sha1":"00","size":1}}}}}}
        ]}}"#,
        sha1_hex(&client_bytes), client_bytes.len(), sha1_hex(idx_json.as_bytes()), sha1_hex(&lib_bytes), lib_bytes.len()
    );
    let (zip_v172, zip_v173) = (installer_zip("v172"), installer_zip("v173-updated"));

    let routes = HashMap::from([
        ("/manifest".to_string(), br#"{"latest":{"release":"1.21.1","snapshot":"1.21.2-pre1"},"versions":[{"id":"1.20.1","url":"{{BASE}}/v1201.json"},{"id":"1.21.1","url":"{{BASE}}/v.json"}]}"#.to_vec()),
        ("/v1201.json".to_string(), version_json.replace(r#""id":"1.21.1""#, r#""id":"1.20.1""#).replace(r#""majorVersion":21"#, r#""majorVersion":17"#).into_bytes()),
        ("/v.json".to_string(), version_json.into_bytes()),
        ("/client.jar".to_string(), client_bytes),
        ("/lib.jar".to_string(), lib_bytes),
        ("/idx.json".to_string(), idx_json.into_bytes()),
        (format!("/res/{}/{}", &asset_hash[..2], asset_hash), asset.clone()),
        ("/neoforge-21.1.172-installer.jar".to_string(), zip_v172.clone()),
        ("/neoforge-21.1.173-installer.jar".to_string(), zip_v173.clone()),
        ("/corrupt-installer.jar".to_string(), b"this is not a zip".to_vec()),
    ]);
    let srv = start(routes, HashMap::new()).await;
    let sources = InstallSources { mojang_manifest: srv.url("/manifest"), resources_base: srv.url("/res") };
    Fixture { srv, sources, asset, zip_v172, zip_v173 }
}

async fn runtime(dir: &std::path::Path, loader_ok: bool) -> JavaRuntime {
    runtime_major(dir, loader_ok, 21).await
}

async fn runtime_major(dir: &std::path::Path, loader_ok: bool, major: u32) -> JavaRuntime {
    let rt = JavaRuntime::with_major(dir, major);
    let ar = dir.join(format!("jre{major}.tar.gz"));
    std::fs::write(&ar, tar_with_java(fake_java(loader_ok, &format!("{major}.0.4")))).unwrap();
    rt.install_from_archive(&ar, "jre.tar.gz", None, &None).await.unwrap();
    rt
}

#[tokio::test]
async fn full_neoforge_install_pipeline_and_idempotent_rerun() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let client_dir = dir.path().join("client");
    let http = build_client("test").unwrap();

    // Остатки прежней установки (Forge 1.20.1): старый installer и версия не должны мешать.
    std::fs::create_dir_all(client_dir.join("versions/1.20.1-forge-47.4.16")).unwrap();
    std::fs::write(client_dir.join("versions/1.20.1-forge-47.4.16/1.20.1-forge-47.4.16.json"), r#"{"id":"1.20.1-forge-47.4.16","inheritsFrom":"1.20.1"}"#).unwrap();
    std::fs::write(client_dir.join("forge-1.20.1-47.4.16-installer.jar"), b"old").unwrap();

    let events = Arc::new(Mutex::new(Vec::<InstallProgress>::new()));
    let ev = events.clone();
    let cb: InstallProgressFn = Arc::new(move |p| ev.lock().unwrap().push(p));
    let target = fx.target("21.1.172", true);

    let launch = install(&http, dir.path(), &client_dir, &fx.sources, &target, &CancelToken::new(), Some(cb)).await.unwrap();

    // Minecraft 1.21.1
    assert_eq!(std::fs::read(client_dir.join("versions/1.21.1/1.21.1.jar")).unwrap(), b"CLIENT-JAR-BYTES");
    assert!(client_dir.join("versions/1.21.1/1.21.1.json").exists());
    assert_eq!(std::fs::read(client_dir.join("libraries/org/x/lib/1.0/lib-1.0.jar")).unwrap(), b"LIB-BYTES");
    assert!(!client_dir.join("libraries/only/mac/1/mac-1.jar").exists(), "правила ОС: mac-библиотека не качается");
    let h = sha1_hex(&fx.asset);
    assert_eq!(std::fs::read(client_dir.join(format!("assets/objects/{}/{h}", &h[..2]))).unwrap(), fx.asset);
    assert!(client_dir.join("assets/indexes/17.json").exists());

    // installer загрузчика — одним запросом, с GitHub-источника; старый installer удалён
    assert_eq!(fx.srv.hits("/neoforge-21.1.172-installer.jar"), 1);
    assert!(client_dir.join("neoforge-21.1.172-installer.jar").exists());
    assert!(!client_dir.join("forge-1.20.1-47.4.16-installer.jar").exists(), "installer прежней версии удалён");
    let log = std::fs::read_to_string(client_dir.join("forge-install.log")).unwrap();
    assert!(log.contains("=== STDOUT ===") && log.contains("Downloading something.jar"));

    // launch.json — читается тем же кодом, что запускает игру
    let parsed = LaunchJson::load(&client_dir.join("launch.json")).await.unwrap();
    assert_eq!(parsed.mc_version, "1.21.1");
    assert_eq!(parsed.install_key, "1.21.1|21.1.172");
    assert_eq!(parsed.main_class, "cpw.mods.bootstraplauncher.BootstrapLauncher");
    assert_eq!(parsed.asset_index, "17");
    assert_eq!(parsed.module_path.len(), 2);
    assert!(parsed.forge_game_args.windows(2).any(|w| w[0] == "--launchTarget" && w[1] == "neoforgeclient"));
    let names: Vec<String> = parsed.classpath.iter().map(|p| std::path::Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["lib-1.0.jar", "neoforge-21.1.172-universal.jar", "1.21.1.jar"]);
    assert_eq!(launch.forge_version_id, "neoforge-21.1.172", "версия Forge 1.20.1 проигнорирована");

    let ev = events.lock().unwrap();
    let phases: std::collections::HashSet<_> = ev.iter().map(|e| e.phase).collect();
    for p in ["java", "manifest", "client", "libraries", "assets", "forge", "done"] {
        assert!(phases.contains(p), "нет фазы {p}: {phases:?}");
    }
    assert_eq!(ev.last().unwrap().phase, "done");
    drop(ev);

    // Повторная установка: ничего тяжёлого не перекачивается, installer не качается (SHA-1 совпал).
    install(&http, dir.path(), &client_dir, &fx.sources, &target, &CancelToken::new(), None).await.unwrap();
    assert_eq!(fx.srv.hits("/client.jar"), 1);
    assert_eq!(fx.srv.hits("/lib.jar"), 1);
    assert_eq!(fx.srv.hits("/neoforge-21.1.172-installer.jar"), 1);
    assert_eq!(fx.srv.hits(&format!("/res/{}/{h}", &h[..2])), 1);
}

#[tokio::test]
async fn updating_loader_in_repo_reinstalls_it() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let client_dir = dir.path().join("client");
    let http = build_client("test").unwrap();

    let t1 = fx.target("21.1.172", true);
    install(&http, dir.path(), &client_dir, &fx.sources, &t1, &CancelToken::new(), None).await.unwrap();

    // Вы загрузили на GitHub новый installer и обновили loader в manifest.json.
    let t2 = fx.target("21.1.173", true);
    assert_ne!(t1.key(), t2.key());
    let launch = install(&http, dir.path(), &client_dir, &fx.sources, &t2, &CancelToken::new(), None).await.unwrap();

    assert_eq!(fx.srv.hits("/neoforge-21.1.173-installer.jar"), 1);
    assert!(client_dir.join("neoforge-21.1.173-installer.jar").exists());
    assert!(!client_dir.join("neoforge-21.1.172-installer.jar").exists(), "старый installer убран");
    assert_eq!(launch.forge_version_id, "neoforge-21.1.173");
    assert_eq!(launch.install_key, "1.21.1|21.1.173");
    assert_eq!(fx.srv.hits("/client.jar"), 1, "Minecraft не перекачивался");
}

#[tokio::test]
async fn same_file_name_but_new_content_is_redownloaded_by_sha1() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let client_dir = dir.path().join("client");
    let http = build_client("test").unwrap();
    let t = fx.target("21.1.172", true);
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    // локальный installer подменили/повредили — SHA-1 не совпадает → перекачка
    std::fs::write(client_dir.join("neoforge-21.1.172-installer.jar"), b"tampered").unwrap();
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    assert_eq!(fx.srv.hits("/neoforge-21.1.172-installer.jar"), 2);
    assert_eq!(std::fs::read(client_dir.join("neoforge-21.1.172-installer.jar")).unwrap(), fx.zip_v172);
}

#[tokio::test]
async fn without_sha1_a_valid_zip_is_kept() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let client_dir = dir.path().join("client");
    let http = build_client("test").unwrap();
    let t = fx.target("21.1.172", false);
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    assert_eq!(fx.srv.hits("/neoforge-21.1.172-installer.jar"), 1);
}

#[tokio::test]
async fn damaged_client_jar_is_redownloaded() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let client_dir = dir.path().join("client");
    let http = build_client("test").unwrap();
    let t = fx.target("21.1.172", true);
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    std::fs::write(client_dir.join("versions/1.21.1/1.21.1.jar"), b"broken").unwrap();
    install(&http, dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap();
    assert_eq!(std::fs::read(client_dir.join("versions/1.21.1/1.21.1.jar")).unwrap(), b"CLIENT-JAR-BYTES");
    assert_eq!(fx.srv.hits("/client.jar"), 2);
}

#[tokio::test]
async fn wrong_sha1_of_installer_is_rejected() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let mut t = fx.target("21.1.172", true);
    t.loader.as_mut().unwrap().installer_sha1 = Some("0".repeat(40)); // в manifest.json указан не тот хеш
    let err = install(&build_client("t").unwrap(), dir.path(), &dir.path().join("client"), &fx.sources, &t, &CancelToken::new(), None).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Не удалось скачать installer загрузчика (neoforge-21.1.172-installer.jar)"), "{msg}");
    assert!(msg.contains("Хеш не совпал") && msg.contains("loader.installer"), "{msg}");
    assert!(!dir.path().join("client/launch.json").exists());
}

#[tokio::test]
async fn missing_or_corrupt_installer_gives_clear_error_after_three_attempts() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), true).await;
    let http = build_client("t").unwrap();

    // файла нет в репозитории → 404 (сразу, без повторов HTTP), 3 попытки с паузами 2 с
    let mut t = fx.target("21.1.172", false);
    t.loader.as_mut().unwrap().installer_url = fx.srv.url("/not-uploaded-yet.jar");
    let started = std::time::Instant::now();
    let err = install(&http, dir.path(), &dir.path().join("c1"), &fx.sources, &t, &CancelToken::new(), None).await.unwrap_err();
    assert!(err.to_string().contains("после 3 попыток"), "{err}");
    assert!(started.elapsed().as_secs() >= 4);
    assert_eq!(fx.srv.hits("/not-uploaded-yet.jar"), 3);

    // не-ZIP вместо jar
    t.loader.as_mut().unwrap().installer_url = fx.srv.url("/corrupt-installer.jar");
    let err = install(&http, dir.path(), &dir.path().join("c2"), &fx.sources, &t, &CancelToken::new(), None).await.unwrap_err();
    assert!(err.to_string().contains("файл повреждён"), "{err}");
}

#[tokio::test]
async fn installer_failure_reports_exit_code_and_log_path() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), false).await;
    let client_dir = dir.path().join("client");
    let t = fx.target("21.1.172", true);
    let err = install(&build_client("t").unwrap(), dir.path(), &client_dir, &fx.sources, &t, &CancelToken::new(), None).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Installer загрузчика завершился с кодом 3"), "{msg}");
    assert!(msg.contains("ERROR: boom") && msg.contains("forge-install.log"), "{msg}");
    assert!(!client_dir.join("launch.json").exists());
}

fn vanilla_target(mc: &str) -> InstallTarget {
    InstallTarget { mc_version: mc.into(), loader: None }
}

#[tokio::test]
async fn vanilla_latest_installs_without_loader_or_mods() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let _rt = runtime(dir.path(), false).await; // «загрузчик» не нужен: даже сломанный installer не вызывается
    let client_dir = dir.path().join("instances/vanilla");
    let http = build_client("t").unwrap();

    let launch = install(&http, dir.path(), &client_dir, &fx.sources, &vanilla_target("latest"), &CancelToken::new(), None).await.unwrap();

    assert_eq!(launch.mc_version, "1.21.1", "latest → актуальный релиз из манифеста Mojang");
    assert_eq!(launch.install_key, "1.21.1|vanilla");
    assert_eq!(launch.java_major, 21);
    assert_eq!(launch.main_class, "net.minecraft.client.main.Main");
    assert!(launch.module_path.is_empty() && launch.forge_jvm_args.is_empty() && launch.forge_game_args.is_empty());
    assert_eq!(std::fs::read(client_dir.join("versions/1.21.1/1.21.1.jar")).unwrap(), b"CLIENT-JAR-BYTES");
    assert!(!client_dir.join("launcher_profiles.json").exists() && !client_dir.join("forge-install.log").exists());
    assert!(!client_dir.join("mods").exists(), "у vanilla нет модпака");
    let names: Vec<String> = launch.classpath.iter().map(|p| std::path::Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["lib-1.0.jar", "1.21.1.jar"]);
    let parsed = LaunchJson::load(&client_dir.join("launch.json")).await.unwrap();
    assert_eq!((parsed.mc_version.as_str(), parsed.java_major_or_default()), ("1.21.1", 21));
    assert_eq!(fx.srv.hits("/neoforge-21.1.172-installer.jar"), 0);
}

#[tokio::test]
async fn minecraft_1_20_1_uses_java_17_next_to_java_21() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    // Java 21 уже стоит (от другого instance), 1.20.1 требует 17 — ставится отдельно, 21 не трогается.
    let _r21 = runtime_major(dir.path(), false, 21).await;
    let _r17 = runtime_major(dir.path(), false, 17).await;
    assert!(dir.path().join("runtime/java17/bin/java").exists() && dir.path().join("runtime/java21/bin/java").exists());
    let client_dir = dir.path().join("instances/colonization");
    let launch = install(&build_client("t").unwrap(), dir.path(), &client_dir, &fx.sources, &vanilla_target("1.20.1"), &CancelToken::new(), None).await.unwrap();
    assert_eq!((launch.mc_version.as_str(), launch.java_major), ("1.20.1", 17), "Java выбрана по данным Mojang");
    assert_eq!(LaunchJson::load(&client_dir.join("launch.json")).await.unwrap().java_major_or_default(), 17);
}

#[tokio::test]
async fn missing_java_is_downloaded_for_the_required_major_only() {
    let fx = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    // Java 17 не установлена, API Adoptium недоступен → понятная ошибка про Java 17, и каталог java21 не создаётся.
    let _r21 = runtime_major(dir.path(), false, 21).await;
    let err = install(&build_client("t").unwrap(), dir.path(), &dir.path().join("c"), &fx.sources, &vanilla_target("1.20.1"), &CancelToken::new(), None).await;
    assert!(err.is_err(), "без сети Java 17 не ставится");
    assert!(!dir.path().join("runtime/java17/bin/java").exists());
}
