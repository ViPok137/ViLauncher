//! Instance'ы: у каждого сервера свой клиент `.mc-launcher/instances/<instanceId>/`
//! (моды, config, options.txt, launch.json, версии, библиотеки). Общие для всех: `runtime/` (Java),
//! `shared/` (шейдеры и ресурспаки), ник и настройки лаунчера.

use crate::servers::is_valid_id;
use std::path::{Path, PathBuf};

pub fn instances_root(launcher_dir: &Path) -> PathBuf {
    launcher_dir.join("instances")
}

/// Каталог instance'а. Идентификатор проверяется: небезопасный → `None`.
pub fn instance_dir(launcher_dir: &Path, instance_id: &str) -> Option<PathBuf> {
    is_valid_id(instance_id).then(|| instances_root(launcher_dir).join(instance_id))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// Старого `client/` нет — делать нечего.
    NothingToDo,
    /// Старый клиент перенесён в instance (файлы целы, ничего не удалено).
    Moved,
    /// И `client/`, и целевой instance существуют — ничего не трогаем.
    TargetExists,
}

/// Мягкая миграция единственного клиента прежних версий: `.mc-launcher/client` → `instances/<target>`.
/// Это простое переименование каталога: настройки игрока (options.txt, config, сохранённые данные) сохраняются;
/// устаревшие файлы старой версии Minecraft потом заменит установка.
pub fn migrate_legacy_client(launcher_dir: &Path, target_instance: &str) -> std::io::Result<Migration> {
    let legacy = launcher_dir.join("client");
    let Some(target) = instance_dir(launcher_dir, target_instance) else {
        return Ok(Migration::NothingToDo);
    };
    if !legacy.is_dir() {
        return Ok(Migration::NothingToDo);
    }
    if target.exists() {
        return Ok(Migration::TargetExists);
    }
    std::fs::create_dir_all(instances_root(launcher_dir))?;
    std::fs::rename(&legacy, &target)?;
    Ok(Migration::Moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_are_validated() {
        let l = Path::new("/l");
        assert_eq!(instance_dir(l, "create"), Some(PathBuf::from("/l/instances/create")));
        for bad in ["", "..", "../x", "a/b", "A", "a b"] {
            assert_eq!(instance_dir(l, bad), None, "{bad:?}");
        }
    }

    #[test]
    fn legacy_client_is_moved_without_losing_files() {
        let d = tempfile::tempdir().unwrap();
        let l = d.path();
        assert_eq!(migrate_legacy_client(l, "create").unwrap(), Migration::NothingToDo);

        std::fs::create_dir_all(l.join("client/mods")).unwrap();
        std::fs::write(l.join("client/options.txt"), b"fov:0.5").unwrap();
        std::fs::write(l.join("client/mods/a.jar"), b"jar").unwrap();
        std::fs::write(l.join("client/launch.json"), b"{}").unwrap();
        assert_eq!(migrate_legacy_client(l, "create").unwrap(), Migration::Moved);
        assert!(!l.join("client").exists());
        assert_eq!(std::fs::read(l.join("instances/create/options.txt")).unwrap(), b"fov:0.5");
        assert_eq!(std::fs::read(l.join("instances/create/mods/a.jar")).unwrap(), b"jar");
        assert!(l.join("instances/create/launch.json").exists());
        // повторный вызов безопасен
        assert_eq!(migrate_legacy_client(l, "create").unwrap(), Migration::NothingToDo);

        // если целевой instance уже есть — старый клиент не трогаем
        std::fs::create_dir_all(l.join("client")).unwrap();
        std::fs::write(l.join("client/x"), b"1").unwrap();
        assert_eq!(migrate_legacy_client(l, "create").unwrap(), Migration::TargetExists);
        assert!(l.join("client/x").exists());
        // небезопасный id
        assert_eq!(migrate_legacy_client(l, "../evil").unwrap(), Migration::NothingToDo);
    }
}
