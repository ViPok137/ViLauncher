# legacy — не используется в сборке

Здесь лежит то, что осталось от Electron-версии и Python-обновлятора. Tauri-версия на это не опирается;
файлы сохранены только для истории и сверки логики.

* `electron/` — прежние `main.js`, `installer.js`, `preload.js`, `config.js`… (Electron 0.4.2).
* `ViLauncherUpdater.py`, `*.spec` — Python-обновлятор (PyInstaller) вместо которого теперь `tauri-plugin-updater`.
* `renderer-updater.js` — старый мини-обновлятор на Node.
