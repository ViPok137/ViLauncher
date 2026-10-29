# ViLauncher — сборка с нуля (Windows)

Пошаговая инструкция для человека, который видит проект впервые. Команды — для **PowerShell**, если не сказано иное.
Проект: Tauri 2 + Rust (ядро `crates/vilauncher-core`), интерфейс на обычном JavaScript (`src/renderer`).

---

## 1. Что нужно установить

| Что | Версия | Зачем |
|---|---|---|
| Windows 10/11 x64 | — | целевая система сборки |
| **Git** | любая | клонирование |
| **Rust** (через rustup) | stable, **не ниже 1.77** (проверено на 1.91) | компиляция |
| **Visual Studio Build Tools 2022** → набор «Разработка классических приложений на C++» (MSVC + Windows SDK) | — | линковщик для Rust на Windows |
| **Node.js** | **20 LTS** или новее (npm идёт в комплекте) | Tauri CLI, сборка интерфейса |
| **WebView2 Runtime** | — | на Windows 11 уже есть; на Windows 10 установщик лаунчера поставит его сам, для сборки не нужен |

Установка:

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id OpenJS.NodeJS.LTS -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Закрой и снова открой PowerShell, затем проверь:

```powershell
rustc --version      # rustc 1.77 или новее
cargo --version
node --version       # v20 или новее
npm --version
git --version
```

Если `rustc` не найден — запусти `rustup default stable` и перезапусти терминал.

> Java, Minecraft и NeoForge для сборки **не нужны** — лаунчер сам скачивает нужную Java (17/21) при установке клиента.

## 2. Клонирование

```powershell
git clone https://github.com/ViPok137/ViLauncher.git
cd ViLauncher
```

## 3. Установка зависимостей

```powershell
npm ci                  # Tauri CLI, esbuild, локальные шрифты, jsdom (для тестов интерфейса)
```

Rust-зависимости (крейты) скачиваются автоматически при первой сборке. Системные пакеты на Windows не нужны.
Проверка, что всё на месте (быстро, без сборки приложения):

```powershell
npm run build:frontend        # собирает src\renderer\vendor\ (мост к Tauri + шрифты)
npm run check:config          # проверяет CSP, права, версии, мост ↔ команды Rust
npm run test:ui               # тесты интерфейса (jsdom)
npm run test:rust             # тесты ядра и программы манифеста (cargo test)
```

## 4. Ключи подписи обновлений (бесплатно, minisign/Ed25519)

Лаунчер обновляется сам: он скачивает обновление с GitHub Releases и **проверяет подпись** публичным ключом,
зашитым в приложение. Подписывать обновления нужно **своим** ключом — платный сертификат Authenticode не нужен.

**4.1. Сгенерируй пару ключей (один раз):**

```powershell
npx tauri signer generate -w $HOME\.tauri\vilauncher.key
```

Придумай пароль (без пароля: `npx tauri signer generate -w $HOME\.tauri\vilauncher.key --ci -p ""`). Недостающие папки создаются сами. Появятся два файла:

* `%USERPROFILE%\.tauri\vilauncher.key` — **приватный** ключ. Никому не показывать, **в репозиторий не коммитить**
  (`*.key` уже в `.gitignore`). Сохрани копию в надёжном месте: **если ключ потерян, выпустить обновление для уже
  установленных лаунчеров невозможно** — игрокам придётся ставить лаунчер заново.
* `%USERPROFILE%\.tauri\vilauncher.key.pub` — **публичный** ключ (одна строка текста).

**4.2. Публичный ключ → в репозиторий.** Открой `src-tauri\tauri.conf.json`, найди

```json
"plugins": { "updater": { "pubkey": "ВСТАВЬ_СЮДА_ПУБЛИЧНЫЙ_КЛЮЧ_из_tauri_signer_generate", ... } }
```

и замени значение на **всё содержимое** файла `vilauncher.key.pub` (одной строкой, без переносов). Закоммить.

**4.3. Приватный ключ → в GitHub Secrets** (репозиторий → *Settings → Secrets and variables → Actions → New repository secret*):

| Имя секрета | Значение |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | **содержимое** файла `vilauncher.key` (открой в Блокноте, скопируй всё) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | пароль от ключа. Если ключ создан **без** пароля (`--ci -p ""`) — этот секрет просто не создавай (GitHub не принимает пустые секреты) |
| `VILAUNCHER_MS_CLIENT_ID` | *необязательно*: Azure client id для входа через Microsoft (в игре сейчас не используется) |

Приватный ключ **никогда** не попадает в файлы проекта — только в переменные окружения (`TAURI_SIGNING_PRIVATE_KEY`).

## 5. Запуск в режиме разработки

```powershell
npm ci
npm run tauri:dev
```

* Первая сборка Rust занимает **5–15 минут** (дальше — секунды).
* Перед запуском автоматически выполняется `npm run build:frontend`. Правки в `src\renderer\*.js|css|html` подхватываются
  перезагрузкой страницы (F5), правки Rust — перезапуском `tauri dev`.
* Инструменты разработчика WebView2: правый клик → *Inspect* (или F12). Логи лаунчера: `%APPDATA%\.mc-launcher\app.log`.
* Для проверки обновлений нужен релиз на GitHub; в dev-режиме без ключа подписи проверка вернёт «Обновления не настроены» — это нормально.
* Данные лаунчера лежат в `%APPDATA%\.mc-launcher` (клиенты — в `instances\<id>\`, Java — в `runtime\`).

## 6. Локальная production-сборка

Приватный ключ нужен и здесь (иначе Tauri не создаст подписанные артефакты обновления):

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $HOME\.tauri\vilauncher.key -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "твой-пароль"
npm run tauri:build
```

Что получится (папка `target` — в корне репозитория):

| Файл | Что это |
|---|---|
| `target\release\bundle\nsis\ViLauncher_0.5.0_x64-setup.exe` | **установщик NSIS** — его раздавать игрокам (ставится в `%LOCALAPPDATA%\ViLauncher`, без прав администратора) |
| `target\release\bundle\nsis\ViLauncher_0.5.0_x64-setup.exe.sig` | подпись обновления (для `latest.json`) |
| `target\release\ViLauncher.exe` | «голый» exe лаунчера (интерфейс вшит внутрь) |

Программа для манифестов (хеши модов):

```powershell
cargo build --release -p vilauncher-manifest      # → target\release\vilauncher-manifest.exe
```

Обычно локальная сборка нужна только для проверки: релизы собирает GitHub Actions (раздел 8–9), он же создаёт `latest.json`.

## 7. Что лежит в `src-tauri\resources` и что больше не нужно

* **`src-tauri\resources` больше нет** — класть туда ничего не надо.
* **Старый Python-обновлятор (`ViLauncherUpdater`) и установщик Inno Setup в сборке больше не используются.** Файлы сохранены
  в папке `legacy\` только для истории. Обновления делает встроенный `tauri-plugin-updater`.
* Если у игроков стоит старая версия (установленная Inno Setup), перед установкой новой её нужно **удалить** (Параметры →
  Приложения → ViLauncher). Данные игры (`%APPDATA%\.mc-launcher`) при этом сохраняются: старый клиент автоматически
  переедет в `instances\create` без потери файлов.

## 8. GitHub Actions: настройка релизной сборки

1. Залей проект в репозиторий `ViPok137/ViLauncher` (ветка `main`).
2. Добавь секреты из пункта 4.3.
3. Убедись, что в `tauri.conf.json` вставлен публичный ключ (пункт 4.2) — иначе релизная сборка остановится с понятной ошибкой.
4. Workflow лежит в `.github\workflows\build.yml`: job `test` (тесты, проверка конфигурации) → job `release` (Windows: сборка,
   подпись, публикация). Ручной запуск (*Actions → build → Run workflow*) собирает установщик и кладёт его в «Artifacts» без релиза.

## 9. Как опубликовать релиз и обновление

1. Подними версию в **двух** местах (должны совпасть, это проверяется):
   * `src-tauri\tauri.conf.json` → `"version": "0.5.1"`
   * `src-tauri\Cargo.toml` → `version = "0.5.1"`
2. Закоммить и поставь тег с той же версией:

   ```powershell
   git add -A
   git commit -m "Релиз 0.5.1"
   git tag v0.5.1
   git push origin main --tags
   ```
3. GitHub Actions соберёт и создаст релиз **v0.5.1** с файлами: `ViLauncher_0.5.1_x64-setup.exe`, `….exe.sig`,
   **`latest.json`** и `vilauncher-manifest.exe`.
4. Установленные лаунчеры при запуске (и по кнопке *Настройки → Проверить обновления*) читают
   `https://github.com/ViPok137/ViLauncher/releases/latest/download/latest.json`, видят новую версию, показывают баннер
   с прогрессом, скачивают обновление, проверяют подпись, ставят и перезапускаются.
5. **Свой сервер обновлений позже:** добавь его адрес в `plugins.updater.endpoints` (массив) в `tauri.conf.json`,
   например `["https://updates.example.com/latest.json", "https://github.com/…/latest.json"]` — используется первый
   рабочий. Формат `latest.json` тот же (`version`, `notes`, `pub_date`, `platforms."windows-x86_64".{signature,url}`);
   только `https`.

> Релиз должен быть **опубликованным** (не draft и не pre-release), иначе ссылка `releases/latest` его не увидит.

## 10. Как включить сервер, добавить новый, обновить моды

Все серверы описаны в **`servers.json`** в корне репозитория. Лаунчер скачивает его при запуске (с кэшем на диске), поэтому
пересборка и новый релиз **не нужны**.

**Включить сейчас неактивный сервер** (например, «Колонизация»): в `servers.json` у нужного сервера поставь
`"active": true` и впиши `"address": "хост:порт"` → закоммить в `main`. Игрокам он станет доступен при следующем запуске
лаунчера. Выключить сервер обратно — `"active": false`.

**Добавить новый (5-й) сервер** — добавь объект в массив `servers`:

```json
{ "id": "tech", "name": "Технолайф", "description": "…", "icon": "🔧",
  "mcVersion": "1.21.1", "loader": "neoforge", "type": "modded",
  "instanceId": "tech", "address": "play.example.com:25565",
  "manifestUrl": "https://raw.githubusercontent.com/ViPok137/ViLauncher/main/servers/tech",
  "active": true, "subscription": false, "order": 5 }
```

* `id` и `instanceId` — латиница, цифры, `-`, `_` (до 32 символов), **уникальные**; два сервера на одной версии Minecraft
  обязаны иметь разные `instanceId` (у каждого свой каталог `instances\<instanceId>` и свой модпак).
* `type`: `modded` (моды + NeoForge/Forge) или `vanilla` (без модов; `mcVersion: "latest"` = всегда последняя версия).
* `manifestUrl` — папка с манифестами этого сервера. Для modded положи в неё `mods\`, `config\`, `options.txt` и
  `neoforge-…-installer.jar`, затем создай манифесты: `vilauncher-manifest.exe servers\tech` (создаст `manifest.json`,
  `defaults.json`, `hashes.json`). Для vanilla манифест не нужен.
* `subscription: true` — только метка в интерфейсе («по подписке»); доступ проверяет игровой сервер.
* Неподходящие записи (повтор `id`, плохой `manifestUrl` и т.п.) лаунчер пропускает и пишет причину в `app.log`.

> Запасная встроенная копия списка серверов — `crates\vilauncher-core\src\servers.default.json` (используется при первом
> запуске без интернета). Тест следит, чтобы она совпадала с корневым `servers.json`: меняя значения по умолчанию, обнови оба файла.

**Обновить моды Create:** положи новые `.jar` в `mods\` (старые удали), запусти `vilauncher-manifest.exe` в корне
репозитория, закоммить `manifest.json`, `defaults.json`, `hashes.json`. Игроки получат изменения при следующем запуске/нажатии «Играть».

## 11. Типичные ошибки сборки

| Симптом | Что проверить |
|---|---|
| `linker 'link.exe' not found` / `LNK1181` | не установлены Build Tools C++ (раздел 1); перезапусти терминал |
| `rustc … is too old` / `edition2024` | `rustup update stable` (нужен Rust ≥ 1.77, лучше свежий stable) |
| `npm ci` падает: «package.json and package-lock.json are not in sync» | выполни `npm install`, закоммить обновлённый `package-lock.json` |
| `Нет файла шрифта …` при `build:frontend` | не выполнен `npm ci` (шрифты берутся из `node_modules\@fontsource`) |
| Пустое окно / «не работают кнопки» | нет `src\renderer\vendor\bridge.js` → `npm run build:frontend`; смотри Console (F12) на ошибки CSP |
| `A public key has been found, but no private key` | не задана переменная `TAURI_SIGNING_PRIVATE_KEY` (раздел 6) |
| `failed to decode secret key` / `incorrect updater private key password` | неверный пароль или в секрет попал не весь файл `.key` |
| В Actions: «Не задан секрет TAURI_SIGNING_PRIVATE_KEY» | добавь секрет (раздел 4.3) |
| `check-config: updater.pubkey — заглушка` | вставь публичный ключ в `tauri.conf.json` (раздел 4.2) |
| `check-config: тег vX не совпадает с версией` | версии в `tauri.conf.json` и `Cargo.toml` должны равняться тегу без `v` |
| В лаунчере «signature verification failed» | публичный ключ в `tauri.conf.json` не от того приватного ключа, которым подписан релиз |
| Лаунчер пишет «Обновления не настроены» | в конфиге остался плейсхолдер публичного ключа, либо нет релиза с `latest.json` |
| `latest.json` отдаёт 404 | релиз draft/pre-release или репозиторий приватный; в `endpoints` другой адрес |
| `failed to bundle project … webview2` | нет интернета при сборке NSIS (скачивается загрузчик WebView2) |
| Несовпадение версий `tauri` и `@tauri-apps/api` | не обновляй крейт `tauri` без `npm i @tauri-apps/api@~X.Y @tauri-apps/cli@~X.Y` той же минорной версии |
| Windows SmartScreen предупреждает при установке | установщик не подписан сертификатом Authenticode (это платно и не используется): «Подробнее → Выполнить в любом случае» |
| Игра не запускается: «Изолированная Java … не найдена» | нажми «Установить» (Java 17/21 скачивается автоматически); проверь доступ к `api.adoptium.net` |
| Шейдеры/ресурспаки не применяются | симлинки на Windows требуют режима разработчика (Параметры → Для разработчиков) |
| Файл в репозитории > 100 МБ | GitHub не принимает такие файлы; крупные моды — через Git LFS или внешний `url` в манифесте |

Если что-то не получилось — приложи вывод команды и последние строки `%APPDATA%\.mc-launcher\app.log`.
