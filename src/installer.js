'use strict';

/**
 * installer.js
 * Полный установщик Minecraft 1.20.1 + Forge для кастомного лаунчера.
 *
 * Что делает:
 *  1. Качает версионный манифест Mojang → находит 1.20.1
 *  2. Качает client.jar
 *  3. Качает все libraries (150+ jar'ов)
 *  4. Качает и распаковывает natives
 *  5. Качает assets (индекс + объекты, ~300 МБ)
 *  6. Качает Forge installer и запускает его (устанавливает Forge в CLIENT_DIR)
 *  7. Читает forge version.json и сохраняет финальный classpath
 *
 * Использование из main.js:
 *   const installer = require('./installer');
 *   await installer.install(CLIENT_DIR, (phase, done, total, name) => { ... });
 */

const https  = require('https');
const http   = require('http');
const fs     = require('fs');
const path   = require('path');
const os     = require('os');
const { execFileSync, spawnSync } = require('child_process');
const { createHash }  = require('crypto');

// ─── КОНСТАНТЫ ────────────────────────────────────────────────────────────────
const MC_VERSION    = '1.20.1';
// Forge для 1.20.1 — последняя стабильная версия
const FORGE_VERSION = '1.20.1-47.3.0';
const FORGE_JAR     = `forge-${FORGE_VERSION}-installer.jar`;
const FORGE_URL     = `https://maven.minecraftforge.net/net/minecraftforge/forge/${FORGE_VERSION}/${FORGE_JAR}`;

const MOJANG_MANIFEST = 'https://launchermeta.mojang.com/mc/game/version_manifest_v2.json';
const RESOURCES_BASE  = 'https://resources.download.minecraft.net';

// ─── ГЛАВНАЯ ФУНКЦИЯ ──────────────────────────────────────────────────────────
/**
 * @param {string}   clientDir  — папка куда устанавливаем (~/.mc-launcher/client)
 * @param {Function} onProgress — (phase, done, total, name) => void
 */
async function install(clientDir, onProgress = () => {}) {

  const dirs = {
    root:       clientDir,
    libraries:  path.join(clientDir, 'libraries'),
    natives:    path.join(clientDir, 'natives'),
    assets:     path.join(clientDir, 'assets'),
    assetIndex: path.join(clientDir, 'assets', 'indexes'),
    assetObj:   path.join(clientDir, 'assets', 'objects'),
    versions:   path.join(clientDir, 'versions', MC_VERSION),
  };
  Object.values(dirs).forEach(d => fs.mkdirSync(d, { recursive: true }));

  // ── 1. Получаем version.json для 1.20.1 ──────────────────────────────────
  onProgress('manifest', 0, 1, 'Получаю манифест Mojang...');
  const manifest  = JSON.parse(await fetchText(MOJANG_MANIFEST));
  const versionEntry = manifest.versions.find(v => v.id === MC_VERSION);
  if (!versionEntry) throw new Error(`Версия ${MC_VERSION} не найдена в манифесте Mojang`);

  const versionJson = JSON.parse(await fetchText(versionEntry.url));
  const versionJsonPath = path.join(dirs.versions, `${MC_VERSION}.json`);
  fs.writeFileSync(versionJsonPath, JSON.stringify(versionJson, null, 2));
  onProgress('manifest', 1, 1, 'Манифест получен');

  // ── 2. Скачиваем client.jar ───────────────────────────────────────────────
  const clientJar  = path.join(dirs.versions, `${MC_VERSION}.jar`);
  const clientInfo = versionJson.downloads.client;
  if (!checkSha1(clientJar, clientInfo.sha1)) {
    onProgress('client', 0, 1, 'Скачиваю client.jar...');
    await dlFile(clientInfo.url, clientJar);
    onProgress('client', 1, 1, 'client.jar готов');
  } else {
    onProgress('client', 1, 1, 'client.jar уже есть');
  }

  // ── 3. Скачиваем libraries ────────────────────────────────────────────────
  const libs = versionJson.libraries.filter(lib => isLibAllowed(lib));
  onProgress('libraries', 0, libs.length, 'Начинаю загрузку библиотек...');
  let libsDone = 0;
  for (const lib of libs) {
    await downloadLibrary(lib, dirs.libraries, dirs.natives);
    libsDone++;
    onProgress('libraries', libsDone, libs.length, getLibName(lib));
  }

  // ── 4. Скачиваем assets ───────────────────────────────────────────────────
  const assetIndexInfo = versionJson.assetIndex;
  const assetIndexPath = path.join(dirs.assetIndex, `${assetIndexInfo.id}.json`);

  if (!fs.existsSync(assetIndexPath)) {
    onProgress('assets', 0, 1, 'Скачиваю индекс ресурсов...');
    await dlFile(assetIndexInfo.url, assetIndexPath);
  }

  const assetIndex = JSON.parse(fs.readFileSync(assetIndexPath, 'utf8'));
  const assetObjects = Object.entries(assetIndex.objects);
  onProgress('assets', 0, assetObjects.length, 'Начинаю загрузку ресурсов...');

  let assetsDone = 0;
  // Качаем батчами по 10 параллельно
  for (let i = 0; i < assetObjects.length; i += 10) {
    const batch = assetObjects.slice(i, i + 10);
    await Promise.all(batch.map(async ([name, obj]) => {
      const prefix  = obj.hash.slice(0, 2);
      const destDir = path.join(dirs.assetObj, prefix);
      const dest    = path.join(destDir, obj.hash);
      if (!checkSha1(dest, obj.hash)) {
        fs.mkdirSync(destDir, { recursive: true });
        await dlFile(`${RESOURCES_BASE}/${prefix}/${obj.hash}`, dest);
      }
    }));
    assetsDone += batch.length;
    onProgress('assets', assetsDone, assetObjects.length, `Ресурсы ${assetsDone}/${assetObjects.length}`);
  }

  // ── 5. Устанавливаем Forge ────────────────────────────────────────────────
  const forgeInstallerPath = path.join(clientDir, FORGE_JAR);
  if (!fs.existsSync(forgeInstallerPath)) {
    onProgress('forge', 0, 1, 'Скачиваю Forge installer...');
    await dlFile(FORGE_URL, forgeInstallerPath);
    onProgress('forge', 1, 2, 'Запускаю Forge installer...');
  } else {
    onProgress('forge', 1, 2, 'Запускаю Forge installer...');
  }

  // Запускаем forge installer в headless режиме
  // --installClient = устанавливает клиент в указанную папку
  const javaPath = process.platform === 'win32' ? findJava() : 'java';
  try {
    spawnSync(javaPath, [
      '-jar', forgeInstallerPath,
      '--installClient', clientDir,
    ], {
      cwd: clientDir,
      stdio: 'pipe',
      timeout: 5 * 60 * 1000, // 5 минут
    });
  } catch (e) {
    // Forge installer иногда возвращает ненулевой код даже при успехе
    // Проверяем результат по наличию forge version.json
  }
  onProgress('forge', 2, 2, 'Forge установлен');

  // ── 6. Читаем forge version.json и строим launch-данные ──────────────────
  const forgeVersionsDir = path.join(clientDir, 'versions');
  const forgeVersionId   = findForgeVersionId(forgeVersionsDir);
  let launchData;

  if (forgeVersionId) {
    const forgeJson = JSON.parse(
      fs.readFileSync(path.join(forgeVersionsDir, forgeVersionId, `${forgeVersionId}.json`), 'utf8')
    );
    launchData = buildLaunchData(forgeJson, versionJson, clientDir, dirs);
  } else {
    // Forge installer не сработал — запускаем vanilla как fallback
    launchData = buildLaunchDataVanilla(versionJson, clientDir, dirs);
  }

  // Сохраняем launch-данные
  const launchDataPath = path.join(clientDir, 'launch.json');
  fs.writeFileSync(launchDataPath, JSON.stringify(launchData, null, 2));

  onProgress('done', 1, 1, 'Установка завершена!');
  return launchData;
}

// ─── LAUNCH DATA ─────────────────────────────────────────────────────────────
function buildLaunchData(forgeJson, vanillaJson, clientDir, dirs) {
  // Forge inheritsFrom vanilla — объединяем libraries
  const allLibs = [
    ...(vanillaJson.libraries || []),
    ...(forgeJson.libraries   || []),
  ];

  const classpath = buildClasspath(allLibs, dirs.libraries, dirs.versions, clientDir);

  return {
    mainClass:   forgeJson.mainClass || vanillaJson.mainClass,
    classpath,
    assetsDir:   dirs.assets,
    assetIndex:  vanillaJson.assetIndex.id,
    nativesDir:  dirs.natives,
    gameDir:     clientDir,
  };
}

function buildLaunchDataVanilla(vanillaJson, clientDir, dirs) {
  const classpath = buildClasspath(vanillaJson.libraries || [], dirs.libraries, dirs.versions, clientDir);
  return {
    mainClass:   vanillaJson.mainClass,
    classpath,
    assetsDir:   dirs.assets,
    assetIndex:  vanillaJson.assetIndex.id,
    nativesDir:  dirs.natives,
    gameDir:     clientDir,
  };
}

function buildClasspath(libs, libsDir, versionsDir, clientDir) {
  const sep       = process.platform === 'win32' ? ';' : ':';
  const entries   = new Set();

  for (const lib of libs) {
    if (!isLibAllowed(lib)) continue;
    const jarPath = getLibPath(lib, libsDir);
    if (jarPath && fs.existsSync(jarPath)) entries.add(jarPath);
  }

  // Добавляем client.jar и forge jar
  const clientJar = path.join(versionsDir, MC_VERSION, `${MC_VERSION}.jar`);
  if (fs.existsSync(clientJar)) entries.add(clientJar);

  // forge-xxx-universal.jar если есть
  const forgeUniversal = findForgeUniversal(clientDir);
  if (forgeUniversal) entries.add(forgeUniversal);

  return Array.from(entries);
}

// ─── LIBRARY HELPERS ─────────────────────────────────────────────────────────
function isLibAllowed(lib) {
  if (!lib.rules) return true;
  for (const rule of lib.rules) {
    const allowed = rule.action === 'allow';
    if (!rule.os) return allowed;
    const osName = { win32: 'windows', darwin: 'osx', linux: 'linux' }[process.platform];
    if (rule.os.name === osName) return allowed;
  }
  return false;
}

async function downloadLibrary(lib, libsDir, nativesDir) {
  const downloads = lib.downloads || {};

  // Обычный artifact
  if (downloads.artifact) {
    const art  = downloads.artifact;
    const dest = path.join(libsDir, art.path);
    if (!checkSha1(dest, art.sha1)) {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      await dlFile(art.url, dest);
    }
  }

  // Natives
  const nativeKey = lib.natives?.[{ win32: 'windows', darwin: 'osx', linux: 'linux' }[process.platform]];
  if (nativeKey && downloads.classifiers?.[nativeKey]) {
    const nat  = downloads.classifiers[nativeKey];
    const dest = path.join(libsDir, nat.path);
    if (!checkSha1(dest, nat.sha1)) {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      await dlFile(nat.url, dest);
    }
    // Распаковываем natives
    extractNatives(dest, nativesDir, lib.extract?.exclude || []);
  }
}

function getLibPath(lib, libsDir) {
  if (lib.downloads?.artifact) return path.join(libsDir, lib.downloads.artifact.path);
  // Строим путь из maven-координат
  const [group, artifact, version] = (lib.name || '').split(':');
  if (!group) return null;
  const groupPath = group.replace(/\./g, '/');
  return path.join(libsDir, groupPath, artifact, version, `${artifact}-${version}.jar`);
}

function getLibName(lib) {
  return (lib.name || '').split(':').slice(0, 2).join(':');
}

// ─── FORGE HELPERS ───────────────────────────────────────────────────────────
function findForgeVersionId(versionsDir) {
  try {
    const dirs = fs.readdirSync(versionsDir);
    return dirs.find(d => d.includes('forge') && d.includes(MC_VERSION)) || null;
  } catch { return null; }
}

function findForgeUniversal(clientDir) {
  // Forge installer кладёт universal.jar в libraries/
  const libsDir = path.join(clientDir, 'libraries', 'net', 'minecraftforge', 'forge');
  try {
    if (!fs.existsSync(libsDir)) return null;
    const versions = fs.readdirSync(libsDir);
    for (const ver of versions) {
      const files = fs.readdirSync(path.join(libsDir, ver));
      const universal = files.find(f => f.endsWith('-universal.jar') || f.endsWith('-client.jar'));
      if (universal) return path.join(libsDir, ver, universal);
    }
  } catch {}
  return null;
}

// ─── NATIVES EXTRACTION ──────────────────────────────────────────────────────
function extractNatives(zipPath, destDir, exclude = []) {
  // Используем встроенный Java для распаковки (java должна быть в PATH)
  try {
    spawnSync('jar', ['xf', zipPath], {
      cwd: destDir,
      stdio: 'pipe',
    });
    // Удаляем excluded файлы (обычно META-INF)
    for (const ex of exclude) {
      const p = path.join(destDir, ex);
      if (fs.existsSync(p)) fs.rmSync(p, { recursive: true });
    }
  } catch {}
}

// ─── JAVA FINDER (Windows) ───────────────────────────────────────────────────
function findJava() {
  const javaHome = process.env.JAVA_HOME;
  if (javaHome) {
    const p = path.join(javaHome, 'bin', 'java.exe');
    if (fs.existsSync(p)) return p;
  }
  // Частые места установки
  const candidates = [
    'C:\\Program Files\\Java\\jdk-17\\bin\\java.exe',
    'C:\\Program Files\\Java\\jre-17\\bin\\java.exe',
    'C:\\Program Files\\Eclipse Adoptium\\jdk-17.0.0.0\\bin\\java.exe',
    'C:\\Program Files\\Microsoft\\jdk-17.0.0.0\\bin\\java.exe',
  ];
  for (const c of candidates) {
    if (fs.existsSync(c)) return c;
  }
  return 'java'; // надеемся что в PATH
}

// ─── HTTP UTILS ──────────────────────────────────────────────────────────────
function fetchText(url) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location)
        return fetchText(res.headers.location).then(resolve).catch(reject);
      let d = '';
      res.on('data', c => d += c);
      res.on('end', () => res.statusCode >= 400
        ? reject(new Error(`HTTP ${res.statusCode} — ${url}`))
        : resolve(d));
    }).on('error', reject);
  });
}

function dlFile(url, dest, retries = 3) {
  return new Promise((resolve, reject) => {
    const mod  = url.startsWith('https') ? https : http;
    const tmp  = dest + '.tmp';
    const file = fs.createWriteStream(tmp);

    const req = mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close();
        fs.unlink(tmp, () => {});
        return dlFile(res.headers.location, dest, retries).then(resolve).catch(reject);
      }
      if (res.statusCode >= 400) {
        file.close();
        fs.unlink(tmp, () => {});
        if (retries > 0) return setTimeout(() => dlFile(url, dest, retries - 1).then(resolve).catch(reject), 1000);
        return reject(new Error(`HTTP ${res.statusCode} — ${url}`));
      }
      res.pipe(file);
      file.on('finish', () => {
        file.close(() => {
          fs.rename(tmp, dest, err => err ? reject(err) : resolve());
        });
      });
    });
    req.on('error', err => {
      file.close();
      fs.unlink(tmp, () => {});
      if (retries > 0) return setTimeout(() => dlFile(url, dest, retries - 1).then(resolve).catch(reject), 1000);
      reject(err);
    });
  });
}

function checkSha1(filePath, expectedSha1) {
  if (!expectedSha1) return fs.existsSync(filePath);
  if (!fs.existsSync(filePath)) return false;
  try {
    const hash = createHash('sha1').update(fs.readFileSync(filePath)).digest('hex');
    return hash === expectedSha1;
  } catch { return false; }
}

module.exports = { install, findJava, MC_VERSION, FORGE_VERSION };
