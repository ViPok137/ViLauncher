'use strict';

const { app, BrowserWindow, ipcMain } = require('electron');
const path   = require('path');
const fs     = require('fs');
const https  = require('https');
const http   = require('http');
const os     = require('os');
const { spawn, execFile } = require('child_process');

const CFG       = require('./config');
const installer = require('./installer');

// ─── PATHS ───────────────────────────────────────────────────────────────────
const LAUNCHER_DIR = path.join(os.homedir(), '.mc-launcher');
const CLIENT_DIR   = path.join(LAUNCHER_DIR, 'client');
const STORE_PATH   = path.join(LAUNCHER_DIR, 'settings.json');
const UPDATE_DIR   = path.join(LAUNCHER_DIR, 'update');
const LAUNCH_JSON  = path.join(CLIENT_DIR, 'launch.json'); // сохранённые данные запуска

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
}
app.whenReady().then(createWindow);
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });

// ─── WINDOW CONTROLS ─────────────────────────────────────────────────────────
ipcMain.on('win-min',   () => win.minimize());
ipcMain.on('win-max',   () => win.isMaximized() ? win.unmaximize() : win.maximize());
ipcMain.on('win-close', () => win.close());

// ─── NICKNAME ────────────────────────────────────────────────────────────────
// ─── APP INFO ────────────────────────────────────────────────────────────────
ipcMain.handle('app-info', () => {
  const store = loadStore();
  const javaPath = (CFG.JAVA_PATH && CFG.JAVA_PATH !== 'java')
    ? CFG.JAVA_PATH
    : installer.findJava(LAUNCHER_DIR);
  return {
    version:     app.getVersion(),
    modsVersion: store.modsVersion || null,
    electron:    process.versions.electron,
    os:          process.platform === 'win32' ? 'Windows' : process.platform,
    java:        javaPath,
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

    await installer.install(CLIENT_DIR, (phase, done, total, name) => {
      const phaseNames = {
        manifest:  '📋 Манифест',
        client:    '📦 Клиент',
        libraries: '📚 Библиотеки',
        assets:    '🖼 Ресурсы',
        forge:     '⚙ Forge',
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
ipcMain.handle('mods-sync', async () => {
  try {
    // Сообщаем что идёт запрос манифеста
    win.webContents.send('mods-status', { text: 'Получаю список модов с GitHub...' });

    let manifestTxt;
    try {
      manifestTxt = await fetchText(RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'manifest.json'));
    } catch (e) {
      return { ok: false, error: 'Не удалось получить manifest.json: ' + e.message };
    }

    const manifest = JSON.parse(manifestTxt);
    const store    = loadStore();

    if (manifest.version === store.modsVersion) {
      return { ok: true, updated: false, msg: `Моды актуальны (${store.modsVersion})` };
    }

    const files = manifest.files || [];
    win.webContents.send('mods-status', { text: `Найдено ${files.length} файлов для обновления` });

    for (let i = 0; i < files.length; i++) {
      const f     = files[i];
      const fname = path.basename(f.path);
      const url   = RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, f.path);
      const dest  = path.join(CLIENT_DIR, f.path);
      fs.mkdirSync(path.dirname(dest), { recursive: true });

      // Сообщаем начало загрузки файла
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

      // Файл готов — сигнал что этот файл завершён
      win.webContents.send('mods-progress', {
        fileIndex: i + 1, total: files.length, name: fname, filePct: 100, done: true,
      });
    }

    store.modsVersion = manifest.version;
    saveStore(store);
    return { ok: true, updated: true, version: manifest.version };
  } catch (e) {
    return { ok: false, error: e.message };
  }
});

// ─── LAUNCH ──────────────────────────────────────────────────────────────────
ipcMain.handle('game-launch', async (_, { nickname }) => {
  try {
    if (!fs.existsSync(LAUNCH_JSON))
      return { ok: false, error: 'Minecraft не установлен. Нажми «Установить».' };

    const launch   = JSON.parse(fs.readFileSync(LAUNCH_JSON, 'utf8'));
    // CFG.JAVA_PATH имеет приоритет — если задан вручную в config.js
    const javaPath = (CFG.JAVA_PATH && CFG.JAVA_PATH !== 'java')
      ? CFG.JAVA_PATH
      : installer.findJava(LAUNCHER_DIR);
    const sep      = process.platform === 'win32' ? ';' : ':';

    // Проверяем что java существует
    if (javaPath !== 'java' && !fs.existsSync(javaPath)) {
      return { ok: false, error: `Java не найдена: ${javaPath}` };
    }

    // Проверяем classpath
    const missing = launch.classpath.filter(p => !fs.existsSync(p));
    if (missing.length > 0) {
      return { ok: false, error: `Отсутствуют файлы classpath (${missing.length} шт). Переустанови клиент.` };
    }

    const logPath = path.join(LAUNCHER_DIR, 'minecraft.log');
    const logFile = fs.openSync(logPath, 'w');

    const args = [
      `-Xmx${CFG.RAM_MAX}`,
      `-Xms${CFG.RAM_MIN}`,
      `-Djava.library.path=${launch.nativesDir}`,
      `-Dminecraft.launcher.brand=mc-launcher`,
      `-Dminecraft.launcher.version=1.0`,
      // Forge 47.x требует эти флаги
      '--add-opens', 'java.base/java.util.jar=ALL-UNNAMED',
      '--add-opens', 'java.base/java.lang.invoke=ALL-UNNAMED',
      '-cp', launch.classpath.join(sep),
      launch.mainClass,
      '--username',    nickname || CFG.DEFAULT_USERNAME,
      '--version',     installer.MC_VERSION,
      '--gameDir',     launch.gameDir,
      '--assetsDir',   launch.assetsDir,
      '--assetIndex',  launch.assetIndex,
      '--accessToken', 'null',
      '--userType',    'legacy',
      '--server',      CFG.SERVER_IP,
      '--port',        String(CFG.SERVER_PORT),
    ];

    const child = spawn(javaPath, args, {
      detached:    true,
      stdio:       ['ignore', logFile, logFile],
      cwd:         launch.gameDir,
      windowsHide: true,   // ← скрывает cmd на Windows
    });

    child.on('error', err => {
      fs.closeSync(logFile);
      win.webContents.send('game-error', { error: 'Не удалось запустить Java: ' + err.message });
    });

    // Если процесс упал сразу (< 5 сек) — читаем лог и показываем ошибку
    child.on('exit', (code) => {
      fs.closeSync(logFile);
      if (code !== 0 && code !== null) {
        try {
          const log = fs.readFileSync(logPath, 'utf8').slice(-2000);
          const lastLines = log.split('\n').slice(-15).join('\n');
          win.webContents.send('game-error', { error: `Java завершилась с кодом ${code}.\nЛог: ${lastLines}` });
        } catch {}
      }
    });

    child.unref();
    return { ok: true, logPath };
  } catch (e) { return { ok: false, error: e.message }; }
});

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
