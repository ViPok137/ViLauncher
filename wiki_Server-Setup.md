# Настройка под свой сервер

## 1. Редактируй `src/config.js`

```js
module.exports = {
  SERVER_NAME:   'VIPOK SERVER',        // Название в лаунчере
  SERVER_IP:     'play.yourserver.gg',  // Адрес Playit.gg или домен
  SERVER_PORT:   25565,
  MC_VERSION:    '1.20.1',

  // GitHub репо с МОДАМИ (manifest.json, NEWS.json, папки mods/ config/)
  MODS_OWNER:    'ViPok137',
  MODS_REPO:     'ViLauncher',
  MODS_BRANCH:   'main',

  // GitHub репо ЛАУНЧЕРА (для самообновления через GitHub Releases)
  LAUNCHER_OWNER: 'ViPok137',
  LAUNCHER_REPO:  'ViLauncher',

  JAVA_PATH: 'java',   // или полный путь к java.exe
  RAM_MAX:   'auto',   // авто по RAM системы, или '4' для 4 ГБ
  RAM_MIN:   'auto',
};
```

## 2. Создай репозиторий с модами

Структура GitHub репо:
```
your-mods-repo/
├── manifest.json   ← список файлов + версия
├── NEWS.json       ← новости сервера
├── mods/
│   ├── create-1.20.1.jar
│   └── jei-1.20.1.jar
└── config/
    └── forge.cfg
```

### manifest.json

```json
{
  "version": "1.0.0",
  "mcVersion": "1.20.1",
  "files": [
    { "path": "mods/create-1.20.1.jar" },
    { "path": "mods/jei-1.20.1.jar" },
    { "path": "config/forge.cfg" }
  ]
}
```

> ⚠️ Чтобы обновить моды — добавь файлы и увеличь `"version"`.
> Лаунчер сравнивает строки версий при каждом запуске.

### NEWS.json

```json
[
  {
    "id": 1,
    "date": "2026-04-13",
    "title": "Сервер открыт!",
    "body": "Добро пожаловать! Подключайся через лаунчер."
  }
]
```

---

# Система модов

## Как работает автообновление

```
Запуск лаунчера
      │
      ▼
Скачать manifest.json с GitHub
      │
      ▼
manifest.version == settings.modsVersion?
      │              │
     Да             Нет
      │              │
      ▼              ▼
 Моды актуальны  Скачать все файлы
                 из manifest.files
                      │
                      ▼
               Сохранить новую версию
```

## Прогресс скачивания

Лаунчер показывает два прогресс-бара:
- **Верхний** — общий прогресс (файл X из N)
- **Нижний** — прогресс текущего файла в байтах

## Важные правила

- Имена файлов **не должны содержать пробелы** — GitHub Raw URL с пробелами не работает
- Репозиторий должен быть **публичным** для Raw-ссылок
- Версия в `manifest.json` — любая строка, главное чтобы менялась
