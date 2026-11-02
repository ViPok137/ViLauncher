'use strict';
/* ViLauncher — сайт-витрина. Чистый JavaScript, без зависимостей, без аналитики и cookies. */

/* ═══════════════════════════ НАСТРОЙКИ (меняются здесь) ═══════════════════════════ */
const VERSION = '0.5.0';   // ← Подставь актуальный тег релиза БЕЗ «v» (например '0.5.1').
                           //   Если AUTO_VERSION = true, версия и ссылка подтянутся из GitHub сами; константа — запасной вариант.
const REPO_URL = 'https://github.com/ViPok137/ViLauncher';
const DOWNLOAD_URL = REPO_URL + '/releases/latest';   // всегда рабочая ссылка на последний релиз (страница релиза)
const SETUP_NAME = 'ViLauncher_{version}_x64-setup.exe'; // шаблон имени установщика Tauri/NSIS
const USE_DIRECT_DOWNLOAD = false;  // true → кнопка ведёт сразу на .exe: …/releases/latest/download/<SETUP_NAME>
                                    //   (работает, только если VERSION совпадает с версией последнего релиза)
const NEWS_URL = 'https://raw.githubusercontent.com/ViPok137/ViLauncher/main/NEWS.json';
const API_LATEST_URL = 'https://api.github.com/repos/ViPok137/ViLauncher/releases/latest';
const AUTO_VERSION = true;          // false → не обращаться к api.github.com, использовать только константы выше
const NEWS_LIMIT = 3;               // сколько новостей показывать
const FETCH_TIMEOUT_MS = 6000;      // ожидание ответа сети
/* ══════════════════════════════════════════════════════════════════════════════════ */

const VERSION_RE = /^\d+\.\d+\.\d+(?:[-+.][0-9A-Za-z.-]+)?$/;
const ASSET_URL_PREFIX = REPO_URL + '/releases/download/';

const $ = (id) => document.getElementById(id);

function setupName(version) {
  return SETUP_NAME.replace('{version}', version);
}

function directUrl(version) {
  return REPO_URL + '/releases/latest/download/' + setupName(version);
}

/** Подставляет версию и ссылку скачивания во все нужные места страницы. */
function applyRelease(version, url) {
  if (!VERSION_RE.test(version)) return;
  const badge = $('ver-badge'); if (badge) badge.textContent = 'v' + version;
  const dlVer = $('dl-ver');    if (dlVer) dlVer.textContent = 'v' + version;
  const name = $('setup-name'); if (name) name.textContent = setupName(version);
  for (const id of ['dl-main', 'dl-box']) {
    const a = $(id); if (a && url) a.setAttribute('href', url);
  }
}

/** GET JSON с таймаутом. Без cookies и без Referer. */
async function fetchJson(url) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), FETCH_TIMEOUT_MS);
  try {
    const res = await fetch(url, { signal: ctrl.signal, credentials: 'omit', referrerPolicy: 'no-referrer', headers: { Accept: 'application/json' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    return await res.json();
  } finally {
    clearTimeout(timer);
  }
}

/* ───────────── Версия и ссылка скачивания ───────────── */
async function initRelease() {
  // 1. Запасной вариант — константы: работает без сети и без JS-API.
  applyRelease(VERSION, USE_DIRECT_DOWNLOAD ? directUrl(VERSION) : DOWNLOAD_URL);
  if (!AUTO_VERSION) return;

  // 2. Актуальный релиз у GitHub: точная версия и реальная ссылка на установщик.
  try {
    const rel = await fetchJson(API_LATEST_URL);
    const version = String(rel.tag_name || '').replace(/^v/i, '');
    if (!VERSION_RE.test(version)) return;
    const asset = (Array.isArray(rel.assets) ? rel.assets : []).find((a) =>
      a && typeof a.browser_download_url === 'string' &&
      a.browser_download_url.startsWith(ASSET_URL_PREFIX) && /-setup\.exe$/i.test(a.browser_download_url));
    applyRelease(version, asset ? asset.browser_download_url : DOWNLOAD_URL);
  } catch (e) {
    /* нет сети / лимит GitHub API — остаются константы */
  }
}

/* ───────────── Новости ───────────── */
function clip(text, max) {
  const t = String(text == null ? '' : text).replace(/\s+/g, ' ').trim();
  return t.length > max ? t.slice(0, max - 1).trimEnd() + '…' : t;
}

/** Принимает массив записей или объект { news: [...] }; всё, что не строка, отбрасывается. */
function normalizeNews(data) {
  const list = Array.isArray(data) ? data : (data && Array.isArray(data.news) ? data.news : []);
  const filtered = list.filter(
    (n) => n && typeof n === 'object' && typeof n.title === 'string' && n.title.trim()
  );

  // Новые сверху: больший id = новее; если id нет — как в конце файла (reverse)
  filtered.sort((a, b) => {
    const idA = Number(a.id);
    const idB = Number(b.id);
    if (Number.isFinite(idA) && Number.isFinite(idB)) return idB - idA;
    return 0;
  });

  const hasIds = filtered.some((n) => Number.isFinite(Number(n.id)));
  const ordered = hasIds ? filtered : filtered.slice().reverse();

  return ordered.slice(0, NEWS_LIMIT).map((n) => ({
    date: clip(typeof n.date === 'string' || typeof n.date === 'number' ? n.date : '', 40),
    title: clip(n.title, 120),
    body: clip(typeof n.body === 'string' ? n.body : '', 260),
  }));
}

/** Рендер только через textContent — HTML из NEWS.json никогда не выполняется. */
function renderNews(items) {
  const ul = $('news-list');
  if (!ul || !items.length) return false;
  ul.replaceChildren(...items.map((n) => {
    const li = document.createElement('li'); li.className = 'card news__item';
    const date = document.createElement('p'); date.className = 'news__date'; date.textContent = n.date || 'Новости';
    const h = document.createElement('h3'); h.textContent = n.title;
    li.append(date, h);
    if (n.body) { const p = document.createElement('p'); p.textContent = n.body; li.append(p); }
    return li;
  }));
  return true;
}

async function initNews() {
  const status = $('news-status');
  try {
    const items = normalizeNews(await fetchJson(NEWS_URL));
    if (renderNews(items)) {
      if (status) status.textContent = 'Последние записи из репозитория проекта.';
    } else if (status) {
      status.textContent = 'Свежих записей пока нет — ниже последние известные новости.';
    }
  } catch (e) {
    if (status) status.textContent = 'Не удалось загрузить свежие новости — показаны последние известные.';
  }
}

/* ───────────── Старт ───────────── */
function init() {
  const year = $('year'); if (year) year.textContent = String(new Date().getFullYear());
  initRelease();
  initNews();
}
init();
