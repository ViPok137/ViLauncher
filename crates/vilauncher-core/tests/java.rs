#![cfg(unix)] // фейковая JRE — shell-скрипт; на Windows логика та же, но бинарник java.exe

mod common;

use common::*;
use flate2::{write::GzEncoder, Compression};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use vilauncher_core::downloader::{build_client, CancelToken};
use vilauncher_core::java_runtime::*;
use vilauncher_core::launch::*;
use vilauncher_core::CoreError;

fn fake_java(version: &str) -> Vec<u8> {
    format!(
        r#"#!/bin/sh
if [ "$1" = "-version" ]; then
  echo 'openjdk version "{version}" 2024-07-16' >&2
  exit 0
fi
DIR="$(cd "$(dirname "$0")/.." && pwd)"
{{
  echo "JAVA_HOME=$JAVA_HOME"
  echo "JTO=$JAVA_TOOL_OPTIONS"
  echo "CP=$CLASSPATH"
  echo "PATH0=${{PATH%%:*}}"
  echo "ARGS=$*"
}} > "$DIR/run.txt"
"#
    )
    .into_bytes()
}

fn make_tar_gz(files: &[(&str, Vec<u8>, u32)]) -> Vec<u8> {
    let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, data, mode) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(*mode);
        h.set_cksum();
        b.append_data(&mut h, path, &data[..]).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn jre_tar(version: &str) -> Vec<u8> {
    make_tar_gz(&[
        ("jdk-21.0.4+7-jre/bin/java", fake_java(version), 0o755),
        ("jdk-21.0.4+7-jre/release", b"JAVA_VERSION=21".to_vec(), 0o644),
        ("jdk-21.0.4+7-jre/lib/modules", vec![7u8; 4096], 0o644),
    ])
}

fn sha256_hex(d: &[u8]) -> String {
    hex::encode(Sha256::digest(d))
}

fn api_path() -> String {
    let os = match std::env::consts::OS { "macos" => "mac", o => o };
    let arch = match std::env::consts::ARCH { "x86_64" => "x64", a => a };
    format!("/v3/assets/latest/21/hotspot?architecture={arch}&image_type=jre&os={os}&vendor=eclipse")
}

fn assets_json(name: &str, checksum: &str, size: usize) -> Vec<u8> {
    format!(r#"[{{"binary":{{"package":{{"link":"{{{{BASE}}}}/pkg/{name}","name":"{name}","checksum":"{checksum}","size":{size}}}}}}}]"#).into_bytes()
}

#[tokio::test]
async fn ensure_downloads_verifies_extracts_and_is_then_a_noop() {
    let tar = jre_tar("21.0.4");
    let name = "OpenJDK21U-jre_x64_linux_hotspot_21.0.4_7.tar.gz";
    let srv = start(
        HashMap::from([
            (api_path(), assets_json(name, &sha256_hex(&tar), tar.len())),
            (format!("/pkg/{name}"), tar.clone()),
        ]),
        HashMap::new(),
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path()).with_api_base(srv.base());
    let client = build_client("test").unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<JavaProgress>::new()));
    let ev = events.clone();
    let cb: JavaProgressFn = std::sync::Arc::new(move |p| ev.lock().unwrap().push(p));

    let exe = rt.ensure(&client, &CancelToken::new(), Some(cb)).await.unwrap();
    assert_eq!(exe, dir.path().join("runtime/java21/bin/java"));
    assert!(dir.path().join("runtime/java21/release").exists(), "корневая папка архива срезана");
    assert!(dir.path().join("runtime/java21/.vilauncher-runtime.json").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0o111, "+x сохранён");
    }
    let info = rt.verify().await.unwrap();
    assert_eq!(info.major, 21);
    let phases: Vec<_> = events.lock().unwrap().iter().map(|e| e.phase).collect();
    for p in ["resolve", "download", "extract", "verify", "done"] {
        assert!(phases.contains(&p), "нет фазы {p}: {phases:?}");
    }
    // мусор убран
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("runtime")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["java21"], "{leftovers:?}");

    // Повторный вызов: установлено и работает → сеть не трогаем.
    let before = srv.total_hits();
    rt.ensure(&client, &CancelToken::new(), None).await.unwrap();
    assert_eq!(srv.total_hits(), before);
}

#[tokio::test]
async fn sha256_mismatch_is_rejected_and_nothing_is_installed() {
    let tar = jre_tar("21.0.4");
    let name = "pkg.tar.gz";
    let srv = start(
        HashMap::from([
            (api_path(), assets_json(name, &"0".repeat(64), tar.len())),
            (format!("/pkg/{name}"), tar),
        ]),
        HashMap::new(),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path()).with_api_base(srv.base());
    let err = rt.ensure(&build_client("t").unwrap(), &CancelToken::new(), None).await.unwrap_err();
    assert!(err.to_string().contains("Хеш не совпал"), "{err}");
    assert!(!rt.exe_exists());
}

#[tokio::test]
async fn wrong_java_major_is_rejected_and_existing_runtime_survives() {
    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path());

    // 1. Ставим рабочую 21-ю из локального архива.
    let good = dir.path().join("good.tar.gz");
    std::fs::write(&good, jre_tar("21.0.4")).unwrap();
    rt.install_from_archive(&good, "good.tar.gz", None, &None).await.unwrap();
    let marker_before = std::fs::read_to_string(dir.path().join("runtime/java21/.vilauncher-runtime.json")).unwrap();

    // 2. Пытаемся «обновиться» на Java 17 → отказ, прежняя сборка цела.
    let bad = dir.path().join("bad.tar.gz");
    std::fs::write(&bad, jre_tar("17.0.12")).unwrap();
    let err = rt.install_from_archive(&bad, "bad.tar.gz", None, &None).await.unwrap_err();
    assert!(err.to_string().contains("Ожидалась Java 21"), "{err}");
    assert_eq!(rt.verify().await.unwrap().major, 21);
    assert_eq!(std::fs::read_to_string(dir.path().join("runtime/java21/.vilauncher-runtime.json")).unwrap(), marker_before);
    assert!(!dir.path().join("runtime/java21.staging").exists());
}

#[tokio::test]
async fn broken_runtime_is_reinstalled() {
    let tar = jre_tar("21.0.4");
    let name = "p.tar.gz";
    let srv = start(HashMap::from([(api_path(), assets_json(name, &sha256_hex(&tar), tar.len())), (format!("/pkg/{name}"), tar)]), HashMap::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path()).with_api_base(srv.base());
    // «Установленная» java, которая не запускается (повреждена).
    std::fs::create_dir_all(dir.path().join("runtime/java21/bin")).unwrap();
    std::fs::write(dir.path().join("runtime/java21/bin/java"), b"garbage").unwrap();
    rt.ensure(&build_client("t").unwrap(), &CancelToken::new(), None).await.unwrap();
    assert_eq!(rt.verify().await.unwrap().major, 21);
}

#[tokio::test]
async fn game_is_launched_via_isolated_java_ignoring_user_java_home() {
    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path());
    let pkg = dir.path().join("p.tar.gz");
    std::fs::write(&pkg, jre_tar("21.0.4")).unwrap();
    rt.install_from_archive(&pkg, "p.tar.gz", None, &None).await.unwrap();

    // «Системные» переменные пользователя, которые обязаны быть проигнорированы.
    std::env::set_var("JAVA_HOME", "/opt/user-jdk-8");
    std::env::set_var("JAVA_TOOL_OPTIONS", "-Xmx1M -javaagent:/evil.jar");
    std::env::set_var("CLASSPATH", "/user/cp.jar");

    let game_dir = dir.path().join("client");
    std::fs::create_dir_all(&game_dir).unwrap();
    let l = LaunchJson {
        main_class: "net.Main".into(), classpath: vec![], module_path: vec![], forge_jvm_args: vec![], forge_game_args: vec![],
        assets_dir: "a".into(), asset_index: "5".into(), natives_dir: "n".into(), game_dir: game_dir.to_string_lossy().into(), mc_version: String::new(), install_key: String::new(), java_major: 0,
    };
    let args = vec!["-Xmx2G".to_string(), "net.Main".to_string(), "--username".to_string(), "Vik".to_string()];
    let mut child = spawn_game(&rt, &l, &args, &dir.path().join("minecraft.log")).unwrap();
    assert!(child.wait().await.unwrap().success());

    let run = std::fs::read_to_string(rt.root().join("run.txt")).unwrap();
    assert!(run.contains(&format!("JAVA_HOME={}", rt.root().display())), "JAVA_HOME должен указывать на изолированную JRE:\n{run}");
    assert!(run.contains("JTO=\n"), "JAVA_TOOL_OPTIONS вычищен:\n{run}");
    assert!(run.contains("CP=\n"), "CLASSPATH вычищен:\n{run}");
    assert!(run.contains(&format!("PATH0={}", rt.root().join("bin").display())), "{run}");
    assert!(run.contains("ARGS=-Xmx2G net.Main --username Vik"), "{run}");
    assert!(!run.contains("user-jdk-8") && !run.contains("evil"));
}

#[tokio::test]
async fn spawn_without_runtime_fails_with_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let rt = JavaRuntime::new(dir.path());
    let l = LaunchJson { main_class: "x".into(), classpath: vec![], module_path: vec![], forge_jvm_args: vec![], forge_game_args: vec![], assets_dir: "".into(), asset_index: "".into(), natives_dir: "".into(), game_dir: dir.path().to_string_lossy().into(), mc_version: String::new(), install_key: String::new(), java_major: 0 };
    let err = spawn_game(&rt, &l, &[], &dir.path().join("l.log")).unwrap_err();
    assert!(err.to_string().contains("Изолированная Java 21 не найдена"), "{err}");
}

// ───────────── безопасность распаковки ─────────────

fn raw_tar_gz_with_name(name: &str, data: &[u8]) -> Vec<u8> {
    // tar::Builder отвергает `..`, поэтому заголовок собираем вручную.
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::Regular);
    {
        let gnu = h.as_gnu_mut().unwrap();
        gnu.name = [0u8; 100];
        gnu.name[..name.len()].copy_from_slice(name.as_bytes());
    }
    h.set_cksum();
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(h.as_bytes()).unwrap();
    enc.write_all(data).unwrap();
    enc.write_all(&vec![0u8; (512 - data.len() % 512) % 512]).unwrap();
    enc.write_all(&[0u8; 1024]).unwrap();
    enc.finish().unwrap()
}

#[test]
fn tar_path_traversal_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ar = dir.path().join("evil.tar.gz");
    std::fs::write(&ar, raw_tar_gz_with_name("jdk/../../evil.txt", b"pwned")).unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let err = extract_tar_gz(&ar, &dest).unwrap_err();
    assert!(matches!(err, CoreError::UnsafePath(_)), "{err:?}");
    assert!(!dir.path().join("evil.txt").exists());
}

#[test]
fn tar_escaping_symlink_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_mode(0o777);
    b.append_link(&mut h, "jdk/bin/sneaky", "/etc").unwrap();
    let bytes = b.into_inner().unwrap().finish().unwrap();
    let ar = dir.path().join("link.tar.gz");
    std::fs::write(&ar, bytes).unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    assert!(matches!(extract_tar_gz(&ar, &dest).unwrap_err(), CoreError::UnsafePath(_)));
}

#[test]
fn zip_extraction_strips_root_sets_exec_bit_and_blocks_zip_slip() {
    let dir = tempfile::tempdir().unwrap();
    let ar = dir.path().join("jre.zip");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&ar).unwrap());
        let exec = zip::write::FileOptions::default().unix_permissions(0o755);
        z.start_file("jdk-21.0.4+7-jre/bin/java", exec).unwrap();
        z.write_all(b"#!/bin/sh\n").unwrap();
        z.start_file("jdk-21.0.4+7-jre/release", zip::write::FileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
    }
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    extract_zip(&ar, &dest).unwrap();
    assert!(dest.join("release").exists());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dest.join("bin/java")).unwrap().permissions().mode() & 0o111, 0o111);

    let evil = dir.path().join("evil.zip");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
        z.start_file("jdk/../../zipslip.txt", zip::write::FileOptions::default()).unwrap();
        z.write_all(b"pwned").unwrap();
        z.finish().unwrap();
    }
    assert!(extract_zip(&evil, &dest).is_err());
    assert!(!dir.path().join("zipslip.txt").exists());
    let _ = Path::new("");
}
