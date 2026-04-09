'use strict';

const { app, BrowserWindow, ipcMain } = require('electron');
const path  = require('path');
const fs    = require('fs');
const https = require('https');
const http  = require('http');
const os    = require('os');
const { spawn, execFile } = require('child_process');

const CFG = require('./config');

// ─── PATHS ───────────────────────────────────────────────────────────────────
const LAUNCHER_DIR = path.join(os.homedir(), '.mc-launcher');
const CLIENT_DIR   = path.join(LAUNCHER_DIR, 'client');
const STORE_PATH   = path.join(LAUNCHER_DIR, 'settings.json');
const UPDATE_DIR   = path.join(LAUNCHER_DIR, 'update');

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

// ─── NICKNAME (сохраняем последний) ──────────────────────────────────────────
ipcMain.handle('nick-get', () => loadStore().nickname || '');
ipcMain.handle('nick-set', (_, nick) => {
  const s = loadStore(); s.nickname = nick; saveStore(s);
  return { ok: true };
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

// ─── LAUNCHER SELF-UPDATE ─────────────────────────────────────────────────────
ipcMain.handle('launcher-check', async () => {
  try {
    const txt = await fetchText(GH_API(CFG.LAUNCHER_OWNER, CFG.LAUNCHER_REPO, 'releases/latest'));
    const rel  = JSON.parse(txt);
    const remote = (rel.tag_name || '').replace(/^v/, '');
    const local  = app.getVersion();
    const asset  = (rel.assets || []).find(a => {
      if (process.platform === 'win32')   return a.name.endsWith('.exe');
      if (process.platform === 'linux')   return a.name.endsWith('.AppImage');
      return false;
    });
    return { hasUpdate: remote !== local && !!remote, remote, local, url: asset?.browser_download_url || null };
  } catch (e) {
    return { hasUpdate: false, error: e.message };
  }
});

ipcMain.handle('launcher-update', async (_, { url }) => {
  if (!url) return { ok: false, error: 'Нет ссылки' };
  try {
    fs.mkdirSync(UPDATE_DIR, { recursive: true });
    const ext  = process.platform === 'win32' ? '.exe' : '.AppImage';
    const dest = path.join(UPDATE_DIR, `update${ext}`);
    await dlProgress(url, dest, (done, total) => {
      win.webContents.send('launcher-dl-progress', { pct: total > 0 ? Math.round(done/total*100) : 0 });
    });
    if (process.platform === 'linux') fs.chmodSync(dest, '755');
    if (process.platform === 'win32') execFile(dest, ['/S'], { detached: true });
    else spawn(dest, ['--update'], { detached: true, stdio: 'ignore' }).unref();
    app.quit();
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// ─── MODS — проверяем при каждом запуске ──────────────────────────────────────
ipcMain.handle('mods-sync', async () => {
  try {
    const txt      = await fetchText(RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, 'manifest.json'));
    const manifest = JSON.parse(txt);
    const store    = loadStore();

    if (manifest.version === store.modsVersion) {
      return { ok: true, updated: false, msg: `Моды актуальны (${store.modsVersion})` };
    }

    const files = manifest.files || [];
    for (let i = 0; i < files.length; i++) {
      const f    = files[i];
      const url  = RAW_URL(CFG.MODS_OWNER, CFG.MODS_REPO, CFG.MODS_BRANCH, f.path);
      const dest = path.join(CLIENT_DIR, f.path);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      await dl(url, dest);
      win.webContents.send('mods-progress', { done: i + 1, total: files.length, name: path.basename(f.path) });
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
    const jar = path.join(CLIENT_DIR, `minecraft-${CFG.MC_VERSION}.jar`);
    if (!fs.existsSync(jar))
      return { ok: false, error: 'Клиент не найден. Дождись загрузки модов.' };

    const args = [
      `-Xmx${CFG.RAM_MAX}`, `-Xms${CFG.RAM_MIN}`,
      `-Djava.library.path=${path.join(CLIENT_DIR, 'natives')}`,
      '-cp', jar,
      'net.minecraft.client.main.Main',
      '--username', nickname || CFG.DEFAULT_USERNAME,
      '--version',  CFG.MC_VERSION,
      '--gameDir',  CLIENT_DIR,
      '--assetsDir', path.join(CLIENT_DIR, 'assets'),
      '--server',   CFG.SERVER_IP,
      '--port',     String(CFG.SERVER_PORT),
    ];
    spawn(CFG.JAVA_PATH, args, { detached: true, stdio: 'ignore' }).unref();
    return { ok: true };
  } catch (e) { return { ok: false, error: e.message }; }
});

// ─── HTTP UTILS ──────────────────────────────────────────────────────────────
function fetchText(url) {
  return new Promise((res, rej) => {
    const mod = url.startsWith('https') ? https : http;
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher' } }, r => {
      if (r.statusCode >= 300 && r.statusCode < 400 && r.headers.location)
        return fetchText(r.headers.location).then(res).catch(rej);
      let d = '';
      r.on('data', c => d += c);
      r.on('end', () => r.statusCode >= 400 ? rej(new Error(`HTTP ${r.statusCode}`)) : res(d));
    }).on('error', rej);
  });
}

function dl(url, dest) {
  return new Promise((res, rej) => {
    const mod  = url.startsWith('https') ? https : http;
    const file = fs.createWriteStream(dest);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher' } }, r => {
      if (r.statusCode >= 300 && r.statusCode < 400 && r.headers.location) {
        file.close(); return dl(r.headers.location, dest).then(res).catch(rej);
      }
      r.pipe(file);
      file.on('finish', () => file.close(res));
    }).on('error', e => { fs.unlink(dest, () => {}); rej(e); });
  });
}

function dlProgress(url, dest, onProgress) {
  return new Promise((res, rej) => {
    const mod  = url.startsWith('https') ? https : http;
    const file = fs.createWriteStream(dest);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher' } }, r => {
      if (r.statusCode >= 300 && r.statusCode < 400 && r.headers.location) {
        file.close(); return dlProgress(r.headers.location, dest, onProgress).then(res).catch(rej);
      }
      const total = parseInt(r.headers['content-length'] || '0', 10);
      let done = 0;
      r.on('data', chunk => { done += chunk.length; onProgress(done, total); file.write(chunk); });
      r.on('end', () => file.close(res));
    }).on('error', e => { fs.unlink(dest, () => {}); rej(e); });
  });
}
