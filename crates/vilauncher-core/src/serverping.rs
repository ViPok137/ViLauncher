//! Пинг сервера (порт `server-ping`): Minecraft Server List Ping.
//! Простой TCP-пинг не подходит — Playit держит порт открытым даже без сервера,
//! поэтому выполняется настоящий Status-запрос.

use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Протокол в Handshake. Для статуса сервера достаточно 47 (как в оригинале).
const PROTOCOL: u32 = 47;
const MAX_PACKET: usize = 2 * 1024 * 1024;

/// Формат ответа совпадает с оригиналом: при ошибке только `{ online: false }`.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct PingResult {
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motd: Option<String>,
}

pub fn encode_varint(mut v: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            return out;
        }
    }
}

fn decode_varint(buf: &[u8]) -> Option<(u32, usize)> {
    let (mut value, mut shift) = (0u32, 0u32);
    for (i, &b) in buf.iter().enumerate().take(5) {
        value |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None
}

async fn read_varint(s: &mut TcpStream) -> std::io::Result<u32> {
    let (mut value, mut shift) = (0u32, 0u32);
    for _ in 0..5 {
        let b = s.read_u8().await?;
        value |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "VarInt слишком длинный"))
}

/// Читает пакет целиком → (packet id, остаток тела).
async fn read_packet(s: &mut TcpStream) -> std::io::Result<(u32, Vec<u8>)> {
    let len = read_varint(s).await? as usize;
    if len == 0 || len > MAX_PACKET {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "неверная длина пакета"));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    let (id, n) = decode_varint(&buf).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "нет id пакета"))?;
    Ok((id, buf[n..].to_vec()))
}

/// Текст MOTD без форматирования: строка или JSON-компонент (`text` + `extra`), коды `§x` удаляются.
pub fn motd_clean(desc: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::String(s) => out.push_str(s),
            Value::Object(o) => {
                if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                    out.push_str(t);
                }
                if let Some(extra) = o.get("extra").and_then(|e| e.as_array()) {
                    for e in extra {
                        walk(e, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|e| walk(e, out)),
            _ => {}
        }
    }
    let mut raw = String::new();
    walk(desc, &mut raw);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

async fn ping_inner(host: &str, port: u16) -> std::io::Result<PingResult> {
    let mut s = TcpStream::connect((host, port)).await?;
    let _ = s.set_nodelay(true);

    // Handshake (0x00) → NextState=1 (Status), затем Status Request (0x00).
    let mut body = encode_varint(0);
    body.extend(encode_varint(PROTOCOL));
    body.extend(encode_varint(host.len() as u32));
    body.extend(host.as_bytes());
    body.extend(port.to_be_bytes());
    body.extend(encode_varint(1));
    let mut pkt = encode_varint(body.len() as u32);
    pkt.extend(body);
    pkt.extend([0x01, 0x00]);

    let started = Instant::now();
    s.write_all(&pkt).await?;
    let (id, payload) = read_packet(&mut s).await?;
    if id != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "не Status Response"));
    }
    let status_ms = started.elapsed().as_millis() as u64;

    // Задержка — по Ping/Pong (как у minecraft-server-util); если сервер не ответил, берём время Status.
    let ping_started = Instant::now();
    let mut ping = vec![0x09, 0x01];
    ping.extend(0x1234_5678_u64.to_be_bytes());
    let latency = match s.write_all(&ping).await {
        Ok(()) => match tokio::time::timeout(Duration::from_millis(1500), read_packet(&mut s)).await {
            Ok(Ok((1, _))) => ping_started.elapsed().as_millis() as u64,
            _ => status_ms,
        },
        Err(_) => status_ms,
    };

    let mut result = PingResult { online: true, ping: Some(latency), ..Default::default() };
    let parsed = decode_varint(&payload).and_then(|(jl, n)| payload.get(n..n + jl as usize)).and_then(|j| serde_json::from_slice::<Value>(j).ok());
    if let Some(status) = parsed {
        result.version = Some(status.pointer("/version/name").and_then(|v| v.as_str()).unwrap_or("").to_string());
        result.online_count = Some(status.pointer("/players/online").and_then(|v| v.as_u64()).unwrap_or(0));
        result.max_count = Some(status.pointer("/players/max").and_then(|v| v.as_u64()).unwrap_or(0));
        result.motd = Some(status.get("description").map(motd_clean).unwrap_or_default());
    }
    Ok(result)
}

/// Любая ошибка/таймаут → `{ online: false }`.
pub async fn ping_server(host: &str, port: u16, timeout: Duration) -> PingResult {
    match tokio::time::timeout(timeout, ping_inner(host, port)).await {
        Ok(Ok(r)) => r,
        _ => PingResult::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn varint_roundtrip() {
        for v in [0u32, 1, 127, 128, 255, 25565, 2_097_151, u32::MAX >> 4] {
            let e = encode_varint(v);
            assert_eq!(decode_varint(&e), Some((v, e.len())));
        }
        assert_eq!(encode_varint(300), vec![0xAC, 0x02]);
    }

    #[test]
    fn motd_is_cleaned() {
        assert_eq!(motd_clean(&serde_json::json!("§aViPok §lserver")), "ViPok server");
        assert_eq!(motd_clean(&serde_json::json!({"text":"A ","extra":[{"text":"§cB"},"C"]})), "A BC");
    }

    async fn fake_server(json: String, answer_ping: bool) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            // handshake + status request
            let mut tmp = [0u8; 256];
            let _ = s.read(&mut tmp).await.unwrap();
            let j = json.as_bytes();
            let mut body = encode_varint(0);
            body.extend(encode_varint(j.len() as u32));
            body.extend(j);
            let mut pkt = encode_varint(body.len() as u32);
            pkt.extend(body);
            s.write_all(&pkt).await.unwrap();
            if answer_ping {
                let mut ping = [0u8; 10];
                if s.read_exact(&mut ping).await.is_ok() {
                    let mut pong = vec![0x09, 0x01];
                    pong.extend(&ping[2..]);
                    let _ = s.write_all(&pong).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        port
    }

    #[tokio::test]
    async fn parses_status_response() {
        let json = r#"{"version":{"name":"NeoForge 1.21.1","protocol":767},"players":{"max":20,"online":3},"description":{"text":"ViPok ","extra":[{"text":"§6server"}]}}"#;
        let port = fake_server(json.to_string(), true).await;
        let r = ping_server("127.0.0.1", port, Duration::from_secs(4)).await;
        assert!(r.online);
        assert_eq!(r.version.as_deref(), Some("NeoForge 1.21.1"));
        assert_eq!((r.online_count, r.max_count), (Some(3), Some(20)));
        assert_eq!(r.motd.as_deref(), Some("ViPok server"));
        assert!(r.ping.is_some());
    }

    #[tokio::test]
    async fn works_without_pong_and_with_garbage_json() {
        let port = fake_server("{not json".to_string(), false).await;
        let r = ping_server("127.0.0.1", port, Duration::from_secs(4)).await;
        assert!(r.online && r.ping.is_some() && r.version.is_none(), "{r:?}");
    }

    #[tokio::test]
    async fn closed_port_and_silent_port_are_offline() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(ping_server("127.0.0.1", port, Duration::from_secs(1)).await, PingResult::default());
        // «Playit держит порт открытым без сервера»: принял соединение и молчит → offline по таймауту
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
        });
        let r = ping_server("127.0.0.1", port, Duration::from_millis(500)).await;
        assert!(!r.online);
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"online":false}"#);
    }
}
