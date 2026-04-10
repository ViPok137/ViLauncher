'use strict';

/**
 * installer.js — Minecraft 1.20.1 + Forge 47.x
 *
 * Порядок работы:
 *  1. version_manifest → version.json для 1.20.1
 *  2. client.jar
 *  3. vanilla libraries + natives (распаковка через Node.js zlib)
 *  4. assets (индекс + объекты)
 *  5. Forge installer → --installClient
 *  6. Читаем forge version.json, рекурсивно разворачиваем inheritsFrom
 *  7. Строим правильный classpath и сохраняем launch.json
 */

const https  = require('https');
const http   = require('http');
const fs     = require('fs');
const path   = require('path');
const os     = require('os');
const zlib   = require('zlib');
const { spawnSync } = require('child_process');
const { createHash } = require('crypto');

const MC_VERSION    = '1.20.1';
const FORGE_VERSION = '1.20.1-47.3.0';
const FORGE_JAR     = `forge-${FORGE_VERSION}-installer.jar`;
const FORGE_URL     = `https://maven.minecraftforge.net/net/minecraftforge/forge/${FORGE_VERSION}/${FORGE_JAR}`;
const MOJANG_MANIFEST = 'https://launchermeta.mojang.com/mc/game/version_manifest_v2.json';
const RESOURCES_BASE  = 'https://resources.download.minecraft.net';

// ─── ГЛАВНАЯ ФУНКЦИЯ ─────────────────────────────────────────────────────────
async function install(clientDir, onProgress = () => {}) {
  const dirs = {
    root:      clientDir,
    libraries: path.join(clientDir, 'libraries'),
    natives:   path.join(clientDir, 'natives'),
    assets:    path.join(clientDir, 'assets'),
    indexes:   path.join(clientDir, 'assets', 'indexes'),
    objects:   path.join(clientDir, 'assets', 'objects'),
    versions:  path.join(clientDir, 'versions'),
  };
  Object.values(dirs).forEach(d => fs.mkdirSync(d, { recursive: true }));

  // 1. Манифест Mojang
  onProgress('manifest', 0, 1, 'Получаю манифест Mojang...');
  const manifest     = JSON.parse(await fetchText(MOJANG_MANIFEST));
  const versionEntry = manifest.versions.find(v => v.id === MC_VERSION);
  if (!versionEntry) throw new Error(`Версия ${MC_VERSION} не найдена`);

  const vanillaJson     = JSON.parse(await fetchText(versionEntry.url));
  const vanillaDir      = path.join(dirs.versions, MC_VERSION);
  fs.mkdirSync(vanillaDir, { recursive: true });
  fs.writeFileSync(path.join(vanillaDir, `${MC_VERSION}.json`), JSON.stringify(vanillaJson, null, 2));
  onProgress('manifest', 1, 1, 'Манифест получен');

  // 2. client.jar
  const clientJar  = path.join(vanillaDir, `${MC_VERSION}.jar`);
  const clientInfo = vanillaJson.downloads.client;
  if (!checkSha1(clientJar, clientInfo.sha1)) {
    onProgress('client', 0, 1, 'Скачиваю client.jar...');
    await dlFile(clientInfo.url, clientJar);
  }
  onProgress('client', 1, 1, 'client.jar готов');

  // 3. Libraries + natives
  const libs = (vanillaJson.libraries || []).filter(isLibAllowed);
  onProgress('libraries', 0, libs.length, 'Загружаю библиотеки...');
  for (let i = 0; i < libs.length; i++) {
    await downloadLibrary(libs[i], dirs.libraries, dirs.natives);
    onProgress('libraries', i + 1, libs.length, getLibName(libs[i]));
  }

  // 4. Assets
  const assetIndexInfo = vanillaJson.assetIndex;
  const assetIndexPath = path.join(dirs.indexes, `${assetIndexInfo.id}.json`);
  if (!fs.existsSync(assetIndexPath)) {
    onProgress('assets', 0, 1, 'Скачиваю индекс ресурсов...');
    await dlFile(assetIndexInfo.url, assetIndexPath);
  }

  const assetIndex   = JSON.parse(fs.readFileSync(assetIndexPath, 'utf8'));
  const assetObjects = Object.entries(assetIndex.objects);
  onProgress('assets', 0, assetObjects.length, 'Загружаю ресурсы...');
  let assetsDone = 0;

  for (let i = 0; i < assetObjects.length; i += 10) {
    const batch = assetObjects.slice(i, i + 10);
    await Promise.all(batch.map(async ([, obj]) => {
      const prefix  = obj.hash.slice(0, 2);
      const dest    = path.join(dirs.objects, prefix, obj.hash);
      if (!checkSha1(dest, obj.hash)) {
        fs.mkdirSync(path.dirname(dest), { recursive: true });
        await dlFile(`${RESOURCES_BASE}/${prefix}/${obj.hash}`, dest);
      }
    }));
    assetsDone += batch.length;
    onProgress('assets', assetsDone, assetObjects.length, `${assetsDone}/${assetObjects.length}`);
  }

  // 5. Forge installer
  const forgeInstallerPath = path.join(clientDir, FORGE_JAR);
  if (!checkSha1(forgeInstallerPath)) {
    onProgress('forge', 0, 3, 'Скачиваю Forge installer...');
    await dlFile(FORGE_URL, forgeInstallerPath);
  }

  onProgress('forge', 1, 3, 'Ищу Java 17 для Forge installer...');

  // Forge 47.x требует СТРОГО Java 17
  const launcherDir = path.dirname(clientDir);
  const javaExe = findJava(launcherDir);
  const javaVer = getJavaVersion(javaExe);

  if (javaVer !== 0 && javaVer !== 17) {
    throw new Error(
      'Forge 1.20.1 требует Java 17, но найдена Java ' + javaVer + '.\n' +
      'Путь: ' + javaExe + '\n\n' +
      'Установи Java 17 с https://adoptium.net (выбери Temurin 17 LTS)\n' +
      'После установки нажми ПОВТОРИТЬ.'
    );
  }

  // Forge installer требует launcher_profiles.json — создаём заглушку
  const profilesPath = path.join(clientDir, 'launcher_profiles.json');
  if (!fs.existsSync(profilesPath)) {
    fs.writeFileSync(profilesPath, JSON.stringify({
      profiles: {
        forge: {
          name: 'forge',
          type: 'custom',
          lastVersionId: MC_VERSION,
          gameDir: clientDir,
        }
      },
      selectedProfile: 'forge',
      clientToken: 'mc-launcher',
      authenticationDatabase: {},
      launcherVersion: { name: '2.12.2', format: 21, profilesFormat: 2 },
    }, null, 2));
  }

  // Forge installer ищет client.jar в versions/1.20.1/1.20.1.jar ОТНОСИТЕЛЬНО installClient
  // Создаём структуру которую он ожидает
  const forgeExpectedVersionDir = path.join(clientDir, 'versions', MC_VERSION);
  const forgeExpectedClientJar  = path.join(forgeExpectedVersionDir, MC_VERSION + '.jar');
  fs.mkdirSync(forgeExpectedVersionDir, { recursive: true });
  if (!fs.existsSync(forgeExpectedClientJar) && fs.existsSync(clientJar)) {
    fs.copyFileSync(clientJar, forgeExpectedClientJar);
  }
  // Также копируем version.json который forge ожидает найти
  const forgeExpectedVersionJson = path.join(forgeExpectedVersionDir, MC_VERSION + '.json');
  const vanillaJsonPath = path.join(dirs.versions, MC_VERSION, MC_VERSION + '.json');
  if (!fs.existsSync(forgeExpectedVersionJson) && fs.existsSync(vanillaJsonPath)) {
    fs.copyFileSync(vanillaJsonPath, forgeExpectedVersionJson);
  }

  // Запускаем forge installer, пишем весь вывод в лог-файл
  const forgeLogPath = path.join(clientDir, 'forge-install.log');
  onProgress('forge', 1, 3, 'Запускаю Forge installer (Java ' + (javaVer || '?') + ', 2-3 мин)...');

  const forgeResult = spawnSync(javaExe, [
    '-jar', forgeInstallerPath,
    '--installClient', clientDir,
  ], {
    cwd:         clientDir,
    stdio:       'pipe',
    timeout:     10 * 60 * 1000,
    windowsHide: true,
  });

  // Сохраняем полный лог
  const forgeStdout = (forgeResult.stdout || '').toString();
  const forgeStderr = (forgeResult.stderr || '').toString();
  fs.writeFileSync(forgeLogPath,
    '=== STDOUT ===\n' + forgeStdout +
    '\n=== STDERR ===\n' + forgeStderr
  );

  onProgress('forge', 2, 3, 'Forge installer завершён, строю classpath...');

  // 6. Читаем forge version.json
  const forgeVersionId = findForgeVersionId(dirs.versions);
  if (!forgeVersionId) {
    const exitCode = forgeResult.status;
    // Ищем реальную ошибку в логе
    const errLines = forgeStderr.split('\n').filter(l =>
      l.includes('ERROR') || l.includes('Exception') || l.includes('FAILED') || l.includes('error')
    );
    const errSummary = errLines.slice(-5).join('\n') || forgeStderr.slice(-500);
    throw new Error(
      'Forge installer завершился с кодом ' + exitCode + '.\n' +
      (errSummary ? '\n' + errSummary : '') +
      '\n\nПолный лог: ' + forgeLogPath
    );
  }

  const forgeVersionDir  = path.join(dirs.versions, forgeVersionId);
  const forgeVersionJson = JSON.parse(
    fs.readFileSync(path.join(forgeVersionDir, `${forgeVersionId}.json`), 'utf8')
  );

  // 7. Строим launch.json
  // Forge 47.x использует "inheritsFrom": "1.20.1"
  // Нужно объединить libraries из обоих json
  const allLibs = [
    ...(vanillaJson.libraries || []),
    ...(forgeVersionJson.libraries || []),
  ];

  const classpath = buildClasspath(allLibs, dirs.libraries, dirs.versions, clientDir);

  // Forge кладёт свои jar в libraries/net/minecraftforge/forge/
  // client.jar от vanilla тоже должен быть в classpath
  if (!classpath.includes(clientJar) && fs.existsSync(clientJar)) {
    classpath.push(clientJar);
  }

  const launchData = {
    mainClass:  forgeVersionJson.mainClass || vanillaJson.mainClass,
    classpath,
    assetsDir:  dirs.assets,
    assetIndex: vanillaJson.assetIndex.id,
    nativesDir: dirs.natives,
    gameDir:    clientDir,
    forgeVersionId,
  };

  fs.writeFileSync(path.join(clientDir, 'launch.json'), JSON.stringify(launchData, null, 2));
  onProgress('forge', 3, 3, 'Forge установлен');
  onProgress('done',  1, 1,  'Установка завершена!');
  return launchData;
}

// ─── CLASSPATH ────────────────────────────────────────────────────────────────
function buildClasspath(libs, libsDir, versionsDir, clientDir) {
  const entries = new Set();

  for (const lib of libs) {
    if (!isLibAllowed(lib)) continue;

    // Способ 1: есть downloads.artifact с path
    if (lib.downloads?.artifact?.path) {
      const p = path.join(libsDir, lib.downloads.artifact.path);
      if (fs.existsSync(p)) { entries.add(p); continue; }
    }

    // Способ 2: maven-координаты из name (формат Forge)
    if (lib.name) {
      const p = mavenToPath(lib.name, libsDir);
      if (p && fs.existsSync(p)) { entries.add(p); continue; }
    }
  }

  // client.jar vanilla
  const clientJar = path.join(versionsDir, MC_VERSION, `${MC_VERSION}.jar`);
  if (fs.existsSync(clientJar)) entries.add(clientJar);

  // forge-xxx-universal.jar / forge-xxx-client.jar
  const forgeJar = findForgeJar(clientDir);
  if (forgeJar) entries.add(forgeJar);

  return Array.from(entries);
}

// "net.minecraftforge:forge:1.20.1-47.3.0:universal"
//  → libraries/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-universal.jar
function mavenToPath(name, libsDir) {
  const parts = name.split(':');  // [group, artifact, version, classifier?]
  if (parts.length < 3) return null;
  const [group, artifact, version, classifier] = parts;
  const groupPath = group.replace(/\./g, '/');
  const fileName  = classifier
    ? `${artifact}-${version}-${classifier}.jar`
    : `${artifact}-${version}.jar`;
  return path.join(libsDir, groupPath, artifact, version, fileName);
}

function findForgeJar(clientDir) {
  const base = path.join(clientDir, 'libraries', 'net', 'minecraftforge', 'forge');
  try {
    if (!fs.existsSync(base)) return null;
    for (const ver of fs.readdirSync(base)) {
      const dir   = path.join(base, ver);
      const files = fs.readdirSync(dir);
      // Приоритет: universal > client > любой forge jar
      const pick  = files.find(f => f.includes('universal')) ||
                    files.find(f => f.includes('client'))    ||
                    files.find(f => f.endsWith('.jar') && !f.endsWith('-installer.jar'));
      if (pick) return path.join(dir, pick);
    }
  } catch {}
  return null;
}

function findForgeVersionId(versionsDir) {
  try {
    return fs.readdirSync(versionsDir).find(d =>
      d.toLowerCase().includes('forge') && d.includes(MC_VERSION)
    ) || null;
  } catch { return null; }
}

// ─── LIBRARY HELPERS ─────────────────────────────────────────────────────────
function isLibAllowed(lib) {
  if (!lib.rules?.length) return true;
  const osName = { win32: 'windows', darwin: 'osx', linux: 'linux' }[process.platform] || 'linux';
  let result = false;
  for (const rule of lib.rules) {
    if (!rule.os || rule.os.name === osName) {
      result = rule.action === 'allow';
    }
  }
  return result;
}

async function downloadLibrary(lib, libsDir, nativesDir) {
  const dl = lib.downloads || {};

  if (dl.artifact?.url) {
    const dest = path.join(libsDir, dl.artifact.path);
    if (!checkSha1(dest, dl.artifact.sha1)) {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      await dlFile(dl.artifact.url, dest);
    }
  }

  // Natives
  const platKey  = { win32: 'windows', darwin: 'osx', linux: 'linux' }[process.platform];
  const nativeKey = lib.natives?.[platKey]?.replace('${arch}', os.arch() === 'x64' ? '64' : '32');
  if (nativeKey && dl.classifiers?.[nativeKey]) {
    const nat  = dl.classifiers[nativeKey];
    const dest = path.join(libsDir, nat.path);
    if (!checkSha1(dest, nat.sha1)) {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      await dlFile(nat.url, dest);
    }
    extractNativesNode(dest, nativesDir, lib.extract?.exclude || []);
  }
}

function getLibName(lib) {
  return (lib.name || lib.downloads?.artifact?.path || '').split(':').slice(0, 2).join(':');
}

// ─── NATIVES (Node.js, без jar) ───────────────────────────────────────────────
function extractNativesNode(zipPath, destDir, exclude = []) {
  try {
    const data    = fs.readFileSync(zipPath);
    const entries = parseZip(data);
    for (const entry of entries) {
      if (entry.isDir) continue;
      if (exclude.some(ex => entry.name.startsWith(ex))) continue;
      if (!entry.name.endsWith('.dll') && !entry.name.endsWith('.so') &&
          !entry.name.endsWith('.dylib') && !entry.name.endsWith('.jnilib')) continue;
      const dest = path.join(destDir, path.basename(entry.name));
      try {
        const raw = entry.compression === 8
          ? zlib.inflateRawSync(entry.data)
          : entry.data;
        fs.writeFileSync(dest, raw);
      } catch {}
    }
  } catch {}
}

// Минимальный ZIP-парсер
function parseZip(buf) {
  const entries = [];
  let i = 0;
  while (i < buf.length - 4) {
    if (buf.readUInt32LE(i) !== 0x04034b50) { i++; continue; }
    const compression  = buf.readUInt16LE(i + 8);
    const compSize     = buf.readUInt32LE(i + 18);
    const nameLen      = buf.readUInt16LE(i + 26);
    const extraLen     = buf.readUInt16LE(i + 28);
    const name         = buf.slice(i + 30, i + 30 + nameLen).toString('utf8');
    const dataStart    = i + 30 + nameLen + extraLen;
    const data         = buf.slice(dataStart, dataStart + compSize);
    entries.push({ name, compression, data, isDir: name.endsWith('/') });
    i = dataStart + compSize;
  }
  return entries;
}

// ─── JAVA FINDER ─────────────────────────────────────────────────────────────
// Forge 47.x требует СТРОГО Java 17. Java 20/21 не поддерживается.
function findJava(launcherDir) {
  if (process.platform !== 'win32') {
    // На Linux/Mac ищем java17 или java
    for (const cmd of ['java17', 'java']) {
      try {
        const r = require('child_process').spawnSync(cmd, ['-version'], { encoding: 'utf8' });
        if (r.status === 0 || r.stderr) return cmd;
      } catch {}
    }
    return 'java';
  }

  // Windows: сначала смотрим наш bundled JRE17 в папке лаунчера
  if (launcherDir) {
    const bundled = path.join(launcherDir, 'jre17', 'bin', 'java.exe');
    if (fs.existsSync(bundled)) return bundled;
  }

  // Ищем именно Java 17 в стандартных местах
  const bases = [
    'C:\\Program Files\\Java',
    'C:\\Program Files\\Eclipse Adoptium',
    'C:\\Program Files\\Microsoft',
    'C:\\Program Files\\Amazon Corretto',
    'C:\\Program Files\\BellSoft',
    'C:\\Program Files\\Zulu',
  ];

  for (const base of bases) {
    try {
      if (!fs.existsSync(base)) continue;
      const dirs = fs.readdirSync(base).filter(d => /17/i.test(d)).sort().reverse();
      for (const d of dirs) {
        const p = path.join(base, d, 'bin', 'java.exe');
        if (fs.existsSync(p)) return p;
      }
    } catch {}
  }

  // Minecraft Official Launcher bundled JRE (java-runtime-gamma = Java 17)
  const mcBase = path.join(os.homedir(), 'AppData', 'Local', 'Packages');
  const mcDirs = ['Microsoft.4297127D64EC6_8wekyb3d8bbwe'];
  for (const d of mcDirs) {
    const p = path.join(mcBase, d, 'LocalCache', 'Local', 'runtime',
      'java-runtime-gamma', 'windows-x64', 'java-runtime-gamma', 'bin', 'java.exe');
    if (fs.existsSync(p)) return p;
  }

  // TLauncher bundled JRE
  const tlPaths = [
    path.join(os.homedir(), 'AppData', 'Roaming', '.tlauncher', 'runtime', 'java17', 'bin', 'java.exe'),
    path.join(os.homedir(), 'AppData', 'Roaming', '.tlauncher', 'jre', 'x64', '17', 'bin', 'java.exe'),
  ];
  for (const p of tlPaths) {
    if (fs.existsSync(p)) return p;
  }

  // Последний шанс — любая java из PATH (может быть не 17!)
  try {
    const r = require('child_process').spawnSync('where', ['java'], { encoding: 'utf8', windowsHide: true });
    if (r.status === 0 && r.stdout.trim()) return r.stdout.trim().split('\n')[0].trim();
  } catch {}

  return 'java';
}

// Проверяет версию java и возвращает major version (17, 20, 21...)
function getJavaVersion(javaPath) {
  try {
    const r = require('child_process').spawnSync(
      javaPath, ['-XshowSettings:all', '-version'],
      { encoding: 'utf8', windowsHide: true, timeout: 5000 }
    );
    const out = (r.stdout || '') + (r.stderr || '');
    const m   = out.match(/version "?(\d+)/);
    return m ? parseInt(m[1]) : 0;
  } catch { return 0; }
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
        ? reject(new Error(`HTTP ${res.statusCode}: ${url}`)) : resolve(d));
    }).on('error', reject);
  });
}

function dlFile(url, dest, retries = 3) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    const tmp = dest + '.tmp';
    fs.mkdirSync(path.dirname(tmp), { recursive: true });
    const file = fs.createWriteStream(tmp);
    mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close(); fs.unlink(tmp, () => {});
        return dlFile(res.headers.location, dest, retries).then(resolve).catch(reject);
      }
      if (res.statusCode >= 400) {
        file.close(); fs.unlink(tmp, () => {});
        if (retries > 0) return setTimeout(() => dlFile(url, dest, retries - 1).then(resolve).catch(reject), 1000);
        return reject(new Error(`HTTP ${res.statusCode}: ${url}`));
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

function checkSha1(filePath, expected) {
  if (!fs.existsSync(filePath)) return false;
  if (!expected) return true;
  try {
    const hash = createHash('sha1').update(fs.readFileSync(filePath)).digest('hex');
    return hash === expected;
  } catch { return false; }
}

module.exports = { install, findJava, MC_VERSION, FORGE_VERSION };
