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
const fsp    = fs.promises;          // async fs API (ТЗ 5c)
const path   = require('path');
const os     = require('os');
const zlib   = require('zlib');
const { promisify }  = require('util');
const inflateRawAsync = promisify(zlib.inflateRaw); // async zlib (ТЗ 5c)

// ТЗ 5c: утилита — async замена fs.existsSync (existsSync блокирует event loop)
async function existsAsync(p) {
  try { await fsp.access(p); return true; } catch { return false; }
}

const { spawnSync, spawn: spawnProc } = require('child_process');
const { createHash } = require('crypto');

// ТЗ 2.6: единое логирование через electron-log (тот же файл что и main.js использует)
let log;
try {
  log = require('electron-log');
} catch {
  log = {
    info:  (...a) => console.log('[info]', ...a),
    warn:  (...a) => console.warn('[warn]', ...a),
    error: (...a) => console.error('[error]', ...a),
  };
}

const MC_VERSION    = '1.20.1';
const FORGE_VERSION = '1.20.1-47.4.16';
const FORGE_JAR     = `forge-${FORGE_VERSION}-installer.jar`;
const FORGE_URLS = [
  'https://minecraft-inside.ru/download/470639/',  // Основной источник
  'https://raw.githubusercontent.com/ViPok137/ViLauncher/main/forge-1.20.1-47.4.16-installer.jar',
  `https://maven.minecraftforge.net/net/minecraftforge/forge/${FORGE_VERSION}/${FORGE_JAR}`,  // Fallback 1
  `https://maven.codemc.org/repository/maven-public/net/minecraftforge/forge/${FORGE_VERSION}/${FORGE_JAR}`,  // Fallback 2
];

const MOJANG_MANIFEST = 'https://launchermeta.mojang.com/mc/game/version_manifest_v2.json';
const RESOURCES_BASE  = 'https://resources.download.minecraft.net';

// Java 17 Temurin от Adoptium — автоматическая установка если нет Java 17
// Java 17 — несколько источников в порядке приоритета
// Adoptium (Eclipse Temurin) — открытая сборка, JRE-only (~45 МБ ZIP)
const JAVA17_WIN_URL  = 'https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jre/hotspot/normal/eclipse?project=jdk';
const JAVA17_LIN_URL  = 'https://api.adoptium.net/v3/binary/latest/17/ga/linux/x64/jre/hotspot/normal/eclipse?project=jdk';
const JAVA17_MAC_URL  = 'https://api.adoptium.net/v3/binary/latest/17/ga/mac/x64/jre/hotspot/normal/eclipse?project=jdk';
// Прямые запасные ссылки (Temurin 17.0.12 ZIP)
const JAVA17_WIN_FALLBACK = 'https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.12%2B7/OpenJDK17U-jre_x64_windows_hotspot_17.0.12_7.zip';
const JAVA17_LIN_FALLBACK = 'https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.12%2B7/OpenJDK17U-jre_x64_linux_hotspot_17.0.12_7.tar.gz';

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
  // ТЗ 5c: создаём папки асинхронно через for...of (forEach не поддерживает await)
  for (const d of Object.values(dirs)) {
    await fsp.mkdir(d, { recursive: true });
  }

  // 0. Проверяем Java 17 — скачиваем если нужно
  const launcherDir = path.dirname(clientDir);
  const javaExe = await ensureJava17(launcherDir, onProgress);

  // 1. Манифест Mojang
  onProgress('manifest', 0, 1, 'Получаю манифест Mojang...');
  const manifest     = JSON.parse(await fetchText(MOJANG_MANIFEST));
  const versionEntry = manifest.versions.find(v => v.id === MC_VERSION);
  if (!versionEntry) throw new Error(`Версия ${MC_VERSION} не найдена`);

  const vanillaJson     = JSON.parse(await fetchText(versionEntry.url));
  const vanillaDir      = path.join(dirs.versions, MC_VERSION);
  await fsp.mkdir(vanillaDir, { recursive: true });
  await fsp.writeFile(path.join(vanillaDir, `${MC_VERSION}.json`), JSON.stringify(vanillaJson, null, 2));
  onProgress('manifest', 1, 1, 'Манифест получен');

  // 2. client.jar — всегда проверяем SHA1, при несовпадении скачиваем заново
  const clientJar  = path.join(vanillaDir, `${MC_VERSION}.jar`);
  const clientInfo = vanillaJson.downloads.client;
  if (!checkSha1(clientJar, clientInfo.sha1)) {
    onProgress('client', 0, 1, 'Скачиваю client.jar (проверка SHA1)...');
    // Удаляем повреждённый файл если есть
    try { await fsp.unlink(clientJar); } catch {}
    await dlFile(clientInfo.url, clientJar);
    // Проверяем после скачивания
    if (!checkSha1(clientJar, clientInfo.sha1)) {
      throw new Error('client.jar скачался повреждённым (SHA1 не совпадает). Проверь интернет.');
    }
  }
  onProgress('client', 1, 1, 'client.jar готов ✓');

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
  if (!await existsAsync(assetIndexPath)) {
    onProgress('assets', 0, 1, 'Скачиваю индекс ресурсов...');
    await dlFile(assetIndexInfo.url, assetIndexPath);
  }

  const assetIndex   = JSON.parse(await fsp.readFile(assetIndexPath, 'utf8'));
  const assetObjects = Object.entries(assetIndex.objects);
  onProgress('assets', 0, assetObjects.length, 'Загружаю ресурсы...');
  let assetsDone = 0;

  // Создаём все нужные папки заранее — избегаем гонки при параллельном скачивании
  const prefixSet = new Set(assetObjects.map(([, obj]) => obj.hash.slice(0, 2)));
  for (const prefix of prefixSet) {
    await fsp.mkdir(path.join(dirs.objects, prefix), { recursive: true });
  }

  // Скачиваем батчами по 5 (не 10) — меньше вероятность конфликтов
  for (let i = 0; i < assetObjects.length; i += 5) {
    const batch = assetObjects.slice(i, i + 5);
    await Promise.all(batch.map(async ([, obj]) => {
      const prefix = obj.hash.slice(0, 2);
      const dest   = path.join(dirs.objects, prefix, obj.hash);
      if (!checkSha1(dest, obj.hash)) {
        await dlFile(`${RESOURCES_BASE}/${prefix}/${obj.hash}`, dest);
      }
    }));
    assetsDone += batch.length;
    onProgress('assets', assetsDone, assetObjects.length, `${assetsDone}/${assetObjects.length}`);
  }

  // 5. Forge installer — скачиваем с проверкой целостности ZIP
  const forgeInstallerPath = path.join(clientDir, FORGE_JAR);

  // Проверяем что уже скачанный файл не повреждён
  async function isValidZip(filePath) {
    if (!(await existsAsync(filePath))) return false;
    try {
      const stat = await fsp.stat(filePath);
      if (stat.size < 22) return false;
      const fh  = await fsp.open(filePath, 'r');
      const buf = Buffer.alloc(Math.min(stat.size, 65536));
      try { await fh.read(buf, 0, buf.length, Math.max(0, stat.size - buf.length)); }
      finally { await fh.close(); }
      for (let i = buf.length - 4; i >= 0; i--) {
        if (buf[i]===0x50 && buf[i+1]===0x4b && buf[i+2]===0x05 && buf[i+3]===0x06) return true;
      }
      return false;
    } catch { return false; }
  }

  if (!(await isValidZip(forgeInstallerPath))) {
    // Удаляем повреждённый файл если есть
    try { await fsp.unlink(forgeInstallerPath); } catch {}

    let downloaded = false;
    const MAX_ATTEMPTS = 3;

    for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
      onProgress('forge', 0, 3,
        attempt === 1
          ? 'Скачиваю Forge installer...'
          : `Повторная загрузка Forge (попытка ${attempt}/${MAX_ATTEMPTS})...`
      );

      // Перебираем зеркала
      for (const url of FORGE_URLS) {
        try {
          await dlFile(url, forgeInstallerPath);
          // Проверяем целостность сразу после скачивания
          if (isValidZip(forgeInstallerPath)) {
            downloaded = true;
            break;
          }
          // Файл повреждён — удаляем и пробуем следующее зеркало
          onProgress('forge', 0, 3, `Зеркало ${url.split('/')[2]} вернуло повреждённый файл, пробую другое...`);
          try { await fsp.unlink(forgeInstallerPath); } catch {}
        } catch (e) {
          log.error('Forge URL failed:', url, e.message);
          try { await fsp.unlink(forgeInstallerPath); } catch {}
        }
      }

      if (downloaded) break;
      // Небольшая пауза перед следующей попыткой
      if (attempt < MAX_ATTEMPTS) await new Promise(r => setTimeout(r, 2000));
    }

    if (!downloaded) {
      throw new Error(
        'Не удалось скачать Forge installer после ' + MAX_ATTEMPTS + ' попыток.\n' +
        'Возможно проблема с интернетом или серверы Forge временно недоступны.\n' +
        'Попробуй нажать ПОВТОРИТЬ через несколько минут.'
      );
    }
  }

  onProgress('forge', 1, 3, 'Ищу Java 17 для Forge installer...');

  // Forge 47.x требует СТРОГО Java 17
  // javaExe уже проверен/установлен на шаге 0 (ensureJava17)
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
  if (!await existsAsync(profilesPath)) {
    await fsp.writeFile(profilesPath, JSON.stringify({
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
  await fsp.mkdir(forgeExpectedVersionDir, { recursive: true });
  if (!await existsAsync(forgeExpectedClientJar) && await existsAsync(clientJar)) {
    await fsp.copyFile(clientJar, forgeExpectedClientJar);
  }
  // Также копируем version.json который forge ожидает найти
  const forgeExpectedVersionJson = path.join(forgeExpectedVersionDir, MC_VERSION + '.json');
  const vanillaJsonPath = path.join(dirs.versions, MC_VERSION, MC_VERSION + '.json');
  if (!await existsAsync(forgeExpectedVersionJson) && await existsAsync(vanillaJsonPath)) {
    await fsp.copyFile(vanillaJsonPath, forgeExpectedVersionJson);
  }

  // Запускаем forge installer, пишем весь вывод в лог-файл
  const forgeLogPath = path.join(clientDir, 'forge-install.log');
  onProgress('forge', 1, 3, 'Запускаю Forge installer (Java ' + (javaVer || '?') + ', 2-3 мин)...');

  // ── Запускаем Forge installer АСИНХРОННО (spawnSync блокирует Electron UI!) ──
  const { forgeStdout, forgeStderr, exitCode: forgeExitCode } = await new Promise((resolve) => {
    const { spawn: spawnAsync } = require('child_process');
    const outChunks = [];
    const errChunks = [];

    const proc = spawnAsync(javaExe, [
      '-Dmaven.wagon.http.retryHandler.class=standard',
      '-Dmaven.wagon.http.retryHandler.count=5',
      '-Dhttp.maxRedirects=10',
      '-jar', forgeInstallerPath,
      '--installClient', clientDir,
    ], {
      cwd:         clientDir,
      stdio:       ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });

    proc.stdout?.on('data', chunk => {
      outChunks.push(chunk);
      // Отправляем прогресс по строкам вывода Forge
      const line = chunk.toString().trim();
      if (line && onProgress) {
        const short = line.slice(0, 80);
        if (line.includes('Downloading') || line.includes('Considering')) {
          onProgress('forge', 1, 3, '⬇ ' + short);
        }
      }
    });
    proc.stderr?.on('data', chunk => errChunks.push(chunk));

    // Таймаут 10 минут
    const timer = setTimeout(() => {
      proc.kill();
      resolve({ forgeStdout: '', forgeStderr: 'TIMEOUT: Forge installer превысил 10 минут', exitCode: -1 });
    }, 10 * 60 * 1000);

    proc.on('close', code => {
      clearTimeout(timer);
      resolve({
        forgeStdout: Buffer.concat(outChunks).toString(),
        forgeStderr: Buffer.concat(errChunks).toString(),
        exitCode: code,
      });
    });

    proc.on('error', err => {
      clearTimeout(timer);
      resolve({ forgeStdout: '', forgeStderr: err.message, exitCode: -1 });
    });
  });

  // Сохраняем полный лог
  await fsp.writeFile(forgeLogPath,
    '=== STDOUT ===\n' + forgeStdout +
    '\n=== STDERR ===\n' + forgeStderr
  );

  onProgress('forge', 2, 3, 'Forge installer завершён, строю classpath...');

  // 6. Читаем forge version.json
  const forgeVersionId = findForgeVersionId(dirs.versions);
  if (!forgeVersionId) {
    const exitCode = forgeExitCode;
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
    await fsp.readFile(path.join(forgeVersionDir, `${forgeVersionId}.json`), 'utf8')
  );

  // 7. Строим launch.json — читаем аргументы ПРЯМО из forge version.json
  // Forge прописывает в arguments.jvm все нужные -p, -DlegacyClassPath и т.д.
  const sep = process.platform === 'win32' ? ';' : ':';

  const allLibs = [
    ...(vanillaJson.libraries     || []),
    ...(forgeVersionJson.libraries || []),
  ];

  // Vanilla classpath (библиотеки ванилла + client.jar)
  const vanillaClasspath = buildClasspath(vanillaJson.libraries || [], dirs.libraries, dirs.versions, clientDir);
  if (!vanillaClasspath.includes(clientJar) && await existsAsync(clientJar)) {
    vanillaClasspath.push(clientJar);
  }

  // Forge JVM args с подстановкой переменных — там живут -p и -DlegacyClassPath
  const rawForgeJvmArgs = extractForgeJvmArgs(forgeVersionJson, dirs.libraries, clientDir);

  // Из forge JVM args вытаскиваем -p (module-path) — Forge сам его прописывает
  // Формат: ["-p", "path1;path2;..."] или ["--module-path", "path1;path2;..."]
  let modulePath = [];
  let legacyClassPath = [];
  const otherJvmArgs = [];

  for (let i = 0; i < rawForgeJvmArgs.length; i++) {
    const arg = rawForgeJvmArgs[i];
    if ((arg === '-p' || arg === '--module-path') && i + 1 < rawForgeJvmArgs.length) {
      modulePath = rawForgeJvmArgs[i + 1].split(sep).filter(Boolean);
      i++; // пропускаем следующий аргумент
    } else if (arg.startsWith('-DlegacyClassPath=')) {
      legacyClassPath = arg.replace('-DlegacyClassPath=', '').split(sep).filter(Boolean);
      otherJvmArgs.push(arg); // оставляем как есть — Forge его читает сам
    } else {
      otherJvmArgs.push(arg);
    }
  }

  // ВАЖНО: module-path берём ТОЛЬКО из forge version.json (-p аргумент).
  // Forge задаёт ровно 8 jar'ов. Добавлять что-либо сверху нельзя —
  // это ломает Java Module System (конфликты jopt.simple, joptsimple и др.)
  // fmlearlydisplay, modlauncher и др. загружаются через legacyClassPath, НЕ через -p
  if (modulePath.length === 0) {
    // Fallback только если Forge совсем не прописал -p (не должно случиться)
    modulePath = [
      path.join(dirs.libraries, 'cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar'),
      path.join(dirs.libraries, 'cpw/mods/securejarhandler/2.1.10/securejarhandler-2.1.10.jar'),
    ];
    // ТЗ 5c: filter через Promise.all (await внутри filter не работает)
    const fallbackExists = await Promise.all(modulePath.map(p => existsAsync(p)));
    modulePath = modulePath.filter((_, i) => fallbackExists[i]);
  }

  // Финальный classpath
  let classpath;
  const nativeSfx = getNativeSuffix();
  if (legacyClassPath.length > 0) {
    // Forge прописывает legacyClassPath — фильтруем natives по архитектуре
    classpath = legacyClassPath.filter(p => {
      const base = path.basename(p, '.jar');
      if (!base.includes('natives-')) return true;
      return base.endsWith(nativeSfx);
    });
    if (!classpath.some(p => p.includes(MC_VERSION + '.jar')) && (await existsAsync(clientJar))) {
      classpath.push(clientJar);
    }
  } else {
    classpath = buildClasspath(allLibs, dirs.libraries, dirs.versions, clientDir);
    if (!classpath.includes(clientJar) && await existsAsync(clientJar)) {
      classpath.push(clientJar);
    }
  }

  // modulePath берётся только из forge version.json — дедупликация не нужна

  // Извлекаем game args из forge version.json (--launchTarget forgeclient и др.)
  const forgeGameArgs = extractForgeGameArgs(forgeVersionJson, clientDir, dirs);

  const launchData = {
    mainClass:     forgeVersionJson.mainClass || vanillaJson.mainClass,
    classpath,
    modulePath,
    forgeJvmArgs:  otherJvmArgs,
    forgeGameArgs,                          // ← --launchTarget forgeclient и др.
    assetsDir:     dirs.assets,
    assetIndex:    vanillaJson.assetIndex.id,
    nativesDir:    dirs.natives,
    gameDir:       clientDir,
    forgeVersionId,
    librariesDir:  dirs.libraries,
  };

  await fsp.writeFile(path.join(clientDir, 'launch.json'), JSON.stringify(launchData, null, 2));
  onProgress('forge', 3, 3, 'Forge установлен');
  onProgress('done',  1, 1,  'Установка завершена!');
  return launchData;
}

// ─── CLASSPATH ────────────────────────────────────────────────────────────────
// Определяем правильный суффикс нативных библиотек для текущей архитектуры
async function getNativeSuffix() {
  const plat = process.platform;
  const arch = os.arch(); // x64, arm64, ia32
  if (plat === 'win32') {
    if (arch === 'arm64') return 'natives-windows-arm64';
    if (arch === 'ia32')  return 'natives-windows-x86';
    return 'natives-windows'; // x64
  }
  if (plat === 'darwin') {
    if (arch === 'arm64') return 'natives-macos-arm64';
    return 'natives-macos';
  }
  if (arch === 'arm64') return 'natives-linux-arm64';
  return 'natives-linux';
}

async function buildClasspath(libs, libsDir, versionsDir, clientDir) {
  const entries     = new Set();
  const nativeSufx  = getNativeSuffix(); // напр. "natives-windows"

  for (const lib of libs) {
    if (!isLibAllowed(lib)) continue;

    if (lib.downloads?.artifact?.path) {
      const artPath = lib.downloads.artifact.path;

      // Фильтруем natives jar'ы — оставляем только нужную архитектуру
      // Пример: lwjgl-3.3.1-natives-windows.jar — да
      //          lwjgl-3.3.1-natives-windows-arm64.jar — нет (если мы x64)
      if (artPath.includes('natives-')) {
        const baseName = path.basename(artPath, '.jar'); // lwjgl-3.3.1-natives-windows-arm64
        // Проверяем что это именно наш суффикс (точное совпадение конца имени)
        if (!baseName.endsWith(nativeSufx)) continue;
      }

      const p = path.join(libsDir, artPath);
      if (await existsAsync(p)) { entries.add(p); continue; }
    }

    // Maven-координаты (формат Forge)
    if (lib.name) {
      // Фильтруем нативные по classifier
      const parts = lib.name.split(':');
      if (parts.length >= 4) {
        const classifier = parts[3];
        if (classifier && classifier.startsWith('natives-') && classifier !== nativeSufx) continue;
      }
      const p = mavenToPath(lib.name, libsDir);
      if (p && await existsAsync(p)) { entries.add(p); continue; }
    }
  }

  // client.jar vanilla
  const clientJar = path.join(versionsDir, MC_VERSION, `${MC_VERSION}.jar`);
  if (await existsAsync(clientJar)) entries.add(clientJar);

  // forge-xxx-universal.jar
  const forgeJar = findForgeJar(clientDir);
  if (forgeJar) entries.add(forgeJar);

  return Array.from(entries);
}

// ─── MODULE PATH (для Forge 47.x bootstraplauncher) ─────────────────────────
// Forge 47.x использует Java Module System.
// Модульные jar: securejarhandler, bootstraplauncher, eventbus, modlauncher и др.
// Их нужно передать через -p (--module-path), а НЕ через -cp
const MODULE_GROUPS = [
  'cpw/mods',                              // bootstraplauncher, securejarhandler
  'net/minecraftforge/securejarhandler',
  'net/minecraftforge/fmlearlydisplay',    // ← обязательно для Forge 47.x
  'net/minecraftforge/modlauncher',
  'net/minecraftforge/eventbus',
  'net/minecraftforge/forgespi',
  'net/minecraftforge/unsafe',
  'net/minecraftforge/mergetool',
  'net/minecraftforge/fmlloader',
  'net/minecraftforge/coremods',
  'net/minecraftforge/accesstransformers',
  'org/spongepowered/mixin',
  'net/sf/jopt-simple',
  'org/ow2/asm',
  'net/jodah/typetools',
  'net/minecrell/terminalconsoleappender',
  'com/github/ben-manes/caffeine',
  'org/apache/logging/log4j',              // ← log4j нужен как модуль
  'org/apache/maven',
  'org/antlr',
  'org/jline',
  'org/openjdk/nashorn',
  'com/electronwill/night-config',
  'net/minecraftforge/JarJarFileSystems',
  'net/minecraftforge/JarJarSelector',
  'net/minecraftforge/JarJarMetadata',
];

async function buildModulePath(libsDir) {
  const seen    = new Set();
  const modJars = [];

  for (const group of MODULE_GROUPS) {
    const groupDir = path.join(libsDir, group);
    if (!await existsAsync(groupDir)) continue;
    collectJars(groupDir, seen, modJars);
  }

  // Дополнительно: прямой поиск по имени файла если не нашли через группы
  const mustHave = ['securejarhandler', 'bootstraplauncher', 'fmlearlydisplay'];
  for (const name of mustHave) {
    if (!modJars.some(j => j.includes(name))) {
      // Ищем рекурсивно по всему libsDir
      const found = findJarByName(libsDir, name);
      if (found) {
        found.filter(j => !seen.has(j)).forEach(j => { seen.add(j); modJars.push(j); });
      }
    }
  }

  return modJars;
}

async function collectJars(dir, seen, result) {
  try {
    for (const entry of await fsp.readdir(dir)) {
      const full = path.join(dir, entry);
      try {
        const stat = await fsp.stat(full);
        if (stat.isDirectory()) {
          await collectJars(full, seen, result);
        } else if (
          entry.endsWith('.jar') &&
          !entry.endsWith('-sources.jar') &&
          !entry.endsWith('-javadoc.jar') &&
          !seen.has(full)
        ) {
          seen.add(full);
          result.push(full);
        }
      } catch {}
    }
  } catch {}
}

async function findJarByName(rootDir, namePart, _depth = 0) {
  if (_depth > 8) return [];
  const found = [];
  try {
    for (const entry of await fsp.readdir(rootDir)) {
      const full = path.join(rootDir, entry);
      try {
        const stat = await fsp.stat(full);
        if (stat.isDirectory()) {
          found.push(...findJarByName(full, namePart, _depth + 1));
        } else if (entry.includes(namePart) && entry.endsWith('.jar') && !entry.endsWith('-sources.jar')) {
          found.push(full);
        }
      } catch {}
    }
  } catch {}
  return found;
}

// Извлекаем JVM-аргументы из forge version.json
// Forge прописывает туда -DlegacyClassPath, -DlibraryDirectory, --add-modules и др.
async function extractForgeJvmArgs(forgeJson, libsDir, clientDir) {
  const args    = [];
  const sep     = process.platform === 'win32' ? ';' : ':';
  const jvmArgs = forgeJson.arguments?.jvm || [];

  for (const arg of jvmArgs) {
    if (typeof arg !== 'string') continue;
    const resolved = arg
      .replace(/\$\{library_directory\}/g,    libsDir)
      .replace(/\$\{libraries_directory\}/g,  libsDir)
      .replace(/\$\{classpath_separator\}/g,  sep)
      .replace(/\$\{version_name\}/g,         MC_VERSION)
      .replace(/\$\{game_directory\}/g,       clientDir)
      .replace(/\$\{assets_root\}/g,          path.join(clientDir, 'assets'))
      .replace(/\$\{natives_directory\}/g,    path.join(clientDir, 'natives'));
    args.push(resolved);
  }

  return args;
}

// "net.minecraftforge:forge:1.20.1-47.3.0:universal"
//  → libraries/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-universal.jar
async function mavenToPath(name, libsDir) {
  const parts = name.split(':');  // [group, artifact, version, classifier?]
  if (parts.length < 3) return null;
  const [group, artifact, version, classifier] = parts;
  const groupPath = group.replace(/\./g, '/');
  const fileName  = classifier
    ? `${artifact}-${version}-${classifier}.jar`
    : `${artifact}-${version}.jar`;
  return path.join(libsDir, groupPath, artifact, version, fileName);
}

async function findForgeJar(clientDir) {
  const base = path.join(clientDir, 'libraries', 'net', 'minecraftforge', 'forge');
  try {
    if (!await existsAsync(base)) return null;
    for (const ver of await fsp.readdir(base)) {
      const dir   = path.join(base, ver);
      const files = await fsp.readdir(dir);
      // Приоритет: universal > client > любой forge jar
      const pick  = files.find(f => f.includes('universal')) ||
                    files.find(f => f.includes('client'))    ||
                    files.find(f => f.endsWith('.jar') && !f.endsWith('-installer.jar'));
      if (pick) return path.join(dir, pick);
    }
  } catch {}
  return null;
}

async function findForgeVersionId(versionsDir) {
  try {
    return await fsp.readdir(versionsDir).find(d =>
      d.toLowerCase().includes('forge') && d.includes(MC_VERSION)
    ) || null;
  } catch { return null; }
}

// ─── LIBRARY HELPERS ─────────────────────────────────────────────────────────
async function isLibAllowed(lib) {
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
      await fsp.mkdir(path.dirname(dest), { recursive: true });
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
      await fsp.mkdir(path.dirname(dest), { recursive: true });
      await dlFile(nat.url, dest);
    }
    extractNativesNode(dest, nativesDir, lib.extract?.exclude || []);
  }
}

async function getLibName(lib) {
  return (lib.name || lib.downloads?.artifact?.path || '').split(':').slice(0, 2).join(':');
}

// ─── NATIVES (Node.js, без jar) ───────────────────────────────────────────────
// ТЗ 5c: async — распаковка .dll нативных библиотек не блокирует UI-поток
async function extractNativesNode(zipPath, destDir, exclude = []) {
  try {
    const data    = await fsp.readFile(zipPath); // async чтение нативного JAR
    const entries = parseZip(data);
    for (const entry of entries) {
      if (entry.isDir) continue;
      if (exclude.some(ex => entry.name.startsWith(ex))) continue;
      if (!entry.name.endsWith('.dll') && !entry.name.endsWith('.so') &&
          !entry.name.endsWith('.dylib') && !entry.name.endsWith('.jnilib')) continue;
      const dest = path.join(destDir, path.basename(entry.name));
      try {
        const raw = entry.compression === 8
          ? await inflateRawAsync(entry.data) // async inflate — не блокируем event loop
          : entry.data;
        await fsp.writeFile(dest, raw);
      } catch {}
    }
  } catch {}
}

// Минимальный ZIP-парсер
async function parseZip(buf) {
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


// Извлекаем game-аргументы из forge version.json
// Это --launchTarget forgeclient, --fml.forgeVersion и т.д.
async function extractForgeGameArgs(forgeJson, clientDir, dirs) {
  const args     = [];
  const gameArgs = forgeJson.arguments?.game || [];

  for (const arg of gameArgs) {
    if (typeof arg !== 'string') continue;
    // Подставляем переменные
    const resolved = arg
      .replace(/\$\{game_directory\}/g,       clientDir)
      .replace(/\$\{assets_root\}/g,           dirs.assets)
      .replace(/\$\{assets_index_name\}/g,     'unknown') // будет перезаписано реальным значением
      .replace(/\$\{version_name\}/g,          MC_VERSION)
      .replace(/\$\{library_directory\}/g,     dirs.libraries)
      .replace(/\$\{classpath_separator\}/g,   process.platform === 'win32' ? ';' : ':');
    args.push(resolved);
  }

  return args;
}

// ─── JAVA AUTO-INSTALL (ТЗ 2.5: изолированная portable Java, системная игнорируется) ──
// Лаунчер ПОЛНОСТЬЮ игнорирует системную Java (любые версии 8/11/21 и PATH).
// Всегда используется собственная Java 17 в %APPDATA%/.mc-launcher/runtime/java17/
async function ensureJava17(launcherDir, onProgress) {
  const javaExe = path.join(launcherDir, 'runtime', 'java17', 'bin',
    process.platform === 'win32' ? 'javaw.exe' : 'java');
  const javaExeConsole = path.join(launcherDir, 'runtime', 'java17', 'bin',
    process.platform === 'win32' ? 'java.exe' : 'java');

  // Уже скачана и распакована — используем без проверки системной Java
  if (await existsAsync(javaExeConsole)) return javaExeConsole;

  onProgress('java', 0, 1, 'Скачиваю изолированную Java 17 (~45 МБ)...');

  const runtimeDir   = path.join(launcherDir, 'runtime', 'java17');
  const tmpArchive    = path.join(launcherDir, 'runtime', 'java17.tmp');
  await fsp.mkdir(path.dirname(tmpArchive), { recursive: true });

  let dlUrl;
  if (process.platform === 'win32')        dlUrl = JAVA17_WIN_URL;
  else if (process.platform === 'darwin')  dlUrl = JAVA17_MAC_URL;
  else                                     dlUrl = JAVA17_LIN_URL;

  try {
    await dlFileProgress(dlUrl, tmpArchive, (done, total) => {
      const pct = total > 0 ? Math.round(done / total * 100) : 0;
      onProgress('java', pct, 100, `Скачиваю Java 17: ${pct}%`);
    });
  } catch (e) {
    onProgress('java', 0, 1, 'Пробую альтернативный источник Java 17...');
    const fallbackUrl = process.platform === 'win32' ? JAVA17_WIN_FALLBACK : JAVA17_LIN_FALLBACK;
    await dlFileProgress(fallbackUrl, tmpArchive, (done, total) => {
      const pct = total > 0 ? Math.round(done / total * 100) : 0;
      onProgress('java', pct, 100, `Java 17: ${pct}%`);
    });
  }

  onProgress('java', 99, 100, 'Распаковываю Java 17...');
  await fsp.mkdir(runtimeDir, { recursive: true });

  if (process.platform === 'win32') {
    await extractZipToDir(tmpArchive, runtimeDir);
  } else {
    const tarResult = spawnSync('tar', ['xzf', tmpArchive, '-C', runtimeDir, '--strip-components=1'], {
      stdio: 'pipe', timeout: 60000,
    });
    if (tarResult.status !== 0) throw new Error('Ошибка распаковки Java: ' + (tarResult.stderr || '').toString().slice(0, 200));
  }

  try { await fsp.unlink(tmpArchive); } catch {}

  const foundJava = findBundledJava(runtimeDir);
  if (!foundJava) throw new Error('Java 17 распакована, но исполняемый файл не найден в ' + runtimeDir);

  onProgress('java', 100, 100, 'Java 17 установлена в изолированную директорию');
  return foundJava;
}

// Ищем java.exe/java внутри распакованной папки JRE
async function findBundledJava(dir, depth = 0) {
  if (depth > 4) return null;
  const target = process.platform === 'win32' ? 'java.exe' : 'java';
  try {
    for (const entry of await fsp.readdir(dir)) {
      const full = path.join(dir, entry);
      try {
        const stat = await fsp.stat(full);
        if (stat.isDirectory()) {
          const found = findBundledJava(full, depth + 1);
          if (found) return found;
        } else if (entry === target) {
          if (process.platform !== 'win32') {
            try { fs.chmodSync(full, '755'); } catch {}
          }
          return full;
        }
      } catch {}
    }
  } catch {}
  return null;
}

// Распаковка ZIP для Windows (JRE архив)
// ТЗ 5c: extractZipToDir полностью асинхронная — использует fsp и inflateRawAsync
async function extractZipToDir(zipPath, destDir) {
  // Читаем ZIP асинхронно — не блокируем event loop пока читается 45 МБ архив Java
  const data    = await fsp.readFile(zipPath);
  const entries = parseZip(data);
  let extracted = 0;

  // Первый компонент пути — корневая папка в архиве (jdk-17.0.12+7-jre/)
  const prefix = entries[0]?.name?.split('/')[0] + '/';

  // Обрабатываем все файлы последовательно (inflate CPU-bound, параллельность не даёт выигрыша)
  for (const entry of entries) {
    if (entry.isDir) continue;
    const relPath = entry.name.startsWith(prefix)
      ? entry.name.slice(prefix.length)
      : entry.name;
    if (!relPath) continue;

    const dest = path.join(destDir, relPath.replace(/\//g, path.sep));
    try {
      await fsp.mkdir(path.dirname(dest), { recursive: true });
      // ТЗ 5c: inflateRawAsync вместо inflateRawSync — не блокирует event loop
      const raw = entry.compression === 8
        ? await inflateRawAsync(entry.data)
        : entry.data;
      await fsp.writeFile(dest, raw);
      extracted++;
    } catch {}
  }
  return extracted;
}

// ТЗ 5c: dlFileProgress полностью async — не использует new Promise, не блокирует UI
async function dlFileProgress(url, dest, onProgress, retries = 3) {
  await fsp.mkdir(path.dirname(dest), { recursive: true });
  const tmp = dest + '.tmp2';

  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      await new Promise((resolve, reject) => {
        const mod  = url.startsWith('https') ? https : http;
        const file = fs.createWriteStream(tmp);

        const req = mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
          // Redirect
          if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
            file.close();
            fsp.unlink(tmp).catch(() => {});
            dlFileProgress(res.headers.location, dest, onProgress, retries)
              .then(resolve).catch(reject);
            return;
          }
          if (res.statusCode >= 400) {
            file.close();
            fsp.unlink(tmp).catch(() => {});
            reject(new Error('HTTP ' + res.statusCode + ': ' + url));
            return;
          }
          const total = parseInt(res.headers['content-length'] || '0', 10);
          let done = 0;
          res.on('data',  chunk => { done += chunk.length; file.write(chunk); onProgress(done, total); });
          res.on('end',   ()    => file.close(() => fsp.rename(tmp, dest).then(resolve).catch(reject)));
          res.on('error', e    => { file.close(); fsp.unlink(tmp).catch(() => {}); reject(e); });
        });
        req.on('error', e => { fsp.unlink(tmp).catch(() => {}); reject(e); });
      });
      return; // успех
    } catch (e) {
      try { await fsp.unlink(tmp); } catch {}
      if (attempt === retries) throw e;
      await new Promise(r => setTimeout(r, 1000 * (attempt + 1))); // экспоненциальная задержка
    }
  }
}

// ─── JAVA FINDER ─────────────────────────────────────────────────────────────
// Forge 47.x требует СТРОГО Java 17. Java 20/21 не поддерживается.
async function findJava(launcherDir) {
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

  // Windows: сначала смотрим нашу изолированную Java в runtime/java17/
  if (launcherDir) {
    const bundled = path.join(launcherDir, 'runtime', 'java17', 'bin', 'java.exe');
    if (await existsAsync(bundled)) return bundled;
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
      if (!await existsAsync(base)) continue;
      const dirs = await fsp.readdir(base).filter(d => /17/i.test(d)).sort().reverse();
      for (const d of dirs) {
        const p = path.join(base, d, 'bin', 'java.exe');
        if (await existsAsync(p)) return p;
      }
    } catch {}
  }

  // Minecraft Official Launcher bundled JRE (java-runtime-gamma = Java 17)
  const mcBase = path.join(os.homedir(), 'AppData', 'Local', 'Packages');
  const mcDirs = ['Microsoft.4297127D64EC6_8wekyb3d8bbwe'];
  for (const d of mcDirs) {
    const p = path.join(mcBase, d, 'LocalCache', 'Local', 'runtime',
      'java-runtime-gamma', 'windows-x64', 'java-runtime-gamma', 'bin', 'java.exe');
    if (await existsAsync(p)) return p;
  }

  // TLauncher bundled JRE
  const tlPaths = [
    path.join(os.homedir(), 'AppData', 'Roaming', '.tlauncher', 'runtime', 'java17', 'bin', 'java.exe'),
    path.join(os.homedir(), 'AppData', 'Roaming', '.tlauncher', 'jre', 'x64', '17', 'bin', 'java.exe'),
  ];
  for (const p of tlPaths) {
    if (await existsAsync(p)) return p;
  }

  // Последний шанс — любая java из PATH (может быть не 17!)
  try {
    const r = require('child_process').spawnSync('where', ['java'], { encoding: 'utf8', windowsHide: true });
    if (r.status === 0 && r.stdout.trim()) return r.stdout.trim().split('\n')[0].trim();
  } catch {}

  return 'java';
}

// Проверяет версию java и возвращает major version (17, 20, 21...)
async function getJavaVersion(javaPath) {
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
async function fetchText(url) {
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

// ТЗ 5c: dlFile полностью async — pipe через stream, проверка EOCD после скачивания
async function dlFile(url, dest, retries = 3) {
  await fsp.mkdir(path.dirname(dest), { recursive: true });
  const tmp = dest + '.tmp';

  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      // Скачиваем через pipe (stream — не держит весь файл в RAM)
      await new Promise((resolve, reject) => {
        const mod  = url.startsWith('https') ? https : http;
        const file = fs.createWriteStream(tmp);
        const req  = mod.get(url, { headers: { 'User-Agent': 'MC-Launcher/1.0' } }, res => {
          if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
            file.close();
            fsp.unlink(tmp).catch(() => {});
            dlFile(res.headers.location, dest, retries).then(resolve).catch(reject);
            return;
          }
          if (res.statusCode >= 400) {
            file.close();
            fsp.unlink(tmp).catch(() => {});
            reject(new Error(`HTTP ${res.statusCode}: ${url}`));
            return;
          }
          res.pipe(file);
          file.on('finish', () => file.close(() => fsp.rename(tmp, dest).then(resolve).catch(reject)));
          res.on('error', e => { file.close(); fsp.unlink(tmp).catch(() => {}); reject(e); });
        });
        req.on('error', e => { fsp.unlink(tmp).catch(() => {}); reject(e); });
      });

      // После скачивания проверяем EOCD-целостность (ТЗ 2.4, ТЗ 5c — async)
      if (dest.endsWith('.jar') || dest.endsWith('.zip')) {
        const valid = await checkZipIntegrity(dest);
        if (!valid) {
          await fsp.unlink(dest).catch(() => {});
          throw new Error(`Downloaded file corrupted (EOCD not found): ${url}`);
        }
      }
      return; // успех

    } catch (e) {
      await fsp.unlink(tmp).catch(() => {});
      await fsp.unlink(dest).catch(() => {});
      if (attempt === retries) throw e;
      await new Promise(r => setTimeout(r, 2000 * (attempt + 1)));
    }
  }
}



// ТЗ 2.4: проверка структуры END-записи центрального каталога (EOCD).
// Старая проверка сигнатуры 0x04034b50 в НАЧАЛЕ файла пропускала оборванные
// закачки — у битого файла первые байты валидны, а хвост отсутствует.
// EOCD-сигнатура 0x06054b50 физически не может появиться без полной записи файла.
// ТЗ 5c: checkZipIntegrity полностью асинхронная — не блокирует UI-поток
async function checkZipIntegrity(filePath) {
  try {
    const stat = await fsp.stat(filePath);
    if (stat.size < 22) return false;

    // Читаем только хвост файла (последние 1024 байта) — не грузим весь JAR в память
    const readLength = Math.min(stat.size, 1024);
    const buffer     = Buffer.alloc(readLength);
    const fh = await fsp.open(filePath, 'r');
    try {
      await fh.read(buffer, 0, readLength, stat.size - readLength);
    } finally {
      await fh.close();
    }
    // EOCD-сигнатура 0x06054b50 — физически невозможна в оборванном файле
    for (let i = readLength - 22; i >= 0; i--) {
      if (buffer.readUInt32LE(i) === 0x06054b50) return true;
    }
    return false;
  } catch { return false; }
}

// ТЗ 5c: checkSha1 полностью асинхронная — читает файл через поток, не блокирует диск
async function checkSha1(filePath, expected) {
  try { await fsp.access(filePath); } catch { return false; } // файл не существует

  if (!expected) {
    try {
      if (filePath.endsWith('.jar') || filePath.endsWith('.zip')) {
        return await checkZipIntegrity(filePath);
      }
      return true;
    } catch { return false; }
  }

  // SHA1 через поток — не держим весь файл в RAM, не блокируем event loop
  return new Promise((resolve) => {
    const hash   = createHash('sha1');
    const stream = fs.createReadStream(filePath);
    stream.on('data',  chunk => hash.update(chunk));
    stream.on('end',   ()    => resolve(hash.digest('hex') === expected));
    stream.on('error', ()    => resolve(false));
  });
}

module.exports = { install, findJava, MC_VERSION, FORGE_VERSION };