# Сборка EXE

## Требования для сборки

- Node.js 18+
- `npm install` выполнен
- Файл `assets/icon.ico` (256×256) — опционально

## Команды

```bash
npm run build:win    # Windows → dist/ViLauncher Setup X.X.X.exe
npm run build:linux  # Linux   → dist/ViLauncher-X.X.X.AppImage
```

Результат в папке `dist/`.

## Через Inno Setup (альтернатива)

1. Сначала собери `npm run build:win` — получишь папку `win-unpacked/`
2. Установи [Inno Setup 6](https://jrsoftware.org/isdl.php)
3. Открой `launcher_setup.iss`
4. Нажми **Build → Compile**
5. Готовый установщик: `installer_output/ViLauncher_Setup_vX.X.X.exe`

> Inno Setup создаёт более компактный установщик с поддержкой Unicode, автозапуска и проверки запущенного процесса.

---

# Выпуск обновления лаунчера

1. Измени `"version"` в `package.json` → `"1.0.1"`
2. Собери: `npm run build:win`
3. Создай релиз на GitHub:
   - **Tag**: `v1.0.1` (обязательно с `v`)
   - Прикрепи `.exe` из `dist/`
   - Нажми **Publish release**
4. При следующем запуске лаунчера у игроков появится жёлтый баннер обновления

---

# Устранение ошибок

## ZipException: zip END header not found

**Причина**: Файл `1.20.1.jar` или один из модов скачан повреждённым (прерванная загрузка).

**Решение**: Лаунчер теперь автоматически проверяет целостность `1.20.1.jar` перед запуском. Если файл повреждён — он удаляется и предлагается переустановка.

Вручную: удали `%LOCALAPPDATA%\ViLauncher\client` полностью и нажми **УСТАНОВИТЬ**.

---

## Module jopt.simple not found / Modules jopt.simple export conflict

**Причина**: В `--module-path` (-p) попадали лишние jar'ы сверх тех 8, что прописывает Forge.

**Решение** (уже исправлено в v1.x): `module-path` берётся строго из `forge version.json` и больше не дополняется.

---

## java.lang.NullPointerException в ImmediateWindowHandler

**Причина**: `--launchTarget forgeclient` не передавался в game args.

**Решение** (уже исправлено): Лаунчер читает `arguments.game` из `forge version.json` и передаёт их при запуске.

---

## Forge installer завершился с кодом 1

**Причина 1**: Отсутствует `launcher_profiles.json` — Forge installer проверяет его наличие.

**Решение**: Лаунчер создаёт файл-заглушку автоматически перед запуском installer.

**Причина 2**: Неправильная версия Java (нужна строго 17).

**Решение**: Проверь в настройках лаунчера какая Java используется. Установи [Temurin 17](https://adoptium.net).

---

## Лаунчер не видит обновление модов

**Причина**: Сравнение `manifest.version` происходило с учётом пробелов.

**Решение** (исправлено): Используется `String().trim()` при сравнении.

---

## Белый/чёрный экран при запуске лаунчера

**Решение**: Убедись что `%LOCALAPPDATA%\ViLauncher\settings.json` не повреждён. Если да — удали его, лаунчер пересоздаст.

---

# Changelog

## v1.0.0 (2026-04-13)
- Первый публичный релиз
- Установка Minecraft 1.20.1 + Forge 47.4.16
- Автообновление модов с GitHub
- Экран логина с «Запомнить меня»
- Скины из Aurora Launcher API
- Статус сервера (Minecraft Status Packet)
- Автоматическое определение RAM
- Aikar's GC flags
- Три вкладки серверов
- Установка в `%LOCALAPPDATA%\ViLauncher`

## v0.2.x (внутренние версии)
- Многочисленные исправления запуска Forge
- Фикс конфликтов module-path (jopt.simple, log4j)
- Фикс нативных библиотек LWJGL (только нужная архитектура)
- Авто-установка Java 17
