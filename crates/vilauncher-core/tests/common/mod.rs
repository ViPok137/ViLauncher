//! Минимальный HTTP/1.1-сервер для интеграционных тестов (без внешних зависимостей).
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct MockServer {
    pub addr: SocketAddr,
    pub hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl MockServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn hits(&self, path: &str) -> usize {
        *self.hits.lock().unwrap().get(path).unwrap_or(&0)
    }
    pub fn total_hits(&self) -> usize {
        self.hits.lock().unwrap().values().sum()
    }
}

/// `routes`: точный путь запроса → тело ответа.
/// `fail_first`: путь → сколько первых запросов вернуть с HTTP 500.
pub async fn start(routes: HashMap<String, Vec<u8>>, fail_first: HashMap<String, usize>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    // Плейсхолдер {{BASE}} в текстовых телах заменяется на реальный адрес сервера.
    let base = format!("http://{addr}");
    let routes: HashMap<String, Vec<u8>> = routes
        .into_iter()
        .map(|(k, v)| match String::from_utf8(v.clone()) {
            Ok(t) if t.contains("{{BASE}}") => (k, t.replace("{{BASE}}", &base).into_bytes()),
            _ => (k, v),
        })
        .collect();
    let routes = Arc::new(routes);
    let fail_first = Arc::new(fail_first);
    let hits2 = hits.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let routes = routes.clone();
            let fail_first = fail_first.clone();
            let hits = hits2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();

                let count = {
                    let mut h = hits.lock().unwrap();
                    let c = h.entry(path.clone()).or_insert(0);
                    *c += 1;
                    *c
                };

                let (status, body): (&str, Vec<u8>) = if fail_first.get(&path).map_or(false, |n| count <= *n) {
                    ("500 Internal Server Error", b"boom".to_vec())
                } else if let Some(b) = routes.get(&path) {
                    ("200 OK", b.clone())
                } else {
                    ("404 Not Found", b"nope".to_vec())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });

    MockServer { addr, hits }
}

pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = vilauncher_core::hash::Hasher::new(vilauncher_core::hash::HashAlgo::Sha1);
    h.update(data);
    h.finalize_hex()
}
pub fn md5_hex(data: &[u8]) -> String {
    let mut h = vilauncher_core::hash::Hasher::new(vilauncher_core::hash::HashAlgo::Md5);
    h.update(data);
    h.finalize_hex()
}

pub fn fast_opts() -> vilauncher_core::downloader::DownloadOptions {
    vilauncher_core::downloader::DownloadOptions {
        backoff: std::time::Duration::from_millis(10),
        ..Default::default()
    }
}

/// Сценарный сервер: n-й запрос к пути получает n-й ответ `(HTTP-статус, тело)`, последний повторяется.
pub async fn start_scripted(script: HashMap<String, Vec<(u16, String)>>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let script = Arc::new(script);
    let hits2 = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let script = script.clone();
            let hits = hits2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // читаем заголовки, затем (если есть) тело по Content-Length
                loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                        if buf.len() >= pos + 4 + len { break; }
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();
                let n = { let mut h = hits.lock().unwrap(); let c = h.entry(path.clone()).or_insert(0); *c += 1; *c };
                let (code, body) = match script.get(&path) {
                    Some(v) => v[(n - 1).min(v.len() - 1)].clone(),
                    None => (404, "{}".to_string()),
                };
                let reason = match code { 200 => "OK", 400 => "Bad Request", 401 => "Unauthorized", 404 => "Not Found", _ => "Error" };
                let resp = format!("HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}", body.len());
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    MockServer { addr, hits }
}
