'use strict';
/**
 * updater.js — Мини-апдейтер ViLauncher
 *
 * Этот скрипт запускается ВМЕСТО лаунчера.
 * Алгоритм:
 *   1. Проверяет GitHub Releases на наличие новой версии
 *   2. Если есть — скачивает установщик (Setup.exe) в temp
 *   3. Запускает установщик тихо (/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-)
 *   4. Ждёт завершения установки
 *   5. Запускает обновлённый лаунчер
 *   6. Если обновлений нет — сразу запускает лаунчер
 *
 * Сборка в exe:
 *   npm install -g pkg
 *   pkg updater.js --target node18-win-x64 --output ViLauncherUpdater.exe
 */

const https   = require('https');
const http    = require('http');
const fs      = require('fs');
const path    = require('path');
const os      = require('os');
const { execFileSync, spawn } = require('child_process');

// ── Настройки ─────────────────────────────────────────────────────────────────
const GH_OWNER   = 'ViPok137';
const GH_REPO    = 'ViLauncher';
const VERSION_FILE = path.join(
  process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'),
  'ViLauncher', 'version.txt'
);
const LAUNCHER_EXE = path.join(
  process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'),
  'ViLauncher', 'ViLauncher.exe'
);
const TMP_DIR = path.join(os.tmpdir(), 'vilauncher-update');

// ── Утилиты ───────────────────────────────────────────────────────────────────
function fetchJson(url) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    mod.get(url, {
      headers: {
        'User-Agent': 'ViLauncher-Updater/1.0',
        'Accept':     'application/vnd.github.v3+json',
      }
    }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location)
        return fetchJson(res.headers.location).then(resolve).catch(reject);
      let data = '';
      res.on('data', c => data += c);
      res.on('end',  () => {
        try { resolve(JSON.parse(data)); }
        catch (e) { reject(new Error('JSON parse error: ' + data.slice(0, 100))); }
      });
    }).on('error', reject);
  });
}

function dlFile(url, dest, onPct) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http;
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    const tmp  = dest + '.tmp';
    const file = fs.createWriteStream(tmp);
    mod.get(url, { headers: { 'User-Agent': 'ViLauncher-Updater/1.0' } }, res => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        file.close(); fs.unlink(tmp, () => {});
        return dlFile(res.headers.location, dest, onPct).then(resolve).catch(reject);
      }
      if (res.statusCode >= 400) { reject(new Error('HTTP ' + res.statusCode)); return; }
      const total = parseInt(res.headers['content-length'] || '0', 10);
      let done = 0;
      res.on('data', chunk => {
        done += chunk.length;
        file.write(chunk);
        if (onPct && total > 0) onPct(Math.round(done / total * 100));
      });
      res.on('end',   () => file.close(() => fs.rename(tmp, dest, e => e ? reject(e) : resolve())));
      res.on('error', e => { file.close(); fs.unlink(tmp, () => {}); reject(e); });
    }).on('error', e => { file.close(); fs.unlink(tmp, () => {}); reject(e); });
  });
}

function getCurrentVersion() {
  try { return fs.readFileSync(VERSION_FILE, 'utf8').trim(); } catch { return '0.0.0'; }
}

function launchLauncher() {
  if (!fs.existsSync(LAUNCHER_EXE)) {
    console.error('[updater] Лаунчер не найден:', LAUNCHER_EXE);
    process.exit(1);
  }
  const child = spawn(LAUNCHER_EXE, [], {
    detached: true,
    stdio:    'ignore',
    windowsHide: false,
  });
  child.unref();
  process.exit(0);
}

// ── Главная логика ────────────────────────────────────────────────────────────
async function main() {
  console.log('[updater] ViLauncher Updater запущен');

  let rel;
  try {
    rel = await fetchJson(`https://api.github.com/repos/${GH_OWNER}/${GH_REPO}/releases/latest`);
  } catch (e) {
    console.warn('[updater] Нет интернета или GitHub недоступен:', e.message);
    launchLauncher(); // запускаем лаунчер без обновления
    return;
  }

  const remoteVer = (rel.tag_name || '').replace(/^v/, '');
  const localVer  = getCurrentVersion();

  console.log(`[updater] Версии: локальная=${localVer}, удалённая=${remoteVer}`);

  if (!remoteVer || remoteVer === localVer) {
    console.log('[updater] Обновлений нет, запускаю лаунчер');
    launchLauncher();
    return;
  }

  // Ищем Setup .exe в релизе
  const asset = (rel.assets || []).find(a => {
    const n = a.name.toLowerCase();
    return (n.includes('setup') || n.includes('install')) && n.endsWith('.exe');
  }) || (rel.assets || []).find(a => a.name.toLowerCase().endsWith('.exe'));

  if (!asset) {
    console.warn('[updater] Установщик не найден в релизе, запускаю текущую версию');
    launchLauncher();
    return;
  }

  console.log(`[updater] Скачиваю обновление ${remoteVer}: ${asset.name}`);
  const setupDest = path.join(TMP_DIR, asset.name);

  try {
    await dlFile(asset.browser_download_url, setupDest, pct => {
      process.stdout.write(`\r[updater] Скачиваю... ${pct}%  `);
    });
    console.log('\n[updater] Скачан:', setupDest);
  } catch (e) {
    console.error('[updater] Ошибка скачивания:', e.message);
    launchLauncher();
    return;
  }

  // Устанавливаем тихо через Inno Setup flags
  console.log('[updater] Устанавливаю обновление...');
  try {
    execFileSync(setupDest, [
      '/VERYSILENT',
      '/SUPPRESSMSGBOXES',
      '/NORESTART',
      '/SP-',
      '/LOG=' + path.join(TMP_DIR, 'install.log'),
    ], {
      windowsHide: false,
      timeout:     5 * 60 * 1000, // 5 минут максимум
    });
    console.log('[updater] Установка завершена');

    // Сохраняем новую версию
    fs.mkdirSync(path.dirname(VERSION_FILE), { recursive: true });
    fs.writeFileSync(VERSION_FILE, remoteVer);
  } catch (e) {
    console.error('[updater] Ошибка установки:', e.message);
    // Даже если установка упала — пробуем запустить лаунчер
  }

  // Небольшая пауза — установщик Inno Setup может ещё работать
  await new Promise(r => setTimeout(r, 1000));

  // Удаляем скачанный установщик
  try { fs.unlinkSync(setupDest); } catch {}

  console.log('[updater] Запускаю обновлённый лаунчер');
  launchLauncher();
}

main().catch(e => {
  console.error('[updater] Критическая ошибка:', e);
  launchLauncher(); // в любом случае запускаем лаунчер
});
