use std::io::{BufRead, Write};
use std::path::PathBuf;
use vilauncher_manifest::{build, check, write, Options};

const HELP: &str = "\
ViLauncher Manifest — считает SHA-1 и подстраивает manifest.json / hashes.json под файлы репозитория.

Использование:  vilauncher-manifest [ПАПКА] [опции]
  Без аргументов работает в текущей папке (положи .exe рядом с mods/, config/, installer-jar и дважды кликни).

Опции:
  --mc <версия>            версия Minecraft (по умолчанию — из имени installer'а, иначе из manifest.json)
  --version <x.y.z>        версия манифеста (по умолчанию +1 к patch, но только если что-то изменилось)
  --loader <файл.jar>      installer NeoForge/Forge в корне репозитория
                           (по умолчанию — уже указанный в манифесте, иначе самый свежий *-installer.jar)
  --loader-version <метка> метка версии загрузчика (по умолчанию из имени файла)
  --client-revision <N>    clientRevision (смена = очистка папки bin у игроков)
  --check                  только проверить репозиторий по manifest.json (код выхода 1 при расхождениях)
  --dry-run                показать, что изменится, ничего не записывая
  --no-pause               не ждать Enter в конце
  -h, --help               эта справка
";

fn pause_if_needed(pause: bool) {
    if pause {
        print!("\nНажми Enter для выхода...");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().lock().read_line(&mut s);
    }
}

fn run() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = Options { repo: PathBuf::from("."), ..Default::default() };
    let (mut dry, mut check_only) = (false, false);
    let mut it = args.iter();
    let take = |name: &str, it: &mut std::slice::Iter<String>| it.next().cloned().ok_or(format!("После {name} нужно значение"));
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(0);
            }
            "--mc" => o.mc = Some(take("--mc", &mut it)?),
            "--version" => o.version = Some(take("--version", &mut it)?),
            "--loader" => o.loader = Some(take("--loader", &mut it)?),
            "--loader-version" => o.loader_version = Some(take("--loader-version", &mut it)?),
            "--client-revision" => o.client_revision = Some(take("--client-revision", &mut it)?.parse().map_err(|_| "--client-revision: нужно число")?),
            "--check" => check_only = true,
            "--dry-run" => dry = true,
            "--no-pause" => {}
            p if !p.starts_with('-') => o.repo = PathBuf::from(p),
            other => return Err(format!("Неизвестная опция: {other}\n\n{HELP}")),
        }
    }
    o.repo = std::fs::canonicalize(&o.repo).map_err(|e| format!("Папка {}: {e}", o.repo.display()))?;
    println!("Репозиторий: {}", o.repo.display());

    if check_only {
        let c = check(&o.repo)?;
        println!("Совпадает с манифестом: {} файл(ов)", c.ok);
        for (title, list) in [("ОТСУТСТВУЮТ", &c.missing), ("ИЗМЕНЕНЫ (хеш/размер не совпадает)", &c.mismatched), ("ЛИШНИЕ моды (нет в манифесте)", &c.not_in_manifest)] {
            if !list.is_empty() {
                println!("\n{title}:");
                list.iter().for_each(|p| println!("  ! {p}"));
            }
        }
        if c.is_clean() {
            println!("\nВсё в порядке.");
            return Ok(0);
        }
        println!("\nЗапусти программу без --check, чтобы обновить манифест.");
        return Ok(1);
    }

    let rep = build(&o)?;
    println!("Моды: {} (manifest.json) | настройки по умолчанию: {} файл(ов) (defaults.json)", rep.mods, rep.defaults);
    for (sign, list) in [('+', &rep.added), ('-', &rep.removed), ('~', &rep.modified)] {
        for p in list {
            println!("  {sign} {p}");
        }
    }
    rep.notes.iter().for_each(|n| println!("  • {n}"));
    rep.warnings.iter().for_each(|w| println!("  ВНИМАНИЕ: {w}"));
    let m = &rep.manifest;
    println!(
        "\nmanifest: версия {} | Minecraft {} | загрузчик {}",
        m["version"].as_str().unwrap_or("?"),
        m["mcVersion"].as_str().unwrap_or("?"),
        m.pointer("/loader/installer").and_then(|v| v.as_str()).unwrap_or("— не задан")
    );
    if dry {
        println!("(--dry-run) {}", if rep.changed { "файлы были бы обновлены" } else { "изменений нет" });
        return Ok(0);
    }
    let written = write(&o.repo, &rep).map_err(|e| format!("Запись: {e}"))?;
    if written.is_empty() {
        println!("Изменений нет — файлы не тронуты.");
    } else {
        println!("Записано: {}", written.join(", "));
        println!("Теперь закоммить и запушь манифест вместе с файлами.");
    }
    Ok(0)
}

fn main() {
    // Двойной клик в Windows (аргументов нет) — не закрывать окно сразу.
    let pause = cfg!(windows) && std::env::args().len() == 1;
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ОШИБКА: {e}");
            2
        }
    };
    pause_if_needed(pause);
    std::process::exit(code);
}
