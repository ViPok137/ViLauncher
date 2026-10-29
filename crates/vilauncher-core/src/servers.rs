//! Описание серверов (`servers.json`): какие серверы есть в лаунчере, на каком instance'е они живут
//! и какие из них сейчас доступны для игры (`active`).
//!
//! Включить или выключить сервер, добавить новый — правка `servers.json` в репозитории, лаунчер пересобирать
//! не нужно: файл скачивается при запуске (с кэшем на диске); без сети используется кэш, а если его нет —
//! встроенная копия `servers.default.json`.

use crate::downloader::{fetch_text, HttpClient};
use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const DEFAULT_SERVERS_JSON: &str = include_str!("servers.default.json");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    #[default]
    Modded,
    Vanilla,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCfg {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    /// Версия Minecraft или `latest` (только для vanilla).
    pub mc_version: String,
    /// `neoforge` / `forge` (для modded; информативно — сам загрузчик определяется installer'ом из манифеста).
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: ServerKind,
    /// Каталог клиента `instances/<instanceId>`; по умолчанию равен `id`.
    #[serde(default)]
    pub instance_id: String,
    /// `host[:port]` игрового сервера (нужен активным серверам).
    #[serde(default)]
    pub address: String,
    /// База манифестов (`manifest.json`, `defaults.json`) этого сервера; по умолчанию — репозиторий лаунчера.
    #[serde(default)]
    pub manifest_url: Option<String>,
    /// `true` — в сервер можно играть. У остальных карточка видна, но «Играть» заблокирована.
    #[serde(default, alias = "enabled")]
    pub active: bool,
    /// Доступ по подписке (проверяет сервер, лаунчер только показывает метку).
    #[serde(default)]
    pub subscription: bool,
    #[serde(default)]
    pub order: i32,
    /// Принудительная версия Java (обычно не нужна — берётся по версии Minecraft / данным Mojang).
    #[serde(default)]
    pub java_major: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ServersFile {
    #[serde(default)]
    servers: Vec<serde_json::Value>,
}

impl ServerCfg {
    pub fn can_play(&self) -> bool {
        self.active
    }

    pub fn is_vanilla(&self) -> bool {
        self.kind == ServerKind::Vanilla
    }

    /// База манифестов: `manifestUrl` или `default_base`.
    pub fn manifest_base(&self, default_base: &str) -> String {
        self.manifest_url.as_deref().filter(|u| !u.trim().is_empty()).unwrap_or(default_base).trim_end_matches('/').to_string()
    }

    /// `host` и порт (по умолчанию 25565); `None`, если адрес пуст или неверен.
    pub fn host_port(&self) -> Option<(String, u16)> {
        parse_address(&self.address)
    }
}

/// `id`/`instanceId` становятся именем каталога — только безопасные символы.
pub fn is_valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

pub fn parse_address(addr: &str) -> Option<(String, u16)> {
    let a = addr.trim();
    if a.is_empty() {
        return None;
    }
    let (host, port) = match a.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p > 0)?),
        None => (a, 25565),
    };
    let ok = !host.is_empty() && host.len() <= 253 && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then(|| (host.to_string(), port))
}

/// База манифестов должна быть https-ссылкой без учётных данных.
pub fn validate_base_url(url: &str, allow_http: bool) -> bool {
    let rest = match (url.strip_prefix("https://"), allow_http.then(|| url.strip_prefix("http://")).flatten()) {
        (Some(r), _) | (None, Some(r)) => r,
        _ => return false,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains('@') && !url.contains(char::is_whitespace)
}

/// Разбор и проверка `servers.json`. Некорректные записи пропускаются (с предупреждением в лог),
/// остальные работают; пустой результат — ошибка.
pub fn parse_servers(text: &str) -> Result<Vec<ServerCfg>> {
    parse_servers_with(text, false)
}

pub fn parse_servers_with(text: &str, allow_http: bool) -> Result<Vec<ServerCfg>> {
    let file: ServersFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let (mut out, mut ids, mut instances): (Vec<ServerCfg>, HashSet<String>, HashSet<String>) = (Vec::new(), HashSet::new(), HashSet::new());
    for (i, raw) in file.servers.into_iter().enumerate() {
        let mut s: ServerCfg = match serde_json::from_value(raw) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[servers] запись #{i} пропущена: {e}");
                continue;
            }
        };
        if s.instance_id.is_empty() {
            s.instance_id = s.id.clone();
        }
        let problem = if !is_valid_id(&s.id) {
            Some("неверный id (a-z, 0-9, - и _, до 32 символов)")
        } else if !is_valid_id(&s.instance_id) {
            Some("неверный instanceId")
        } else if s.name.trim().is_empty() || s.mc_version.trim().is_empty() {
            Some("нет name или mcVersion")
        } else if s.mc_version == "latest" && !s.is_vanilla() {
            Some("`latest` допустим только для vanilla")
        } else if !ids.insert(s.id.clone()) {
            Some("повторяющийся id")
        } else if !instances.insert(s.instance_id.clone()) {
            Some("instanceId уже занят другим сервером")
        } else if s.manifest_url.as_deref().filter(|u| !u.trim().is_empty()).map(|u| !validate_base_url(u, allow_http)).unwrap_or(false) {
            Some("manifestUrl должен быть https-ссылкой")
        } else if s.active && s.host_port().is_none() {
            Some("у активного сервера нет корректного address")
        } else {
            None
        };
        if let Some(p) = problem {
            log::warn!("[servers] «{}» пропущен: {p}", s.id);
            // id/instance уже занесены в множества выше только при успехе, поэтому откатываем для надёжности
            continue;
        }
        out.push(s);
    }
    if out.is_empty() {
        return Err(CoreError::other("В servers.json нет ни одного корректного сервера"));
    }
    out.sort_by_key(|s| s.order); // стабильная: при равном order сохраняется порядок файла
    Ok(out)
}

/// Встроенный список (запасной вариант без сети и кэша).
pub fn default_servers() -> Vec<ServerCfg> {
    parse_servers(DEFAULT_SERVERS_JSON).expect("встроенный servers.default.json корректен")
}

pub async fn fetch_servers(client: &HttpClient, base: &str) -> Result<(Vec<ServerCfg>, String)> {
    let text = fetch_text(client, &format!("{}/servers.json", base.trim_end_matches('/')), 2).await?;
    Ok((parse_servers(&text)?, text))
}

fn cache_path(launcher_dir: &Path) -> PathBuf {
    launcher_dir.join("servers.cache.json")
}

pub async fn save_servers_cache(launcher_dir: &Path, raw: &str) {
    let _ = tokio::fs::write(cache_path(launcher_dir), raw).await;
}

pub async fn load_servers_cache(launcher_dir: &Path) -> Option<Vec<ServerCfg>> {
    parse_servers(&tokio::fs::read_to_string(cache_path(launcher_dir)).await.ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_match_the_spec() {
        let s = default_servers();
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["create", "colonization", "infection", "vanilla"]);
        let active: Vec<&str> = s.iter().filter(|x| x.active).map(|x| x.id.as_str()).collect();
        assert_eq!(active, ["create"], "активен только Create");
        let by = |id: &str| s.iter().find(|x| x.id == id).unwrap();
        assert_eq!((by("create").mc_version.as_str(), by("create").loader.as_deref(), by("create").kind), ("1.21.1", Some("neoforge"), ServerKind::Modded));
        assert_eq!(by("colonization").mc_version, "1.20.1");
        assert_eq!(by("infection").mc_version, "1.21.1");
        assert!(by("vanilla").is_vanilla() && by("vanilla").subscription && by("vanilla").mc_version == "latest");
        assert_ne!(by("create").instance_id, by("infection").instance_id, "два сервера на 1.21.1 — разные instance'ы");
        assert!(!by("colonization").can_play() && by("create").can_play());
        assert_eq!(by("create").host_port(), Some(("subjects-emirates.gl.joinmc.link".into(), 25565)));
    }

    #[test]
    fn repo_servers_json_equals_embedded_default() {
        let root = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../servers.json")).expect("servers.json в корне репозитория");
        assert_eq!(root.replace("\r\n", "\n"), DEFAULT_SERVERS_JSON.replace("\r\n", "\n"), "servers.json в корне и встроенный servers.default.json должны совпадать");
    }

    #[test]
    fn enabling_a_server_is_a_config_edit() {
        let edited = DEFAULT_SERVERS_JSON
            .replace("\"id\": \"colonization\"", "\"id\": \"colonization\"")
            .replacen("\"address\": \"\",\n      \"manifestUrl\": \"https://raw.githubusercontent.com/ViPok137/ViLauncher/main/servers/colonization\",\n      \"active\": false", "\"address\": \"mc.example.org:25570\",\n      \"manifestUrl\": \"https://raw.githubusercontent.com/ViPok137/ViLauncher/main/servers/colonization\",\n      \"active\": true", 1);
        assert_ne!(edited, DEFAULT_SERVERS_JSON);
        let s = parse_servers(&edited).unwrap();
        let c = s.iter().find(|x| x.id == "colonization").unwrap();
        assert!(c.can_play());
        assert_eq!(c.host_port(), Some(("mc.example.org".into(), 25570)));
        // alias `enabled` тоже понимается
        let s = parse_servers(r#"{"servers":[{"id":"a","name":"A","mcVersion":"1.21.1","enabled":true,"address":"h"}]}"#).unwrap();
        assert!(s[0].active && s[0].instance_id == "a" && s[0].kind == ServerKind::Modded);
    }

    #[test]
    fn adding_a_fifth_server_and_ordering() {
        let s = parse_servers(r#"{"servers":[
            {"id":"b","name":"B","mcVersion":"1.21.1","order":5},
            {"id":"a","name":"A","mcVersion":"1.21.1","order":2},
            {"id":"c","name":"C","mcVersion":"1.21.1","order":2}]}"#).unwrap();
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["a", "c", "b"], "по order, затем порядок файла");
    }

    #[test]
    fn invalid_entries_are_skipped_not_fatal() {
        let text = r#"{"servers":[
            {"id":"../evil","name":"x","mcVersion":"1.21.1"},
            {"id":"ok","name":"OK","mcVersion":"1.21.1"},
            {"id":"ok","name":"dup","mcVersion":"1.21.1"},
            {"id":"other","instanceId":"ok","name":"same instance","mcVersion":"1.21.1"},
            {"id":"noname","name":" ","mcVersion":"1.21.1"},
            {"id":"modlatest","name":"M","mcVersion":"latest","type":"modded"},
            {"id":"badurl","name":"U","mcVersion":"1.21.1","manifestUrl":"http://evil.example/x"},
            {"id":"creds","name":"U","mcVersion":"1.21.1","manifestUrl":"https://github.com@evil.example/x"},
            {"id":"noaddr","name":"N","mcVersion":"1.21.1","active":true},
            {"id":"van","name":"V","mcVersion":"latest","type":"vanilla"},
            {"id":"weird","name":"W","mcVersion":"1.21.1","type":"alien"}
        ]}"#;
        let s = parse_servers(text).unwrap();
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["ok", "van"]);
        assert!(parse_servers(r#"{"servers":[]}"#).is_err());
        assert!(parse_servers("not json").is_err());
        assert!(parse_servers(r#"{"servers":[{"id":"BAD ID","name":"x","mcVersion":"1"}]}"#).is_err());
    }

    #[test]
    fn address_and_url_helpers() {
        assert_eq!(parse_address("play.example.com"), Some(("play.example.com".into(), 25565)));
        assert_eq!(parse_address(" host:1234 "), Some(("host".into(), 1234)));
        for bad in ["", "host:0", "host:70000", "host:abc", "ho st", ":25565", "a/b"] {
            assert_eq!(parse_address(bad), None, "{bad:?}");
        }
        assert!(validate_base_url("https://raw.githubusercontent.com/x/y/main", false));
        assert!(!validate_base_url("http://raw.githubusercontent.com/x", false));
        assert!(validate_base_url("http://127.0.0.1:8080", true));
        assert!(!validate_base_url("https://", false) && !validate_base_url("ftp://x", true) && !validate_base_url("https://a b/", false));
        let s = default_servers();
        assert_eq!(s[1].manifest_base("https://base"), "https://raw.githubusercontent.com/ViPok137/ViLauncher/main/servers/colonization");
        assert_eq!(s[3].manifest_base("https://base/"), "https://base", "vanilla без manifestUrl → база по умолчанию");
        for id in ["create", "a1", "x_y-z", "0abc"] {
            assert!(is_valid_id(id), "{id}");
        }
        for id in ["", "-a", "A", "a b", "a/b", "..", &"x".repeat(33)] {
            assert!(!is_valid_id(id), "{id:?}");
        }
    }

    #[tokio::test]
    async fn cache_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_servers_cache(d.path()).await.is_none());
        save_servers_cache(d.path(), DEFAULT_SERVERS_JSON).await;
        assert_eq!(load_servers_cache(d.path()).await.unwrap().len(), 4);
        save_servers_cache(d.path(), "{broken").await;
        assert!(load_servers_cache(d.path()).await.is_none(), "битый кэш игнорируется");
    }
}
