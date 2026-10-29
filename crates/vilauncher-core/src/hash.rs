//! Потоковое хеширование файлов (SHA-1 / MD5 / SHA-256).
//! Файл никогда не читается в память целиком — только блоками по 64 КиБ.

use crate::error::Result;
use sha1::Digest;
use std::io::Read;
use std::path::{Path, PathBuf};

const BUF_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgo {
    Sha1,
    Md5,
    Sha256,
}

/// Инкрементальный хешер — используется и при чтении локального файла,
/// и «на лету» при скачивании (без повторного прохода по файлу).
pub enum Hasher {
    Sha1(sha1::Sha1),
    Md5(md5::Md5),
    Sha256(sha2::Sha256),
}

impl Hasher {
    pub fn new(algo: HashAlgo) -> Self {
        match algo {
            HashAlgo::Sha1 => Hasher::Sha1(sha1::Sha1::new()),
            HashAlgo::Md5 => Hasher::Md5(md5::Md5::new()),
            HashAlgo::Sha256 => Hasher::Sha256(sha2::Sha256::new()),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(data),
            Hasher::Md5(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
        }
    }

    pub fn finalize_hex(self) -> String {
        match self {
            Hasher::Sha1(h) => hex::encode(h.finalize()),
            Hasher::Md5(h) => hex::encode(h.finalize()),
            Hasher::Sha256(h) => hex::encode(h.finalize()),
        }
    }
}

/// Синхронное хеширование (вызывать только из `spawn_blocking`).
pub fn hash_file_blocking(path: &Path, algo: HashAlgo) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new(algo);
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

/// Асинхронная обёртка: тяжёлое чтение+хеширование уходит в blocking-пул
/// tokio и не блокирует async-рантайм.
pub async fn hash_file(path: impl Into<PathBuf>, algo: HashAlgo) -> Result<String> {
    let path = path.into();
    tokio::task::spawn_blocking(move || hash_file_blocking(&path, algo))
        .await
        .map_err(|e| crate::error::CoreError::other(format!("hash task: {e}")))?
}

/// Сравнение хешей без учёта регистра и пробелов по краям.
pub fn hash_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}
