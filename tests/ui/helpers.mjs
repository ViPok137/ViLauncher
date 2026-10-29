import { JSDOM } from 'jsdom';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const rendererDir = join(root, 'src', 'renderer');
export const sleep = ms => new Promise(r => setTimeout(r, ms));

// Окна jsdom держат таймеры (пинг серверов раз в 30 с) — после каждого теста их нужно закрывать.
const openWindows = [];
export function closeAll() {
  for (const w of openWindows.splice(0)) { try { w.close(); } catch { /* уже закрыто */ } }
}

/** Карточки серверов так, как их отдаёт бэкенд (list_servers): servers.json + installed/selected/playable. */
export function serverViews(selected = 'create', installed = { create: true }) {
  const file = JSON.parse(readFileSync(join(root, 'servers.json'), 'utf8'));
  return file.servers.map(s => ({ ...s, type: s.type || 'modded', instanceId: s.instanceId || s.id,
    installed: !!installed[s.id], selected: s.id === selected, playable: !!s.active }));
}

/**
 * Загружает index.html с настоящими скриптами лаунчера (renderer.js + game-settings.js) и заглушкой window.api.
 * `overrides` — свои реализации методов; все вызовы пишутся в `calls`.
 */
export async function loadLauncher(overrides = {}, { scripts = ['renderer.js', 'game-settings.js'] } = {}) {
  const html = readFileSync(join(rendererDir, 'index.html'), 'utf8');
  const dom = new JSDOM(html, { runScripts: 'dangerously', pretendToBeVisual: true, url: 'http://localhost/' });
  const w = dom.window, calls = [], cbs = {};
  openWindows.push(w);
  let selected = 'create';
  const views = () => serverViews(selected);
  const defaults = {
    authGetUser: 'Vik', authGetSaved: { username: '', password: '' }, getNick: 'Vik', setNick: { ok: true },
    installCheck: { installed: true }, installStart: { ok: true },
    modsSync: { ok: true, updated: false, msg: 'Моды актуальны (1.0.0)' },
    listServers: () => ({ ok: true, selected, servers: views() }),
    selectServer: id => { selected = id; return { ok: true, server: views().find(s => s.id === id) }; },
    serverPing: id => (id === 'create' ? { online: true, ping: 23, version: '1.21.1', online_count: 3, max_count: 20 } : { online: false }),
    launcherCheck: { hasUpdate: false, local: '0.5.0', remote: '0.5.0' }, launcherUpdate: { ok: true },
    getAppInfo: { version: '0.5.0', modsVersion: '1.0.0', java: 'Изолированная Java 21', ram: '6G', electron: 'Tauri 2', server: 'Create' },
    ramGet: { totalGb: 16, autoMax: 6, autoMin: 3, currentMax: 'auto', currentMin: 'auto' },
    devStatus: {}, fetchNews: [], skinGet: { skinUrl: null, isCustom: false },
    modrinthSearch: { ok: true, hits: [] }, modrinthInstalled: { ok: true, files: [] }, modrinthActive: { ok: true, files: [] },
    scanCheats: { ok: true, threats: [] }, launch: { ok: true }, discordSetPlaying: { ok: true },
  };
  w.api = new Proxy({}, {
    get: (_, k) => {
      if (typeof k !== 'string') return undefined;
      if (k.startsWith('on')) return cb => { cbs[k] = cb; };
      return async (...args) => {
        calls.push([k, ...args]);
        const impl = k in overrides ? overrides[k] : defaults[k];
        const v = typeof impl === 'function' ? impl(...args) : impl;
        return v === undefined ? { ok: true } : v;
      };
    },
  });
  w.fetch = () => Promise.reject(new Error('нет сети в тесте'));
  w.matchMedia = () => ({ matches: false, addListener() {}, addEventListener() {} });
  w.HTMLCanvasElement.prototype.getContext = () => null;
  w.confirm = () => true;
  const errors = [];
  w.addEventListener('error', e => errors.push(e.message));
  for (const f of scripts) {
    const sc = w.document.createElement('script');
    sc.textContent = readFileSync(join(rendererDir, f), 'utf8');
    w.document.body.appendChild(sc);
  }
  await sleep(150);
  return { w, d: w.document, calls, cbs, errors, callsOf: n => calls.filter(c => c[0] === n) };
}
