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
  const bundled = path.join(LAUNCHER_DIR, 'jre17', 'bin',
    process.platform === 'win32' ? 'java.exe' : 'java');
  const javaPath = fs.existsSync(bundled) ? bundled
    : (CFG.JAVA_PATH && CFG.JAVA_PATH !== 'java') ? CFG.JAVA_PATH
    : installer.findJava(LAUNCHER_DIR);
  const javaLabel = fs.existsSync(bundled)
    ? 'Bundled JRE 17 (авто)'
    : javaPath;
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
    // Иначе используем bundled JRE17 из папки лаунчера (скачан при установке)
    let javaPath;
    if (CFG.JAVA_PATH && CFG.JAVA_PATH !== 'java') {
      javaPath = CFG.JAVA_PATH;
    } else {
      const bundled = require('path').join(LAUNCHER_DIR, 'jre17', 'bin',
        process.platform === 'win32' ? 'java.exe' : 'java');
      javaPath = fs.existsSync(bundled) ? bundled : installer.findJava(LAUNCHER_DIR);
    }
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
      '-Dfml.earlyprogresswindow=false',
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
      '--username',    nickname || CFG.DEFAULT_USERNAME,
      '--version',     installer.MC_VERSION,
      '--gameDir',     launch.gameDir,
      '--assetsDir',   launch.assetsDir,
      '--assetIndex',  launch.assetIndex,
      '--accessToken', 'null',
      '--userType',    'legacy',
      '--server',      CFG.SERVER_IP,
      '--port',        String(CFG.SERVER_PORT),
      // Отключаем ранний экран Forge через program arg (именно так проверяет Forge 47.x)
      '--fml.earlyprogresswindow', 'false',
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


// ─── SERVER PING (настоящий Minecraft Status Packet) ────────────────────────
// Простой TCP-пинг не подходит — Playit держит порт открытым даже без сервера.
// Используем настоящий Minecraft 1.7+ Status протокол.
const net = require('net');

ipcMain.handle('server-ping', async () => {
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
});

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
  if      (totalGb <= 8)  { autoMin = 2; autoMax = 3; }
  else if (totalGb <= 16) { autoMin = 3; autoMax = Math.min(6, Math.floor(totalGb * 0.4)); }
  else if (totalGb <= 32) { autoMin = 4; autoMax = Math.min(8, Math.floor(totalGb * 0.35)); }
  else                    { autoMin = 4; autoMax = Math.min(10, Math.floor(totalGb * 0.3)); }
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

  // Автоматическое определение по RAM системы
  if      (totalGb <= 8)  { minG = 2; maxG = 3; }
  else if (totalGb <= 16) { minG = 3; maxG = Math.min(6, Math.floor(totalGb * 0.4)); }
  else if (totalGb <= 32) { minG = 4; maxG = Math.min(8, Math.floor(totalGb * 0.35)); }
  else                    { minG = 4; maxG = Math.min(10, Math.floor(totalGb * 0.3)); }

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
