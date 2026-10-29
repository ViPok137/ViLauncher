//! Безопасная работа с путями из недоверенных источников (манифест, архивы).

use crate::error::{CoreError, Result};
use std::path::{Component, Path, PathBuf};

/// Склеивает `base` и относительный путь из манифеста.
/// Отвергает абсолютные пути, `..`, буквы дисков (`C:`), UNC и NUL-байты —
/// иначе скомпрометированный манифест мог бы записать файл за пределы клиента.
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.contains('\0') {
        return Err(CoreError::UnsafePath(rel.to_string()));
    }
    let normalized = rel.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err(CoreError::UnsafePath(rel.to_string()));
    }
    let mut out = base.to_path_buf();
    let mut depth = 0usize;
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
            Component::CurDir => {}
            _ => return Err(CoreError::UnsafePath(rel.to_string())),
        }
    }
    if depth == 0 {
        return Err(CoreError::UnsafePath(rel.to_string()));
    }
    Ok(out)
}

/// Кодирует путь для URL: каждый сегмент отдельно, `/` сохраняется.
/// Пробелы, `+`, `#`, `%` и не-ASCII символы кодируются в %XX.
pub fn encode_url_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for byte in path.replace('\\', "/").bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Аналог `encodeURIComponent`: кодирует всё, кроме `A-Z a-z 0-9 - _ . ~` (в т.ч. `/`).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Допустимое имя файла без пути: без разделителей, `..`, `:` и управляющих символов.
pub fn safe_file_name(name: &str) -> Result<&str> {
    let bad = name.is_empty()
        || name == "."
        || name.contains("..")
        || name.contains(['/', '\\', ':', '\0'])
        || name.chars().any(|c| c.is_control());
    if bad {
        return Err(CoreError::UnsafePath(name.to_string()));
    }
    Ok(name)
}

/// `path` + суффикс (например `.part`) без потери исходного расширения.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute() {
        let base = Path::new("/tmp/client");
        for bad in ["../x", "a/../../x", "/etc/passwd", "C:/Windows/x", "..\\x", "", "a\0b", "\\\\srv\\share"] {
            assert!(safe_join(base, bad).is_err(), "должен отвергнуть {bad:?}");
        }
    }

    #[test]
    fn accepts_normal_paths() {
        let base = Path::new("/tmp/client");
        assert_eq!(safe_join(base, "mods/a.jar").unwrap(), base.join("mods").join("a.jar"));
        assert_eq!(safe_join(base, "mods\\b.jar").unwrap(), base.join("mods").join("b.jar"));
        assert_eq!(safe_join(base, "./config/x.toml").unwrap(), base.join("config").join("x.toml"));
    }

    #[test]
    fn url_encoding() {
        assert_eq!(encode_url_path("mods/copycats-3.0.7+mc.1.20.1-forge.jar"), "mods/copycats-3.0.7%2Bmc.1.20.1-forge.jar");
        assert_eq!(encode_url_path("config/my file.toml"), "config/my%20file.toml");
        assert_eq!(encode_url_path("mods/Без имени.jar"), "mods/%D0%91%D0%B5%D0%B7%20%D0%B8%D0%BC%D0%B5%D0%BD%D0%B8.jar");
    }
}
