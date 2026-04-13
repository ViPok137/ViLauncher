# Быстрый старт

## Требования

| Компонент | Версия |
|-----------|--------|
| Windows | 10 / 11 (64-bit) |
| Node.js | 18 LTS или новее |
| Java | **17** (строго — Forge 47.x не работает на Java 20/21) |

> **Java 17** скачать: https://adoptium.net → Temurin 17 LTS

---

## Для игроков — установка лаунчера

1. Скачай последний `ViLauncher_Setup_vX.X.X.exe` из [Releases](https://github.com/ViPok137/ViLauncher/releases)
2. Запусти установщик — он установится в `%LOCALAPPDATA%\ViLauncher`
3. Войди с никнеймом и паролем
4. Нажми **УСТАНОВИТЬ** — лаунчер скачает Minecraft 1.20.1 + Forge (~500 МБ)
5. После установки нажми **ЗАПУСТИТЬ** 🎮

> Данные игры хранятся в `%LOCALAPPDATA%\ViLauncher\client\`

---

## Для разработчиков — запуск из исходников

```bash
# 1. Клонируй репозиторий
git clone https://github.com/ViPok137/ViLauncher.git
cd ViLauncher

# 2. Установи зависимости
npm install

# 3. Запусти в режиме разработки
npm start
```

### Отладка (DevTools)

В `src/main.js` раскомментируй строку после `win.loadFile(...)`:
```js
win.webContents.openDevTools();
```

---

## Структура данных игрока

```
%LOCALAPPDATA%\ViLauncher\
├── settings.json          ← никнейм, пароль (зашифр.), версия модов
├── minecraft.log          ← лог последнего запуска MC
├── launch-cmd.log         ← команда запуска Java (для отладки)
├── client\
│   ├── mods\              ← моды с GitHub
│   ├── config\            ← конфиги модов
│   ├── versions\          ← vanilla + forge version.json
│   ├── libraries\         ← все библиотеки
│   ├── assets\            ← текстуры, звуки (~300 МБ)
│   ├── natives\           ← нативные .dll файлы
│   └── launch.json        ← сохранённый classpath для запуска
└── jre17\                 ← bundled Java 17 (если скачан авто)
```
