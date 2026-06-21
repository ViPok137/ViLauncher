'use strict';

const { app, BrowserWindow, ipcMain, Tray, Menu, nativeImage } = require('electron');
const path   = require('path');
const fs     = require('fs');
const https  = require('https');
const http   = require('http');
const os     = require('os');
const { spawn, execFile } = require('child_process');

const CFG       = require('./config');
const installer = require('./installer');

// ─── PATHS ───────────────────────────────────────────────────────────────────
const LAUNCHER_DIR = path.join(os.homedir(), 'AppData', 'Roaming', '.mc-launcher');
const CLIENT_DIR   = path.join(LAUNCHER_DIR, 'client');
const STORE_PATH   = path.join(LAUNCHER_DIR, 'settings.json');
const UPDATE_DIR   = path.join(LAUNCHER_DIR, 'update');
const LAUNCH_JSON  = path.join(CLIENT_DIR, 'launch.json'); // сохранённые данные запуска

// ─── ТЗ 2.6: electron-log — сквозное логирование вместо console.log ──────────
// Требует: npm install electron-log
// Если пакет не установлен (например в dev-среде без npm install) — graceful fallback на console
let log;
try {
  log = require('electron-log');
  fs.mkdirSync(LAUNCHER_DIR, { recursive: true });
  log.transports.file.resolvePathFn = () => path.join(LAUNCHER_DIR, 'app.log');
  log.transports.file.maxSize = 5 * 1024 * 1024; // 5 МБ — после этого архивация (ротация)
  log.transports.console.level = 'info';
  log.transports.file.level    = 'info';
  // Перехватываем необработанные ошибки и rejection
  log.errorHandler.startCatching();
} catch (e) {
  // electron-log не установлен — используем console как fallback,
  // но через единый интерфейс чтобы остальной код не менялся
  log = {
    info:  (...a) => console.log('[info]', ...a),
    warn:  (...a) => console.warn('[warn]', ...a),
    error: (...a) => console.error('[error]', ...a),
    debug: (...a) => console.log('[debug]', ...a),
  };
}

const RAW_URL = (owner, repo, branch, file) =>
  `https://raw.githubusercontent.com/${owner}/${repo}/${branch}/${file}`;
const GH_API = (owner, repo, endpoint) =>
  `https://api.github.com/repos/${owner}/${repo}/${endpoint}`;

// ─── STORE ───────────────────────────────────────────────────────────────────
function loadStore() {
  try { if (fs.existsSync(STORE_PATH)) return JSON.parse(fs.readFileSync(STORE_PATH, 'utf8')); }
  catch {}
  return {};
}
function saveStore(data) {
  fs.mkdirSync(LAUNCHER_DIR, { recursive: true });
  fs.writeFileSync(STORE_PATH, JSON.stringify(data, null, 2));
}

// ─── WINDOW ──────────────────────────────────────────────────────────────────
let win;
let tray = null;
let isQuitting = false; // ТЗ 3.6: различаем "свернуть в трей" от настоящего выхода

function createWindow() {
  win = new BrowserWindow({
    width: 1100, height: 680,
    minWidth: 900, minHeight: 580,
    frame: false, transparent: true,
    backgroundColor: '#00000000',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));

  // ТЗ 3.6: при нажатии на крестик — сворачиваем в трей, не завершаем процесс
  win.on('close', (e) => {
    if (isQuitting) return; // настоящий выход через трей/Exit — не перехватываем
    e.preventDefault();
    win.hide();
  });
}

function createTray() {
  // Пытаемся загрузить иконку лаунчера; если файла нет — используем пустую (не падаем)
  let icon;
  try {
    const iconPath = path.join(__dirname, '..', 'assets', 'icon.ico');
    icon = fs.existsSync(iconPath) ? nativeImage.createFromPath(iconPath) : nativeImage.createEmpty();
  } catch {
    icon = nativeImage.createEmpty();
  }

  tray = new Tray(icon);
  tray.setToolTip(CFG.SERVER_NAME ? `${CFG.SERVER_NAME} — ViLauncher` : 'ViLauncher');

  const contextMenu = Menu.buildFromTemplate([
    {
      label: 'Открыть',
      click: () => {
        if (win) { win.show(); win.focus(); }
      },
    },
    { type: 'separator' },
    {
      label: 'Выход',
      click: () => {
        isQuitting = true;
        app.quit();
      },
    },
  ]);
  tray.setContextMenu(contextMenu);

  // Клик по иконке трея — открыть/скрыть окно
  tray.on('click', () => {
    if (!win) return;
    if (win.isVisible()) { win.hide(); }
    else { win.show(); win.focus(); }
  });
}

app.whenReady().then(() => {
  createWindow();
  createTray();   // ТЗ 3.6
  initDiscord();  // ТЗ 3.3
});

// ТЗ 3.6: не завершаем процесс при закрытии последнего окна — трей держит приложение живым
app.on('window-all-closed', () => {
  // На Windows/Linux ничего не делаем — окно скрыто (close event), трей активен.
  // На macOS поведение тоже не завершает процесс автоматически.
});

// Явный сигнал "действительно выходим" — устанавливаем флаг перед закрытием окна программно
app.on('before-quit', () => { isQuitting = true; });

// ─── WINDOW CONTROLS ─────────────────────────────────────────────────────────
ipcMain.on('win-min',   () => win.minimize());
ipcMain.on('win-max',   () => win.isMaximized() ? win.unmaximize() : win.maximize());
ipcMain.on('win-close', () => win.close());

// ─── NICKNAME ────────────────────────────────────────────────────────────────
// ─── APP INFO ────────────────────────────────────────────────────────────────
ipcMain.handle('app-info', () => {
  const store = loadStore();
  // ТЗ 2.5: единственный источник Java — изолированная runtime/java17/
  const bundled = path.join(LAUNCHER_DIR, 'runtime', 'java17', 'bin',
    process.platform === 'win32' ? 'java.exe' : 'java');
  const javaPath  = bundled;
  const javaLabel = fs.existsSync(bundled)
    ? 'Изолированная Java 17 (portable)'
    : 'Не установлена — нажми «Установить»';
  const totalRam = (os.totalmem() / 1024 / 1024 / 1024).toFixed(1);
  const ramArgs  = autoRam();
  const xmx = ramArgs.find(a => a.startsWith('-Xmx')) || '';
  return {
    version:     app.getVersion(),
    modsVersion: store.modsVersion || null,
    electron:    process.versions.electron,
    os:          process.platform === 'win32' ? 'Windows' : process.platform,
    java:        javaLabel,
    ram:         `${xmx.replace('-Xmx','')} / ${totalRam} ГБ всего`,
  };
});

ipcMain.handle('nick-get', () => loadStore().nickname || '');
ipcMain.handle('nick-set', (_, nick) => {
  const s = loadStore(); s.nickname = nick; saveStore(s); return { ok: true };
});

// ─── NEWS ────────────────────────────────────────────────────────────────────
ipcMain.handle('news-fetch', async () => {
  try {
    const txt = await fetchText(RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'NEWS.json'));
    return JSON.parse(txt);
  } catch {
    return [{ id: 0, date: '—', title: 'Нет связи с GitHub', body: 'Проверь интернет.' }];
  }
});

// ─── LAUNCHER SELF-UPDATE ────────────────────────────────────────────────────
ipcMain.handle('launcher-check', async () => {
  try {
    const txt  = await fetchText(GH_API(CFG.LAUNCHER_OWNER, CFG.LAUNCHER_REPO, 'releases/latest'));
    const rel  = JSON.parse(txt);
    const remote = (rel.tag_name || '').replace(/^v/, '');
    const local  = app.getVersion();
    const asset  = (rel.assets || []).find(a => {
      if (process.platform === 'win32') return a.name.endsWith('.exe');
      if (process.platform === 'linux') return a.name.endsWith('.AppImage');
      return false;
    });
    return { hasUpdate: remote !== local && !!remote, remote, local, url: asset?.browser_download_url || null };
  } catch (e) { return { hasUpdate: false, error: e.message }; }
});

ipcMain.handle('launcher-update', async (_, { url }) => {
  if (!url) return { ok: false, error: 'Нет ссылки' };
  try {
    fs.mkdirSync(UPDATE_DIR, { recursive: true });
    const ext  = process.platform === 'win32' ? '.exe' : '.AppImage';
    const dest = path.join(UPDATE_DIR, `update${ext}`);
    await dlProgress(url, dest, (done, total) => {
      win.webContents.send('launcher-dl-progress', { pct: total > 0 ? Math.round(done / total * 100) : 0 });
    });
    if (process.platform === 'linux') fs.chmodSync(dest, '755');
    if (process.platform === 'win32') execFile(dest, ['/S'], { detached: true });
    else spawn(dest, ['--update'], { detached: true, stdio: 'ignore' }).unref();
    app.quit();
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});


// ─── AUTH ────────────────────────────────────────────────────────────────────
ipcMain.handle('auth-login', (_, { username, password, remember }) => {
  if (!username || !password) return { ok: false, error: 'Введи логин и пароль' };
  // TODO: проверка с сервером — пока принимаем любые данные
  const store = loadStore();
  store.username = username;
  if (remember) { store.savedUser = username; store.savedPass = password; }
  else { delete store.savedUser; delete store.savedPass; }
  saveStore(store);
  return { ok: true, username };
});

ipcMain.handle('auth-logout', () => {
  const store = loadStore();
  delete store.username;
  saveStore(store);
  return { ok: true };
});

ipcMain.handle('auth-get-saved', () => {
  const s = loadStore();
  return { username: s.savedUser || '', password: s.savedPass || '' };
});

ipcMain.handle('auth-get-user', () => loadStore().username || null);

// ─── SKIN / CAPE ──────────────────────────────────────────────────────────────
// Путь к кастомному скину игрока (если загружен вручную)
const CUSTOM_SKIN_PATH = path.join(LAUNCHER_DIR, 'custom_skin.png');

ipcMain.handle('skin-get', async (_, username) => {
  try {
    // Если есть кастомный скин — отдаём его, к API НЕ обращаемся
    if (fs.existsSync(CUSTOM_SKIN_PATH)) {
      const buf = fs.readFileSync(CUSTOM_SKIN_PATH);
      return { skinUrl: `data:image/png;base64,${buf.toString('base64')}`, isCustom: true };
    }

    const skinUrl = `https://api.aurora-launcher.ru/mojang/username/skin/${encodeURIComponent(username)}`;

    // Скачиваем скин в main процессе и возвращаем как data:URL
    // (из renderer CORS блокирует canvas.drawImage на внешние URL)
    const dataUrl = await new Promise((resolve) => {
      https.get(skinUrl, { headers: { 'User-Agent': 'ViLauncher/1.0' } }, res => {
        if (res.statusCode === 204 || res.statusCode === 404) {
          return resolve(null); // скин не задан
        }
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          https.get(res.headers.location, { headers: { 'User-Agent': 'ViLauncher/1.0' } }, res2 => {
            const chunks = [];
            res2.on('data', c => chunks.push(c));
            res2.on('end', () => {
              if (res2.statusCode >= 400) return resolve(null);
              resolve(`data:image/png;base64,${Buffer.concat(chunks).toString('base64')}`);
            });
          }).on('error', () => resolve(null));
          return;
        }
        if (res.statusCode >= 400) return resolve(null);
        const chunks = [];
        res.on('data', c => chunks.push(c));
        res.on('end', () => resolve(`data:image/png;base64,${Buffer.concat(chunks).toString('base64')}`));
      }).on('error', () => resolve(null));
    });

    return { skinUrl: dataUrl, isCustom: false };
  } catch { return { skinUrl: null, isCustom: false }; }
});

// Загрузка кастомного скина — открываем диалог выбора файла
ipcMain.handle('skin-upload', async () => {
  try {
    const { dialog } = require('electron');
    const result = await dialog.showOpenDialog(win, {
      title: 'Выбери файл скина (PNG, 64x64)',
      filters: [{ name: 'PNG изображения', extensions: ['png'] }],
      properties: ['openFile'],
    });
    if (result.canceled || !result.filePaths[0]) return { ok: false };

    const srcPath = result.filePaths[0];
    // Проверяем что это валидный PNG (сигнатура 89 50 4E 47)
    const header = fs.readFileSync(srcPath).subarray(0, 8);
    const pngSig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    if (!header.equals(pngSig)) {
      return { ok: false, error: 'Файл не является валидным PNG' };
    }

    fs.copyFileSync(srcPath, CUSTOM_SKIN_PATH);
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Сброс кастомного скина — возвращаемся к API
ipcMain.handle('skin-reset', async () => {
  try {
    fs.unlinkSync(CUSTOM_SKIN_PATH);
    return { ok: true };
  } catch { return { ok: true }; } // файла и не было — тоже ок
});

// ─── INSTALL CHECK ────────────────────────────────────────────────────────────
// Проверяем: установлен ли Minecraft+Forge (есть ли launch.json)
ipcMain.handle('install-check', () => {
  const installed = fs.existsSync(LAUNCH_JSON);
  const store     = loadStore();
  return { installed, modsVersion: store.modsVersion || null };
});

// ─── INSTALL MINECRAFT + FORGE ────────────────────────────────────────────────
ipcMain.handle('install-start', async () => {
  try {
    fs.mkdirSync(CLIENT_DIR, { recursive: true });
    // Всегда удаляем старый launch.json чтобы он пересоздался с актуальным modulePath
    try { fs.unlinkSync(LAUNCH_JSON); } catch {}

    await installer.install(CLIENT_DIR, (phase, done, total, name) => {
      const phaseNames = {
        java:      '☕ Изолированная Java 17',
        manifest:  '📋 Манифест',
        client:    '📦 Клиент',
        libraries: '📚 Библиотеки',
        assets:    '🖼 Ресурсы',
        forge:     '⚙ Forge (2-3 мин)',
        done:      '✅ Готово',
      };
      win.webContents.send('install-progress', {
        phase: phaseNames[phase] || phase,
        done, total, name,
        pct: total > 0 ? Math.round(done / total * 100) : 0,
      });
    });

    return { ok: true };
  } catch (e) {
    return { ok: false, error: e.message };
  }
});

// ─── MODS SYNC ────────────────────────────────────────────────────────────────
// ТЗ 2.7: вычисляем SHA1 локального файла для сравнения с hashes.json
function sha1OfFile(filePath) {
  try {
    const buf = fs.readFileSync(filePath);
    return require('crypto').createHash('sha1').update(buf).digest('hex');
  } catch { return null; }
}

ipcMain.handle('mods-sync', async () => {
  try {
    win.webContents.send('mods-status', { text: 'Получаю хэш-манифест с GitHub...' });

    // ТЗ 2.7: hashes.json — пары 'имя_файла': 'sha1_хэш', источник истины для дельта-синхронизации
    let hashesTxt;
    try {
      hashesTxt = await fetchText(RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'hashes.json'));
    } catch (e) {
      log.warn('[mods-sync] hashes.json недоступен, fallback на manifest.json:', e.message);
      hashesTxt = null;
    }

    // manifest.json остаётся источником версии и списка файлов вне mods/ (config/, options.txt)
    let manifestTxt;
    try {
      manifestTxt = await fetchText(RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'manifest.json'));
    } catch (e) {
      return { ok: false, error: 'Не удалось получить manifest.json: ' + e.message };
    }
    const manifest = JSON.parse(manifestTxt);
    const store    = loadStore();

    if (String(manifest.version).trim() === String(store.modsVersion || '').trim()) {
      return { ok: true, updated: false, msg: `Моды актуальны (${store.modsVersion})` };
    }

    const modsDir = path.join(CLIENT_DIR, 'mods');
    fs.mkdirSync(modsDir, { recursive: true });

    if (hashesTxt) {
      // ── Дельта-режим: сравниваем хэши, качаем только изменённое ──
      const hashes = JSON.parse(hashesTxt); // { "filename.jar": "sha1hash", ... }
      const remoteNames = Object.keys(hashes);

      // Считаем какие файлы нужно скачать (новые или изменённые)
      const toDownload = remoteNames.filter(fname => {
        const localPath = path.join(modsDir, fname);
        if (!fs.existsSync(localPath)) return true; // отсутствует
        const localHash = sha1OfFile(localPath);
        return localHash !== hashes[fname]; // изменился
      });

      // Удаляем локальные файлы которых нет в манифесте (лишние/устаревшие)
      const removed = [];
      if (fs.existsSync(modsDir)) {
        for (const localFile of fs.readdirSync(modsDir)) {
          if (!localFile.endsWith('.jar')) continue;
          if (!remoteNames.includes(localFile)) {
            try { fs.unlinkSync(path.join(modsDir, localFile)); removed.push(localFile); }
            catch {}
          }
        }
      }
      if (removed.length) log.info('[mods-sync] Удалены лишние моды:', removed.join(', '));

      win.webContents.send('mods-status', {
        text: `Дельта-синхронизация: ${toDownload.length} изменённых из ${remoteNames.length} модов`
      });

      for (let i = 0; i < toDownload.length; i++) {
        const fname = toDownload[i];
        const url   = RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'mods/' + fname);
        const dest  = path.join(modsDir, fname);

        win.webContents.send('mods-progress', {
          fileIndex: i, total: toDownload.length, name: fname, filePct: 0, fileDone: 0, fileSize: 0,
        });
        await dlFileProgress(url, dest, (fileDone, fileSize) => {
          win.webContents.send('mods-progress', {
            fileIndex: i, total: toDownload.length, name: fname,
            filePct: fileSize > 0 ? Math.round(fileDone / fileSize * 100) : 0,
            fileDone, fileSize,
          });
        });
        win.webContents.send('mods-progress', {
          fileIndex: i + 1, total: toDownload.length, name: fname, filePct: 100, done: true,
        });
      }

      // Дополнительные файлы из manifest.json вне mods/ (config/, options.txt и т.д.)
      // — синкаем как раньше, без хэш-дельты (обычно немного и редко меняются)
      const extraFiles = (manifest.files || []).filter(f => !f.path.startsWith('mods/'));
      for (const f of extraFiles) {
        const url  = RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, f.path);
        const dest = path.join(CLIENT_DIR, f.path);
        fs.mkdirSync(path.dirname(dest), { recursive: true });
        try { await dlFileProgress(url, dest, () => {}); } catch (e) { log.warn('[mods-sync] extra file failed:', f.path, e.message); }
      }

    } else {
      // ── Fallback: старый режим без хэшей, качаем все файлы манифеста целиком ──
      const files = manifest.files || [];
      win.webContents.send('mods-status', { text: `Найдено ${files.length} файлов для обновления` });

      for (let i = 0; i < files.length; i++) {
        const f     = files[i];
        const fname = path.basename(f.path);
        const url   = RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, f.path);
        const dest  = path.join(CLIENT_DIR, f.path);
        fs.mkdirSync(path.dirname(dest), { recursive: true });

        win.webContents.send('mods-progress', {
          fileIndex: i, total: files.length, name: fname, filePct: 0, fileDone: 0, fileSize: 0,
        });
        await dlFileProgress(url, dest, (fileDone, fileSize) => {
          win.webContents.send('mods-progress', {
            fileIndex: i, total: files.length, name: fname,
            filePct: fileSize > 0 ? Math.round(fileDone / fileSize * 100) : 0,
            fileDone, fileSize,
          });
        });
        win.webContents.send('mods-progress', {
          fileIndex: i + 1, total: files.length, name: fname, filePct: 100, done: true,
        });
      }
    }

    store.modsVersion = manifest.version;
    saveStore(store);
    return { ok: true, updated: true, version: manifest.version };
  } catch (e) {
    log.error('[mods-sync] Ошибка:', e.message);
    return { ok: false, error: e.message };
  }
});

// ─── LAUNCH ──────────────────────────────────────────────────────────────────
let mcProcess = null; // Текущий запущенный процесс Minecraft (одиночный запуск)

ipcMain.handle('game-launch', async (_, { nickname }) => {
  try {
    // Не даём запустить второй процесс Minecraft одновременно
    if (mcProcess !== null) {
      return { ok: false, error: 'Minecraft уже запущен! Закрой игру перед повторным запуском.' };
    }

    if (!fs.existsSync(LAUNCH_JSON))
      return { ok: false, error: 'Minecraft не установлен. Нажми «Установить».' };

    const launch   = JSON.parse(fs.readFileSync(LAUNCH_JSON, 'utf8'));
    // ТЗ 2.5: системная Java полностью игнорируется. Единственный источник —
    // изолированная портативная сборка в %APPDATA%/.mc-launcher/runtime/java17/
    const javaPath = path.join(LAUNCHER_DIR, 'runtime', 'java17', 'bin',
      process.platform === 'win32' ? 'java.exe' : 'java');
    const sep      = process.platform === 'win32' ? ';' : ':';

    // Проверяем что java существует
    if (!fs.existsSync(javaPath)) {
      return { ok: false, error: `Изолированная Java 17 не найдена: ${javaPath}\nНажми «Переустановить клиент» в настройках.` };
    }

    // Проверяем classpath
    const missing = launch.classpath.filter(p => !fs.existsSync(p));
    if (missing.length > 0) {
      return { ok: false, error: `Отсутствуют файлы classpath (${missing.length} шт). Переустанови клиент.` };
    }

    // Проверяем целостность client.jar (1.20.1.jar) — частая причина ZipException
    const clientJarPath = launch.classpath.find(p => p.endsWith('1.20.1.jar') && p.includes('versions'));
    if (clientJarPath && fs.existsSync(clientJarPath)) {
      const buf = fs.readFileSync(clientJarPath);
      // ZIP должен заканчиваться на сигнатуру 0x06054b50
      let zipOk = false;
      for (let i = buf.length - 22; i >= Math.max(0, buf.length - 65536); i--) {
        if (buf[i] === 0x50 && buf[i+1] === 0x4b && buf[i+2] === 0x05 && buf[i+3] === 0x06) {
          zipOk = true; break;
        }
      }
      if (!zipOk) {
        try { fs.unlinkSync(clientJarPath); } catch {}
        // Удаляем launch.json чтобы форсировать переустановку
        try { fs.unlinkSync(LAUNCH_JSON); } catch {}
        return { ok: false, error: 'Файл 1.20.1.jar повреждён. Удалён автоматически — нажми Установить для переустановки.' };
      }
    }

    // Проверяем целостность jar'ов в папке mods/ — ZipException если повреждён
    const modsDir = path.join(launch.gameDir, 'mods');
    if (fs.existsSync(modsDir)) {
      const badMods = [];
      for (const f of fs.readdirSync(modsDir)) {
        if (!f.endsWith('.jar')) continue;
        const fpath = path.join(modsDir, f);
        try {
          const buf = fs.readFileSync(fpath);
          // ZIP должен заканчиваться на сигнатуру END 0x06054b50
          // Ищем её в последних 64KB файла
          const tail = buf.slice(Math.max(0, buf.length - 65536));
          let found = false;
          for (let i = tail.length - 4; i >= 0; i--) {
            if (tail[i] === 0x50 && tail[i+1] === 0x4b && tail[i+2] === 0x05 && tail[i+3] === 0x06) {
              found = true; break;
            }
          }
          if (!found) badMods.push(f);
        } catch { badMods.push(f); }
      }
      if (badMods.length > 0) {
        // Удаляем повреждённые файлы чтобы при следующем запуске лаунчер их перекачал
        for (const f of badMods) {
          try { fs.unlinkSync(path.join(modsDir, f)); } catch {}
        }
        // Сбрасываем версию модов чтобы триггернуть перекачку
        const store = loadStore();
        delete store.modsVersion;
        saveStore(store);
        return { ok: false, error: `Повреждены моды (${badMods.length} шт): ${badMods.slice(0, 3).join(', ')}...
Файлы удалены — перезапусти лаунчер для перекачки.` };
      }
    }

    const logPath = path.join(LAUNCHER_DIR, 'minecraft.log');
    const logFile = fs.openSync(logPath, 'w');

    // JVM аргументы — Forge сам прописывает -p, -DlegacyClassPath и --add-modules в version.json
    const forgeJvmArgs = launch.forgeJvmArgs || [];

    // module-path из launch.json (извлечён из forge version.json при установке)
    const modulePath = (launch.modulePath || []).join(sep);

    const args = [
      // Автоматическое выделение памяти на основе RAM системы
      ...autoRam(),
      `-Djava.library.path=${launch.nativesDir}`,
      `-Dminecraft.launcher.brand=mc-launcher`,
      `-Dminecraft.launcher.version=1.0`,
      // Отключаем ранний экран Forge (и как system property, и как program arg ниже)
      '-Dfml.earlyprogresswindow=true',
      // ignoreList — список jar'ов которые Forge НЕ должен открывать как модули
      // Без этого Forge пытается загрузить client.jar как мод и падает
      `-DignoreList=${path.basename(launch.classpath.find(p => p.includes(installer.MC_VERSION + '.jar')) || '')}`,
      // Открытия модулей для Forge 47.x
      '--add-opens', 'java.base/java.util.jar=ALL-UNNAMED',
      '--add-opens', 'java.base/java.lang.invoke=ALL-UNNAMED',
      '--add-opens', 'java.base/java.lang=ALL-UNNAMED',
      '--add-opens', 'java.desktop/sun.awt.image=ALL-UNNAMED',
      '--add-opens', 'java.base/sun.security.util=ALL-UNNAMED',
      '--add-opens', 'java.base/java.net=ALL-UNNAMED',
      // module-path — securejarhandler, bootstraplauncher и др.
      ...(modulePath ? ['-p', modulePath] : []),
      ...(modulePath ? ['--add-modules', 'ALL-MODULE-PATH'] : []),
      // JVM args от Forge (-DlegacyClassPath, -DlibraryDirectory и т.д.)
      ...forgeJvmArgs,
      '-cp', launch.classpath.join(sep),
      launch.mainClass,
      // Vanilla game args
      '--username',    nickname || CFG.DEFAULT_USERNAME,
      '--version',     installer.MC_VERSION,
      '--gameDir',     launch.gameDir,
      '--assetsDir',   launch.assetsDir,
      '--assetIndex',  launch.assetIndex,
      '--accessToken', 'null',
      '--userType',    'legacy',
      // Forge game args: --launchTarget forgeclient, --fml.forgeVersion и др.
      // БЕЗ ЭТОГО Forge не знает что запускать → NPE в ImmediateWindowHandler
      ...(launch.forgeGameArgs || []),
      // ТЗ 1.1: флаги и значения — НЕЗАВИСИМЫЕ элементы массива, не единый литерал
      '--server', CFG.SERVER_IP,
      '--port',   String(CFG.SERVER_PORT),
    ];

    // Убираем null/undefined и дедуплицируем ПАРНЫЕ аргументы (флаг + значение)
    // Важно: --add-modules ALL-MODULE-PATH — это пара, нельзя дедупить по отдельности
    const rawArgs = args.filter(a => a !== undefined && a !== null && a !== '').map(String);
    const cleanArgs = [];
    const seenPairs = new Set(); // ключ = "флаг|значение"

    for (let i = 0; i < rawArgs.length; i++) {
      const a = rawArgs[i];
      // Парные флаги без = : следующий аргумент — значение
      const isPairedFlag = (a === '--add-modules' || a === '--add-opens' ||
                            a === '--add-exports' || a === '--add-reads' ||
                            a === '-p' || a === '--module-path');
      if (isPairedFlag && i + 1 < rawArgs.length) {
        const val = rawArgs[i + 1];
        const key = a + '|' + val;
        if (!seenPairs.has(key)) {
          seenPairs.add(key);
          cleanArgs.push(a, val);
        }
        i++; // пропускаем значение
      } else {
        cleanArgs.push(a);
      }
    }

    // Логируем команду запуска для отладки
    const cmdLog = path.join(LAUNCHER_DIR, 'launch-cmd.log');
    try { fs.writeFileSync(cmdLog, javaPath + '\n' + cleanArgs.join('\n')); } catch {}

    const child = spawn(javaPath, cleanArgs, {
      detached:    true,
      stdio:       ['ignore', logFile, logFile],
      cwd:         launch.gameDir,
      windowsHide: true,   // ← скрывает cmd на Windows
    });

    mcProcess = child; // ТЗ: одиночный запуск — запоминаем процесс
    win.webContents.send('mc-running', { running: true });

    // ТЗ 3.3: статус Discord — игрок начал игру
    try {
      const pingResult = await mcServerUtil
        ? mcServerUtil.status(CFG.SERVER_IP, CFG.SERVER_PORT, { timeout: 2000 }).catch(() => null)
        : null;
      setDiscordPlaying(pingResult?.players?.online ?? null);
    } catch {}

    child.on('error', err => {
      fs.closeSync(logFile);
      mcProcess = null;
      win.webContents.send('mc-running', { running: false });
      win.webContents.send('game-error', { error: 'Не удалось запустить Java: ' + err.message });
    });

    // Если процесс упал сразу (< 5 сек) — читаем лог и показываем ошибку
    child.on('exit', (code) => {
      fs.closeSync(logFile);
      mcProcess = null;
      if (win && !win.isDestroyed()) {
        win.webContents.send('mc-running', { running: false });
      }
      setDiscordLauncher(); // ТЗ 3.3: статус Discord — вернулись в лаунчер
      if (code !== 0 && code !== null) {
        try {
          const logTail = fs.readFileSync(logPath, 'utf8').slice(-2000);
          const lastLines = logTail.split('\n').slice(-15).join('\n');
          if (win && !win.isDestroyed()) {
            win.webContents.send('game-error', { error: `Java завершилась с кодом ${code}.\nЛог: ${lastLines}` });
          }
        } catch {}
      }
    });

    child.unref();
    return { ok: true, logPath };
  } catch (e) { return { ok: false, error: e.message }; }
});



// ─── SETTINGS ACTIONS ────────────────────────────────────────────────────────
// Переустановка клиента — удаляем папку client целиком
ipcMain.handle('reinstall-client', async () => {
  try {
    if (fs.existsSync(CLIENT_DIR)) {
      fs.rmSync(CLIENT_DIR, { recursive: true, force: true });
    }
    const store = loadStore();
    delete store.modsVersion;
    saveStore(store);
    log.info('[settings] Клиент удалён для переустановки');
    return { ok: true };
  } catch (e) { log.error('[settings] reinstall-client:', e.message); return { ok: false, error: e.message }; }
});

// Проверка модов вручную — сбрасываем версию чтобы триггернуть перекачку
ipcMain.handle('check-mods', async () => {
  try {
    const store = loadStore();
    delete store.modsVersion;
    saveStore(store);
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Открыть папку лаунчера в Проводнике
ipcMain.handle('open-launcher-dir', () => {
  try {
    require('electron').shell.openPath(LAUNCHER_DIR);
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// ─── DISCORD RPC (ТЗ 3.3) ────────────────────────────────────────────────────
// Требует: npm install discord-rpc
// CLIENT_ID создаётся на https://discord.com/developers/applications
let discordRpc    = null;
let discordReady  = false;
const DISCORD_CLIENT_ID = CFG.DISCORD_CLIENT_ID || '1234567890123456789'; // ← задай свой в config.js

async function initDiscord() {
  try {
    const RPC = require('discord-rpc');
    discordRpc = new RPC.Client({ transport: 'ipc' });

    discordRpc.on('ready', () => {
      discordReady = true;
      log.info('[discord] RPC подключён');
      setDiscordLauncher();
    });
    discordRpc.on('disconnected', () => { discordReady = false; });

    await discordRpc.login({ clientId: DISCORD_CLIENT_ID });
  } catch (e) {
    log.warn('[discord] RPC недоступен (discord-rpc не установлен или Discord не запущен):', e.message);
    discordRpc = null;
  }
}

function setDiscordLauncher() {
  if (!discordReady || !discordRpc) return;
  try {
    discordRpc.setActivity({
      details:        `В лаунчере ${CFG.SERVER_NAME}`,
      state:          `Minecraft ${CFG.MC_VERSION} Forge`,
      largeImageKey:  'launcher_logo',
      largeImageText: 'ViPok Launcher',
      smallImageKey:  'minecraft_icon',
      smallImageText: `Minecraft ${CFG.MC_VERSION}`,
      startTimestamp: Math.floor(Date.now() / 1000),
      buttons: [
        { label: 'Скачать лаунчер', url: `https://github.com/${CFG.LAUNCHER_OWNER}/${CFG.LAUNCHER_REPO}/releases` },
      ],
    });
  } catch (e) { log.warn('[discord] setDiscordLauncher:', e.message); }
}

function setDiscordPlaying(playerCount) {
  if (!discordReady || !discordRpc) return;
  try {
    discordRpc.setActivity({
      details:        `Играет на ${CFG.SERVER_NAME}`,
      state:          playerCount != null ? `Онлайн: ${playerCount} игроков` : `Minecraft ${CFG.MC_VERSION} Forge`,
      largeImageKey:  'minecraft_icon',
      largeImageText: `Minecraft ${CFG.MC_VERSION} Forge`,
      smallImageKey:  'launcher_logo',
      smallImageText: 'ViPok Launcher',
      startTimestamp: Math.floor(Date.now() / 1000),
      buttons: [
        { label: 'Скачать лаунчер', url: `https://github.com/${CFG.LAUNCHER_OWNER}/${CFG.LAUNCHER_REPO}/releases` },
      ],
    });
  } catch (e) { log.warn('[discord] setDiscordPlaying:', e.message); }
}

ipcMain.handle('discord-set-playing',  (_, { playerCount }) => { setDiscordPlaying(playerCount); return { ok: true }; });
ipcMain.handle('discord-set-launcher', () => { setDiscordLauncher(); return { ok: true }; });

// ─── MODRINTH (шейдеры и ресурспаки) ─────────────────────────────────────────
// API без ключа: https://docs.modrinth.com/api/
const MODRINTH_API = 'https://api.modrinth.com/v2';

function modrinthFetch(url) {
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'ViLauncher/1.0 (github.com/ViPok137/ViLauncher)' } }, res => {
      if (res.statusCode >= 400) return reject(new Error('HTTP ' + res.statusCode));
      let data = '';
      res.on('data', c => data += c);
      res.on('end', () => {
        try { resolve(JSON.parse(data)); }
        catch (e) { reject(e); }
      });
    }).on('error', reject);
  });
}

// Поиск проектов: type = 'shader' | 'resourcepack'
ipcMain.handle('modrinth-search', async (_, { query, type, limit }) => {
  try {
    const facets = encodeURIComponent(JSON.stringify([
      [`project_type:${type}`],
      [`versions:${installer.MC_VERSION}`],
    ]));
    const q = encodeURIComponent(query || '');
    const url = `${MODRINTH_API}/search?query=${q}&facets=${facets}&limit=${limit || 20}`;
    const data = await modrinthFetch(url);
    return {
      ok: true,
      hits: (data.hits || []).map(h => ({
        id:          h.project_id,
        title:       h.title,
        description: h.description,
        iconUrl:     h.icon_url,
        downloads:   h.downloads,
        author:      h.author,
      })),
    };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Получаем версии проекта подходящие под MC_VERSION + Forge
ipcMain.handle('modrinth-versions', async (_, { projectId }) => {
  try {
    const url = `${MODRINTH_API}/project/${projectId}/version`
      + `?game_versions=["${installer.MC_VERSION}"]`;
    const versions = await modrinthFetch(url);
    return {
      ok: true,
      versions: versions.slice(0, 10).map(v => ({
        id:          v.id,
        versionName: v.version_number,
        loaders:     v.loaders,
        files: (v.files || []).map(f => ({
          url:      f.url,
          filename: f.filename,
          size:     f.size,
          primary:  f.primary,
        })),
      })),
    };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Скачивание шейдера/ресурспака в нужную папку клиента
ipcMain.handle('modrinth-download', async (_, { url, filename, type }) => {
  try {
    // type: 'shader' → shaderpacks/, 'resourcepack' → resourcepacks/
    const folder = type === 'shader' ? 'shaderpacks' : 'resourcepacks';
    const destDir = path.join(CLIENT_DIR, folder);
    fs.mkdirSync(destDir, { recursive: true });
    const dest = path.join(destDir, filename);

    await dlFileProgress(url, dest, (done, total) => {
      win.webContents.send('modrinth-dl-progress', {
        filename,
        pct: total > 0 ? Math.round(done / total * 100) : 0,
      });
    });

    return { ok: true, path: dest };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Список уже скачанных шейдеров/ресурспаков
ipcMain.handle('modrinth-installed', async (_, { type }) => {
  try {
    const folder = type === 'shader' ? 'shaderpacks' : 'resourcepacks';
    const dir = path.join(CLIENT_DIR, folder);
    if (!fs.existsSync(dir)) return { ok: true, files: [] };
    const files = fs.readdirSync(dir).filter(f => f.endsWith('.zip'));
    return { ok: true, files };
  } catch (e) { return { ok: false, error: e.message }; }
});

// Удаление шейдера/ресурспака
ipcMain.handle('modrinth-remove', async (_, { filename, type }) => {
  try {
    const folder = type === 'shader' ? 'shaderpacks' : 'resourcepacks';
    fs.unlinkSync(path.join(CLIENT_DIR, folder, filename));
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// ─── SERVER PING (ТЗ 3.4: minecraft-server-util с fallback на ручной протокол) ──
// Простой TCP-пинг не подходит — Playit держит порт открытым даже без сервера.
const net = require('net');

let mcServerUtil = null;
try { mcServerUtil = require('minecraft-server-util'); } catch { mcServerUtil = null; }

ipcMain.handle('server-ping', async () => {
  // Приоритет: minecraft-server-util (надёжная, поддерживаемая библиотека)
  if (mcServerUtil) {
    try {
      const result = await mcServerUtil.status(CFG.SERVER_IP, CFG.SERVER_PORT, { timeout: 4000 });
      return {
        online:       true,
        ping:         result.roundTripLatency,
        version:      result.version?.name || '',
        online_count: result.players?.online || 0,
        max_count:    result.players?.max    || 0,
        motd:         result.motd?.clean     || '',
      };
    } catch (e) {
      return { online: false };
    }
  }

  // Fallback: ручная реализация Minecraft Status протокола (если пакет не установлен)
  return await pingServerManual();
});

function pingServerManual() {
  return new Promise((resolve) => {
    const host    = CFG.SERVER_IP;
    const port    = CFG.SERVER_PORT;
    const timeout = 4000;
    const socket  = new net.Socket();
    let   resolved = false;
    let   start;

    function done(result) {
      if (resolved) return;
      resolved = true;
      socket.destroy();
      resolve(result);
    }

    socket.setTimeout(timeout);
    socket.on('error',   () => done({ online: false }));
    socket.on('timeout', () => done({ online: false }));

    socket.connect(port, host, () => {
      start = Date.now();

      // ── Handshake packet (0x00) ────────────────────────────────────────────
      // Пакет: PacketID=0x00, ProtocolVersion=47(varint), Host, Port, NextState=1
      const hostBuf  = Buffer.from(host, 'utf8');
      const hostLen  = hostBuf.length;

      // Собираем тело Handshake
      const handshakeBody = Buffer.concat([
        encodeVarint(0x00),                   // Packet ID
        encodeVarint(47),                      // Protocol version (1.8, достаточно для пинга)
        encodeVarint(hostLen),                 // Host string length
        hostBuf,                               // Host
        Buffer.from([port >> 8, port & 0xFF]), // Port (2 bytes big-endian)
        encodeVarint(1),                       // Next state: Status
      ]);
      socket.write(Buffer.concat([encodeVarint(handshakeBody.length), handshakeBody]));

      // ── Status Request packet (0x00, length=1) ────────────────────────────
      socket.write(Buffer.from([0x01, 0x00]));

      // ── Читаем ответ ──────────────────────────────────────────────────────
      let   buf  = Buffer.alloc(0);
      socket.on('data', chunk => {
        buf = Buffer.concat([buf, chunk]);

        // Ждём хотя бы 5 байт (varint length + packet id + varint json len)
        if (buf.length < 5) return;

        try {
          // Читаем длину пакета (varint)
          const { value: pktLen, bytesRead: lenBytes } = readVarint(buf, 0);
          if (buf.length < lenBytes + pktLen) return; // ещё не всё пришло

          // Читаем packet id (varint)
          const { value: pktId, bytesRead: idBytes } = readVarint(buf, lenBytes);
          if (pktId !== 0x00) return; // не Status Response

          // Читаем длину JSON строки (varint)
          const { value: jsonLen, bytesRead: jsonLenBytes } = readVarint(buf, lenBytes + idBytes);
          const jsonStart = lenBytes + idBytes + jsonLenBytes;

          if (buf.length < jsonStart + jsonLen) return;

          const jsonStr = buf.slice(jsonStart, jsonStart + jsonLen).toString('utf8');
          const ping    = Date.now() - start;

          try {
            const status = JSON.parse(jsonStr);
            const players = status.players || {};
            done({
              online:  true,
              ping,
              version: status.version?.name  || '',
              online_count:  players.online  || 0,
              max_count:     players.max     || 0,
              motd:    status.description?.text || '',
            });
          } catch {
            // JSON есть — сервер работает, просто не смогли распарсить
            done({ online: true, ping });
          }
        } catch {
          // Пакет ещё не полный — ждём
        }
      });
    });
  });
}

// Вспомогательные функции для VarInt (Minecraft protocol)
function encodeVarint(val) {
  const bytes = [];
  do {
    let byte = val & 0x7F;
    val >>>= 7;
    if (val !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (val !== 0);
  return Buffer.from(bytes);
}

function readVarint(buf, offset) {
  let value = 0, shift = 0, bytesRead = 0;
  let byte;
  do {
    if (offset + bytesRead >= buf.length) throw new Error('Buffer too short');
    byte = buf[offset + bytesRead];
    value |= (byte & 0x7F) << shift;
    shift += 7;
    bytesRead++;
  } while (byte & 0x80);
  return { value, bytesRead };
}


// ─── RAM SETTINGS ────────────────────────────────────────────────────────────
ipcMain.handle('ram-get', () => {
  const store   = loadStore();
  const totalGb = os.totalmem() / 1024 / 1024 / 1024;
  let autoMin, autoMax;
  if      (totalGb <= 4.2) { autoMin = 1; autoMax = 2; }
  else if (totalGb <= 8)   { autoMin = 2; autoMax = 3; }
  else if (totalGb <= 16)  { autoMin = 3; autoMax = Math.min(6, Math.floor(totalGb * 0.4)); }
  else if (totalGb <= 32)  { autoMin = 4; autoMax = Math.min(8, Math.floor(totalGb * 0.35)); }
  else                     { autoMin = 4; autoMax = Math.min(10, Math.floor(totalGb * 0.3)); }
  return {
    totalGb:    Math.round(totalGb),
    autoMax,
    autoMin,
    currentMax: store.ramMaxG || 'auto',
    currentMin: store.ramMinG || 'auto',
  };
});

ipcMain.handle('ram-set', (_, { maxG, minG }) => {
  const store = loadStore();
  store.ramMaxG = maxG;
  store.ramMinG = minG;
  saveStore(store);
  return { ok: true };
});

// ─── AUTO RAM ────────────────────────────────────────────────────────────────
function autoRam() {
  const totalGb = os.totalmem() / 1024 / 1024 / 1024;
  let minG, maxG;

  // ТЗ 2.2: для слабых ПК (≤4.2 ГБ) выделяем меньше — иначе ОС остаётся без памяти
  // и происходит дисковый трэшинг (своппинг), который парализует систему
  if      (totalGb <= 4.2) { minG = 1; maxG = 2; }
  else if (totalGb <= 8)   { minG = 2; maxG = 3; }
  else if (totalGb <= 16)  { minG = 3; maxG = Math.min(6, Math.floor(totalGb * 0.4)); }
  else if (totalGb <= 32)  { minG = 4; maxG = Math.min(8, Math.floor(totalGb * 0.35)); }
  else                     { minG = 4; maxG = Math.min(10, Math.floor(totalGb * 0.3)); }

  // Проверяем пользовательскую настройку из settings.json
  const store = loadStore();
  if (store.ramMaxG && store.ramMaxG !== 'auto') maxG = parseInt(store.ramMaxG) || maxG;
  if (store.ramMinG && store.ramMinG !== 'auto') minG = parseInt(store.ramMinG) || minG;

  // minG всегда < maxG
  if (minG >= maxG) minG = Math.max(1, maxG - 1);

  return [
    `-Xms${minG}G`,
    `-Xmx${maxG}G`,
    // Оптимизированные GC флаги для Minecraft (Aikar's flags)
    '-XX:+UseG1GC',
    '-XX:+ParallelRefProcEnabled',
    '-XX:MaxGCPauseMillis=200',
    '-XX:+UnlockExperimentalVMOptions',
    '-XX:+DisableExplicitGC',
    '-XX:+AlwaysPreTouch',
    '-XX:G1NewSizePercent=30',
    '-XX:G1MaxNewSizePercent=40',
    '-XX:G1HeapRegionSize=8M',
    '-XX:G1ReservePercent=20',
    '-XX:G1HeapWastePercent=5',
    '-XX:G1MixedGCCountTarget=4',
    '-XX:InitiatingHeapOccupancyPercent=15',
    '-XX:G1MixedGCLiveThresholdPercent=90',
    '-XX:SurvivorRatio=32',
    '-XX:+PerfDisableSharedMem',
    '-XX:MaxTenuringThreshold=1',
  ];
}

// ─── HTTP UTILS ──────────────────────────────────────────────────────────────
function fetchText(url, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    const req = mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location)
        return fetchText(res.headers.location, timeoutMs).then(resolve).catch(reject);
      let d = '';
      res.on('data', c => d += c);
      res.on('end', () => res.statusCode >= 400
        ? reject(new Error(`HTTP ${res.statusCode}`)) : resolve(d));
    });
    req.on('error', reject);
    req.setTimeout(timeoutMs, () => {
      req.destroy(new Error(`Timeout: ${url}`));
    });
  });
}

function dlFile(url, dest, retries = 3) {
  return new Promise((resolve, reject) => {
    const mod  = url.startsWith('https') ? https : http;
    const tmp  = dest + '.tmp';
    const file = fs.createWriteStream(tmp);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close(); fs.unlink(tmp, () => {});
        return dlFile(res.headers.location, dest, retries).then(resolve).catch(reject);
      }
      if (res.statusCode >= 400) {
        file.close(); fs.unlink(tmp, () => {});
        if (retries > 0) return setTimeout(() => dlFile(url, dest, retries - 1).then(resolve).catch(reject), 1000);
        return reject(new Error(`HTTP ${res.statusCode}`));
      }
      res.pipe(file);
      file.on('finish', () => file.close(() => fs.rename(tmp, dest, e => e ? reject(e) : resolve())));
    }).on('error', err => {
      file.close(); fs.unlink(tmp, () => {});
      if (retries > 0) return setTimeout(() => dlFile(url, dest, retries - 1).then(resolve).catch(reject), 1000);
      reject(err);
    });
  });
}

function dlProgress(url, dest, onProgress) {
  return new Promise((resolve, reject) => {
    const mod  = url.startsWith('https') ? https : http;
    const file = fs.createWriteStream(dest);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close();
        return dlProgress(res.headers.location, dest, onProgress).then(resolve).catch(reject);
      }
      const total = parseInt(res.headers['content-length'] || '0', 10);
      let done = 0;
      res.on('data', chunk => { done += chunk.length; onProgress(done, total); file.write(chunk); });
      res.on('end', () => file.close(resolve));
    }).on('error', e => { file.close(); reject(e); });
  });
}
// dlFileProgress — скачивание с побайтовым прогрессом + retry
function dlFileProgress(url, dest, onProgress, retries = 3) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    const tmp = dest + '.tmp';
    const file = fs.createWriteStream(tmp);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close(); fs.unlink(tmp, () => {});
        return dlFileProgress(res.headers.location, dest, onProgress, retries).then(resolve).catch(reject);
      }
      if (res.statusCode >= 400) {
        file.close(); fs.unlink(tmp, () => {});
        if (retries > 0) return setTimeout(() => dlFileProgress(url, dest, onProgress, retries - 1).then(resolve).catch(reject), 1500);
        return reject(new Error(`HTTP ${res.statusCode} — ${url}`));
      }
      const fileSize = parseInt(res.headers['content-length'] || '0', 10);
      let fileDone = 0;
      res.on('data', chunk => {
        fileDone += chunk.length;
        onProgress(fileDone, fileSize);
        file.write(chunk);
      });
      res.on('end', () => file.close(() => fs.rename(tmp, dest, e => e ? reject(e) : resolve())));
      res.on('error', err => { file.close(); fs.unlink(tmp, () => {}); reject(err); });
    }).on('error', err => {
      file.close(); fs.unlink(tmp, () => {});
      if (retries > 0) return setTimeout(() => dlFileProgress(url, dest, onProgress, retries - 1).then(resolve).catch(reject), 1500);
      reject(err);
    });
  });
}
