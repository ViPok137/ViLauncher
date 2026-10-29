//! Скины (порт `skin-get` / `skin-upload` / `skin-reset`).

use crate::downloader::HttpClient;
use crate::fsutil::encode_component;
use base64::Engine;
use std::path::Path;
use tokio::io::AsyncReadExt;

pub const SKIN_API: &str = "https://api.aurora-launcher.ru/mojang/username/skin/";
const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..8] == PNG_SIG
}

pub fn png_data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Скин игрока с API как data:URL (из renderer CORS блокирует canvas на внешние URL).
/// 204 / 404 / любая ошибка → `None` (скин не задан).
pub async fn fetch_skin(client: &HttpClient, api_base: &str, username: &str) -> Option<String> {
    let url = format!("{}{}", api_base, encode_component(username));
    let resp = client.get(&url).header("User-Agent", "ViLauncher/1.0").send().await.ok()?;
    let status = resp.status();
    if status == reqwest::StatusCode::NO_CONTENT || status.as_u16() >= 400 {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    Some(png_data_url(&bytes))
}

/// Кастомный скин, если загружен вручную.
pub async fn custom_skin_data_url(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(png_data_url(&bytes))
}

/// Копирует выбранный файл как кастомный скин после проверки PNG-сигнатуры.
pub async fn install_custom_skin(src: &Path, dest: &Path) -> Result<(), String> {
    let mut head = [0u8; 8];
    let mut f = tokio::fs::File::open(src).await.map_err(|e| e.to_string())?;
    if f.read_exact(&mut head).await.is_err() || !is_png(&head) {
        return Err("Файл не является валидным PNG".to_string());
    }
    tokio::fs::copy(src, dest).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Сброс кастомного скина: «файла и не было» — тоже успех.
pub async fn reset_custom_skin(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_detection_and_data_url() {
        let mut png = PNG_SIG.to_vec();
        png.extend([1, 2, 3]);
        assert!(is_png(&png));
        assert!(!is_png(b"GIF89a.."));
        assert!(!is_png(&[0x89, 0x50]));
        assert!(png_data_url(&png).starts_with("data:image/png;base64,iVBORw0KGgo"));
    }

    #[tokio::test]
    async fn custom_skin_roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("s.png");
        let mut b = PNG_SIG.to_vec();
        b.extend([0u8; 16]);
        std::fs::write(&good, &b).unwrap();
        let bad = dir.path().join("fake.png");
        std::fs::write(&bad, b"not a png at all").unwrap();
        let short = dir.path().join("short.png");
        std::fs::write(&short, b"x").unwrap();
        let dest = dir.path().join("custom_skin.png");

        assert_eq!(install_custom_skin(&bad, &dest).await.unwrap_err(), "Файл не является валидным PNG");
        assert_eq!(install_custom_skin(&short, &dest).await.unwrap_err(), "Файл не является валидным PNG");
        assert!(!dest.exists());
        install_custom_skin(&good, &dest).await.unwrap();
        assert!(custom_skin_data_url(&dest).await.unwrap().starts_with("data:image/png;base64,"));
        reset_custom_skin(&dest).await;
        reset_custom_skin(&dest).await; // повторный сброс — не ошибка
        assert!(custom_skin_data_url(&dest).await.is_none());
    }
}
