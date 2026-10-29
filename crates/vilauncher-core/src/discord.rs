//! Discord Rich Presence по IPC (порт `discord-rpc`): локальный сокет/именованный канал `discord-ipc-N`.
//! Если Discord не запущен или Client ID неверный — функция не работает, лаунчер продолжает как обычно.

use crate::error::{CoreError, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct Activity {
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    pub small_image: String,
    pub small_text: String,
    pub start_timestamp: u64,
    pub buttons: Vec<(String, String)>,
}

impl Activity {
    pub fn to_json(&self) -> Value {
        let mut a = json!({
            "details": self.details,
            "state": self.state,
            "assets": {
                "large_image": self.large_image, "large_text": self.large_text,
                "small_image": self.small_image, "small_text": self.small_text,
            },
            "timestamps": { "start": self.start_timestamp },
        });
        if !self.buttons.is_empty() {
            a["buttons"] = self.buttons.iter().take(2).map(|(l, u)| json!({"label": l, "url": u})).collect();
        }
        a
    }
}

pub struct DiscordRpc {
    stream: Box<dyn Duplex>,
    nonce: u64,
}

async fn write_frame(s: &mut dyn Duplex, op: u32, payload: &Value) -> std::io::Result<()> {
    let data = serde_json::to_vec(payload)?;
    let mut buf = Vec::with_capacity(8 + data.len());
    buf.extend(op.to_le_bytes());
    buf.extend((data.len() as u32).to_le_bytes());
    buf.extend(data);
    s.write_all(&buf).await?;
    s.flush().await
}

async fn read_frame(s: &mut dyn Duplex) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    s.read_exact(&mut head).await?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "слишком большой кадр"));
    }
    let mut data = vec![0u8; len];
    s.read_exact(&mut data).await?;
    Ok((op, serde_json::from_slice(&data).unwrap_or(Value::Null)))
}

impl DiscordRpc {
    /// Рукопожатие по уже открытому каналу.
    pub async fn handshake(mut stream: Box<dyn Duplex>, client_id: &str) -> Result<Self> {
        write_frame(stream.as_mut(), OP_HANDSHAKE, &json!({"v": 1, "client_id": client_id})).await?;
        let (_, reply) = tokio::time::timeout(Duration::from_secs(5), read_frame(stream.as_mut()))
            .await
            .map_err(|_| CoreError::other("Discord не ответил на рукопожатие"))??;
        match reply.get("evt").and_then(|e| e.as_str()) {
            Some("READY") => Ok(Self { stream, nonce: 0 }),
            _ => Err(CoreError::other(format!(
                "Discord отклонил подключение: {}",
                reply.pointer("/data/message").and_then(|m| m.as_str()).unwrap_or("неизвестная ошибка")
            ))),
        }
    }

    /// Подключение к первому доступному `discord-ipc-0..9`.
    pub async fn connect(client_id: &str) -> Result<Self> {
        let mut last = CoreError::other("Discord не запущен");
        for i in 0..10 {
            for stream in open_candidates(i).await {
                match Self::handshake(stream, client_id).await {
                    Ok(rpc) => return Ok(rpc),
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }

    pub async fn set_activity(&mut self, activity: &Activity) -> Result<()> {
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity.to_json() },
            "nonce": self.nonce.to_string(),
        });
        write_frame(self.stream.as_mut(), OP_FRAME, &payload).await?;
        // Ответ нужно вычитывать, иначе буфер канала переполнится; ошибка Discord → Err.
        if let Ok(Ok((_, reply))) = tokio::time::timeout(Duration::from_secs(2), read_frame(self.stream.as_mut())).await {
            if reply.get("evt").and_then(|e| e.as_str()) == Some("ERROR") {
                return Err(CoreError::other(format!("Discord: {}", reply.pointer("/data/message").and_then(|m| m.as_str()).unwrap_or("ошибка"))));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
async fn open_candidates(i: u32) -> Vec<Box<dyn Duplex>> {
    let mut bases: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|k| std::env::var(k).ok()).collect();
    bases.push("/tmp".into());
    let mut out: Vec<Box<dyn Duplex>> = Vec::new();
    for b in bases {
        for sub in ["", "snap.discord/", "app/com.discordapp.Discord/"] {
            let p = format!("{}/{sub}discord-ipc-{i}", b.trim_end_matches('/'));
            if let Ok(s) = tokio::net::UnixStream::connect(&p).await {
                out.push(Box::new(s));
            }
        }
    }
    out
}

#[cfg(windows)]
async fn open_candidates(i: u32) -> Vec<Box<dyn Duplex>> {
    match tokio::net::windows::named_pipe::ClientOptions::new().open(format!(r"\\.\pipe\discord-ipc-{i}")) {
        Ok(p) => vec![Box::new(p)],
        Err(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_json_shape() {
        let a = Activity {
            details: "Играет на ViPok server".into(), state: "Онлайн: 3 игроков".into(),
            large_image: "minecraft_icon".into(), large_text: "Minecraft 1.21.1 NeoForge".into(),
            small_image: "launcher_logo".into(), small_text: "ViPok Launcher".into(),
            start_timestamp: 1_700_000_000, buttons: vec![("Скачать лаунчер".into(), "https://github.com/x/y/releases".into())],
        };
        let j = a.to_json();
        assert_eq!(j["assets"]["large_image"], "minecraft_icon");
        assert_eq!(j["timestamps"]["start"], 1_700_000_000u64);
        assert_eq!(j["buttons"][0]["label"], "Скачать лаунчер");
        assert!(Activity::default().to_json().get("buttons").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn handshake_and_set_activity_over_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discord-ipc-0");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<(Value, Value)>();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let (op, hs) = read_frame(&mut s).await.unwrap();
            assert_eq!(op, OP_HANDSHAKE);
            write_frame(&mut s, OP_FRAME, &json!({"cmd":"DISPATCH","evt":"READY","data":{"v":1}})).await.unwrap();
            let (_, act) = read_frame(&mut s).await.unwrap();
            write_frame(&mut s, OP_FRAME, &json!({"cmd":"SET_ACTIVITY","evt":null,"nonce":"1"})).await.unwrap();
            let _ = tx.send((hs, act));
        });

        let stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        let mut rpc = DiscordRpc::handshake(Box::new(stream), "123456789").await.unwrap();
        rpc.set_activity(&Activity { details: "d".into(), state: "s".into(), start_timestamp: 5, ..Default::default() }).await.unwrap();
        let (hs, act) = rx.await.unwrap();
        assert_eq!(hs, json!({"v": 1, "client_id": "123456789"}));
        assert_eq!(act["cmd"], "SET_ACTIVITY");
        assert_eq!(act["args"]["pid"], std::process::id());
        assert_eq!(act["args"]["activity"]["details"], "d");
        assert_eq!(act["nonce"], "1");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_client_id_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discord-ipc-0");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let _ = read_frame(&mut s).await;
            let _ = write_frame(&mut s, 2, &json!({"cmd":"DISPATCH","evt":"ERROR","data":{"code":4000,"message":"Invalid Client ID"}})).await;
        });
        let stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        let err = DiscordRpc::handshake(Box::new(stream), "bad").await.err().unwrap();
        assert!(err.to_string().contains("Invalid Client ID"), "{err}");
    }
}
