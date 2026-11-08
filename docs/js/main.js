'use strict';
/* ViLauncher — сайт-витрина. Чистый JavaScript, без зависимостей, без аналитики и cookies. */

/* ═══════════════════════════ НАСТРОЙКИ (меняются здесь) ═══════════════════════════ */
const VERSION = '0.0.0';   // ← Подставь актуальный тег релиза БЕЗ «v» (например '0.5.1').
                           //   Если AUTO_VERSION = true, версия и ссылка подтянутся из GitHub сами; константа — запасной вариант.
const REPO_URL = 'https://github.com/ViPok137/ViLauncher';
const SETUP_NAME = 'ViLauncher_{version}_x64-setup.exe'; // шаблон имени установщика Tauri/NSIS
const NEWS_URL = 'https://raw.githubusercontent.com/ViPok137/ViLauncher/main/NEWS.json';
const SERVERS_URL = 'https://raw.githubusercontent.com/ViPok137/ViLauncher/main/servers.json';   // каталог серверов (контракт v1)
const API_LATEST_URL = 'https://api.github.com/repos/ViPok137/ViLauncher/releases/latest';
const AUTO_VERSION = true;          // false → не обращаться к api.github.com, использовать только константы выше
const NEWS_LIMIT = 3;               // сколько новостей показывать карточками в разделе
const NEWS_MAX = 200;               // сколько записей можно листать в окне (от новых к самым старым)
const SERVERS_LIMIT = 12;           // больше карточек не рисуем (защита от мусора в JSON)
const ACCEPT_LEGACY_ACTIVE = true;  // переходный режим: если в записи нет status, но есть старое булево active → open/coming.
                                    //   Когда в servers.json у всех записей появится status — поставь false.
const FETCH_TIMEOUT_MS = 6000;      // ожидание ответа сети
/* ══════════════════════════════════════════════════════════════════════════════════ */

const VERSION_RE = /^\d+\.\d+\.\d+(?:[-+.][0-9A-Za-z.-]+)?$/;
const ASSET_URL_PREFIX = REPO_URL + '/releases/download/';
const RAW_PREFIX = 'https://raw.githubusercontent.com/ViPok137/ViLauncher/';   // единственный внешний источник иконок

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
  applyRelease(VERSION, directUrl(VERSION));
  if (!AUTO_VERSION) return;

  // 2. Актуальный релиз у GitHub: точная версия и реальная ссылка на установщик.
  try {
    const rel = await fetchJson(API_LATEST_URL);
    const version = String(rel.tag_name || '').replace(/^v/i, '');
    if (!VERSION_RE.test(version)) return;
    const exes = (Array.isArray(rel.assets) ? rel.assets : []).filter((a) =>
      a && typeof a.browser_download_url === 'string' && a.browser_download_url.startsWith(ASSET_URL_PREFIX) &&
      /setup[^/]*\.exe$/i.test(a.browser_download_url));
    // Предпочитаем Tauri-имя *-setup.exe, затем любой *setup*.exe; иначе — прямая ссылка по шаблону SETUP_NAME
    const asset = exes.find((a) => /-setup\.exe$/i.test(a.browser_download_url)) || exes[0];
    applyRelease(version, asset ? asset.browser_download_url : directUrl(version));
  } catch (e) {
    /* нет сети / лимит GitHub API — остаются константы */
  }
}

/* ───────────── Новости ───────────── */
function clip(text, max) {
  const t = String(text == null ? '' : text).replace(/\s+/g, ' ').trim();
  return t.length > max ? t.slice(0, max - 1).trimEnd() + '…' : t;
}

/** Числовой id записи или null: принимаются числа и строки из одних цифр. */
function newsId(n) {
  if (typeof n.id === 'number' && Number.isFinite(n.id)) return n.id;
  if (typeof n.id === 'string' && /^\d+$/.test(n.id.trim())) return Number(n.id);
  return null;
}

function initHeroShot() {
  const btn = document.getElementById('hero-shot-open');
  const dlg = document.getElementById('shot-dialog');
  if (!btn || !dlg || typeof dlg.showModal !== 'function') return;

  btn.addEventListener('click', () => dlg.showModal());

  dlg.addEventListener('click', (e) => {
    if (e.target === dlg) dlg.close(); // клик по фону/полю dialog
  });
}

// в init():
initHeroShot();
/**
 * Принимает массив записей или объект { news: [...] }; всё, что не строка, отбрасывается.
 * Порядок «сначала новые»: записи с числовым id — по убыванию id; записи без id — в обратном порядке файла
 * (считаем, что новое дописывают в конец). Возвращаются все записи (до NEWS_MAX): карточки берут первые NEWS_LIMIT, окно листает все.
 */
function normalizeNews(data) {
  const list = Array.isArray(data) ? data : (data && Array.isArray(data.news) ? data.news : []);
  const entries = list
    .filter((n) => n && typeof n === 'object' && typeof n.title === 'string' && n.title.trim())
    .map((n, i) => ({ n, i, id: newsId(n) }));
  const withId = entries.filter((e) => e.id !== null).sort((x, y) => y.id - x.id || x.i - y.i);
  const noId = entries.filter((e) => e.id === null).reverse();
  return [...withId, ...noId]
    .slice(0, NEWS_MAX)
    .map(({ n }) => {
      const raw = typeof n.body === 'string' ? n.body : '';
      const full = raw.replace(/\r/g, '').replace(/[ \t]+/g, ' ').replace(/ ?\n ?/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
      return {
        date: clip(typeof n.date === 'string' || typeof n.date === 'number' ? n.date : '', 40),
        title: clip(n.title, 120),
        body: clip(raw, 260),                                            // для карточки
        full: full.length > 2000 ? full.slice(0, 1999).trimEnd() + '…' : full,   // для окна
      };
    });
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

async function initNews(setNews) {
  const status = $('news-status');
  try {
    const items = normalizeNews(await fetchJson(NEWS_URL));
    if (renderNews(items.slice(0, NEWS_LIMIT))) {
      setNews(items);
      if (status) status.textContent = items.length > NEWS_LIMIT
        ? 'Последние записи из репозитория проекта. Откройте любую — и листайте до самых старых (всего ' + items.length + ').'
        : 'Последние записи из репозитория проекта.';
    } else if (status) {
      status.textContent = 'Свежих записей пока нет — ниже последние известные новости.';
    }
  } catch (e) {
    if (status) status.textContent = 'Не удалось загрузить свежие новости — показаны последние известные.';
  }
}

/* ───────────── Скриншоты лаунчера (необязательно) ─────────────
   Блок показывается, только если рядом лежат реальные файлы docs/assets/screenshot-1.png и/или screenshot-2.png. */
const SCREENSHOTS = [
  { item: 'shot-1', img: 'shot-1-img', src: 'assets/screenshot-1.png' },
  { item: 'shot-2', img: 'shot-2-img', src: 'assets/screenshot-2.png' },
];

function initScreenshots() {
  for (const shot of SCREENSHOTS) {
    const probe = new Image();
    probe.onload = () => {
      const img = $(shot.img), item = $(shot.item), section = $('shots');
      if (!img || !item || !section) return;
      img.src = shot.src;
      item.hidden = false;
      section.hidden = false;
    };
    probe.onerror = () => { /* файла нет — слот остаётся скрытым */ };
    probe.src = shot.src;
  }
}

/* ───────────── Каталог серверов (servers.json, контракт v1) ─────────────
   Источник истины о том, можно ли зайти, — поле status из JSON. Сайт ничего не пингует и не хранит.
   Всё выводится через textContent; адрес на странице не показывается (его подставляет лаунчер). */
const STATUS = {
  open:        { label: 'Доступен',        kind: 'ok'   },
  maintenance: { label: 'На обслуживании', kind: 'warn' },
  updating:    { label: 'Обновляется',     kind: 'busy' },
  coming:      { label: 'Скоро',           kind: 'soon' },
  offline:     { label: 'Выключен',        kind: 'off'  },
};
const LOADERS = { neoforge: 'NeoForge', forge: 'Forge', fabric: 'Fabric', quilt: 'Quilt' };
const ID_RE = /^[A-Za-z0-9][A-Za-z0-9_-]{0,47}$/;
let iconSeq = 0;

/** Иконка по ссылке: только assets/… этого сайта или файлы из этого репозитория на raw.githubusercontent.com. */
function safeIconUrl(u) {
  if (typeof u !== 'string') return '';
  const t = u.trim();
  if (!t || t.length > 300) return '';
  if (/^assets\/[\w./-]+\.(png|webp|jpe?g|gif|svg)$/i.test(t) && !t.includes('..')) return t;
  try {
    const x = new URL(t);
    if (x.protocol === 'https:' && x.href.startsWith(RAW_PREFIX) && /\.(png|webp|jpe?g|gif)$/i.test(x.pathname)) return x.href;
  } catch (e) { /* не ссылка */ }
  return '';
}

/** Сначала показывается символ icon; картинка подменяет его, только когда реально загрузилась. */
function paintIcon(el, glyph, url) {
  const token = String(++iconSeq);
  el.dataset.t = token;
  el.textContent = glyph;
  if (!url) return;
  const img = new Image();
  img.alt = ''; img.decoding = 'async'; img.referrerPolicy = 'no-referrer';
  img.onload = () => { if (el.dataset.t === token) el.replaceChildren(img); };
  img.src = url;
}

function versionLine(s) {
  const mc = typeof s.mcVersion === 'string' ? clip(s.mcVersion, 24) : '';
  const parts = [!mc || /^latest$/i.test(mc) ? 'Всегда последняя версия' : 'Minecraft ' + mc];
  const raw = typeof s.loader === 'string' ? s.loader.trim() : '';
  if (s.type !== 'vanilla' && raw && !/^(none|vanilla)$/i.test(raw)) parts.push(LOADERS[raw.toLowerCase()] || clip(raw, 20));
  return parts.join(' · ');
}

/** releaseAt (ISO 8601 с часовым поясом) → { text: «Откроется 20 декабря в 10:00», until: мс }.
 *  Время переводится в часовой пояс посетителя, сам пояс на странице не пишем.
 *  Только для status: coming; без пояса, с неверной датой или уже прошедшая дата — null. */
function releaseInfo(v) {
  if (typeof v !== 'string' || !/(Z|[+-]\d{2}:?\d{2})$/i.test(v.trim())) return null;
  const until = Date.parse(v.trim());
  if (!Number.isFinite(until) || until <= Date.now()) return null;
  const d = new Date(until);
  const opts = { day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' };
  if (d.getFullYear() !== new Date().getFullYear()) opts.year = 'numeric';
  try { return { text: 'Откроется ' + new Intl.DateTimeFormat('ru-RU', opts).format(d), until }; } catch (e) { return null; }
}

/** «12 д 03:45:10» (дни — только если они есть) */
function formatLeft(ms) {
  const t = Math.max(0, Math.floor(ms / 1000)), d = Math.floor(t / 86400);
  const p = (n) => String(n).padStart(2, '0');
  return (d ? d + ' д ' : '') + p(Math.floor(t % 86400 / 3600)) + ':' + p(Math.floor(t % 3600 / 60)) + ':' + p(t % 60);
}

/** Раз в секунду обновляет все таймеры на странице (карточки и окно подробностей); истёкшие убирает. */
function tickTimers() {
  const now = Date.now();
  document.querySelectorAll('[data-until]').forEach((el) => {
    const left = Number(el.dataset.until) - now;
    if (!(left > 0)) el.remove();
    else if (el.classList.contains('badge')) el.textContent = formatLeft(left);
  });
}

function timerBadge(until) {
  const b = document.createElement('span');
  b.className = 'badge badge--ok badge--timer';
  b.setAttribute('role', 'timer');
  b.dataset.until = String(until);
  b.textContent = formatLeft(until - Date.now());
  return b;
}

/** Принимает массив или { version, servers: [...] }; мусорные записи отбрасываются, порядок — по order (нет order → в конец, по порядку файла). */
function normalizeServers(data) {
  const list = Array.isArray(data) ? data : (data && Array.isArray(data.servers) ? data.servers : []);
  const seen = new Set();
  const out = [];
  list.forEach((s, i) => {
    if (!s || typeof s !== 'object' || typeof s.name !== 'string' || !s.name.trim()) return;
    const id = typeof s.id === 'string' ? s.id.trim() : '';
    if (!ID_RE.test(id) || seen.has(id)) return;
    seen.add(id);

    let status = typeof s.status === 'string' ? s.status.trim().toLowerCase() : '';
    if (!status && ACCEPT_LEGACY_ACTIVE && typeof s.active === 'boolean') status = s.active ? 'open' : 'coming';
    const known = Object.prototype.hasOwnProperty.call(STATUS, status);
    const hasAddress = typeof s.address === 'string' && s.address.trim() !== '';
    const joinable = status === 'open' && hasAddress;
    // reason выбирает подпись и подсказку в окне; open без адреса — не «доступен», а «адрес не указан»
    const reason = !known ? 'unknown' : (status === 'open' && !hasAddress ? 'noaddress' : status);
    const glyph = Array.from(typeof s.icon === 'string' ? s.icon.trim() : '').slice(0, 8).join('') || Array.from(s.name.trim())[0].toUpperCase();

    out.push({
      i, id, reason, joinable,
      order: typeof s.order === 'number' && Number.isFinite(s.order) ? s.order : Infinity,
      name: clip(s.name, 60),
      version: versionLine(s),
      description: clip(typeof s.description === 'string' ? s.description : '', 200),
      release: status === 'coming' ? releaseInfo(s.releaseAt) : null,                     // необязательное поле releaseAt
      message: clip(typeof s.statusMessage === 'string' ? s.statusMessage : '', 160),   // необязательное поле на будущее
      glyph, iconUrl: safeIconUrl(s.iconUrl),
      subscription: s.subscription === true,
    });
  });
  return out.sort((a, b) => (a.order === b.order ? a.i - b.i : a.order - b.order)).slice(0, SERVERS_LIMIT);
}

function badgeFor(reason) {
  if (reason === 'noaddress') return { text: 'Адрес не указан', kind: 'soon' };
  if (reason === 'unknown') return { text: 'Статус неизвестен', kind: 'off' };
  return { text: STATUS[reason].label, kind: STATUS[reason].kind };
}

function buildServerCard(s) {
  const li = document.createElement('li');
  li.className = 'srv ' + (s.joinable ? 'srv--on' : 'srv--off');
  li.dataset.id = s.id; li.dataset.reason = s.reason; li.dataset.glyph = s.glyph;
  if (s.iconUrl) li.dataset.iconUrl = s.iconUrl;

  const top = document.createElement('div'); top.className = 'srv__top';
  const ic = document.createElement('span'); ic.className = 'srv__ic'; ic.setAttribute('aria-hidden', 'true');
  paintIcon(ic, s.glyph, s.iconUrl);
  const h = document.createElement('h3'); h.textContent = s.name;
  top.append(ic, h);

  const ver = document.createElement('p'); ver.className = 'srv__ver'; ver.textContent = s.version;
  li.append(top, ver);
  if (s.description) { const p = document.createElement('p'); p.textContent = s.description; li.append(p); }
  if (s.release) { const r = document.createElement('p'); r.className = 'srv__msg'; r.textContent = s.release.text; r.dataset.until = String(s.release.until); li.append(r); }
  if (s.message) { const m = document.createElement('p'); m.className = 'srv__msg'; m.textContent = s.message; li.append(m); }

  const badges = document.createElement('div'); badges.className = 'srv__badges';
  const make = (b) => { const span = document.createElement('span'); span.className = 'badge badge--' + b.kind; span.textContent = b.text; return span; };
  badges.append(make(badgeFor(s.reason)));
  if (s.subscription) badges.append(make({ text: 'по подписке', kind: 'sub' }));
  if (s.release) badges.append(timerBadge(s.release.until));
  li.append(badges);
  return li;
}

async function initServers(refreshModal) {
  const ul = $('servers-list'), status = $('servers-status');
  const note = (t) => { if (status) status.textContent = t; };
  try {
    const items = normalizeServers(await fetchJson(SERVERS_URL));
    if (!ul || !items.length) { note('В каталоге пока нет серверов — показаны последние известные данные, статусы могут быть неактуальны.'); return; }
    ul.replaceChildren(...items.map(buildServerCard));
    refreshModal();
    note('Статусы взяты из каталога проекта.');
  } catch (e) {
    note('Не удалось загрузить каталог — показаны последние известные данные, статусы могут быть неактуальны.');
  }
}

/* ───────────── Карточки серверов → модальное окно ─────────────
   Содержимое окна читается из самих карточек .srv и выводится только через textContent. */
const REDUCED_MOTION = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
const MODAL_MS = REDUCED_MOTION ? 0 : 300;   // открытие/закрытие (совпадает с transition в CSS)
const SLIDE_MS = REDUCED_MOTION ? 0 : 180;   // смена сервера внутри окна
const SWIPE_MIN_PX = 48;                     // минимальный сдвиг пальца для листания
const HINT_ON = 'Установи лаунчер, выбери этот сервер в списке и нажми «Играть». Адрес подставится сам.';
const HINT_OFF = 'Сервер пока закрыт: карточка уже есть в лаунчере, а доступ откроется позже. Следи за новостями.';
const HINTS = {
  open: HINT_ON,
  coming: HINT_OFF,
  maintenance: 'Сервер на обслуживании: зайти сейчас нельзя. Загляни позже — статус на этой странице обновится.',
  updating: 'Сервер обновляется: подключиться пока нельзя. Попробуй чуть позже.',
  offline: 'Сервер выключен: подключиться нельзя.',
  noaddress: 'Адрес сервера пока не опубликован, поэтому подключиться нельзя. Следи за новостями.',
  unknown: 'Статус сервера неизвестен, поэтому подключиться не получится. Загляни позже.',
};

function readServerCard(card) {
  const text = (sel) => { const el = card.querySelector(sel); return el ? el.textContent.trim() : ''; };
  return {
    icon: card.dataset.glyph || text('.srv__ic'),
    title: text('h3'),
    version: text('.srv__ver'),
    description: text('p:not(.srv__ver):not(.srv__msg)'),
    message: [...card.querySelectorAll('.srv__msg')].map((x) => x.textContent.trim().replace(/[.!]+$/, '')).filter(Boolean).map((t) => t + '.').join(' '),
    iconUrl: card.dataset.iconUrl || '',
    active: card.classList.contains('srv--on'),
    reason: card.dataset.reason || (card.classList.contains('srv--on') ? 'open' : 'coming'),
    badges: [...card.querySelectorAll('.badge')].map((b) => ({
      text: b.textContent.trim(),
      kind: ['ok', 'soon', 'warn', 'busy', 'off', 'sub'].find((k) => b.classList.contains('badge--' + k)) || 'soon',
      until: b.dataset.until || '',
    })),
  };
}

function initServerModal() {
  const dialog = $('srv-modal'), list = $('servers-list');
  if (!dialog || !list || typeof dialog.showModal !== 'function') return () => {}; // без поддержки <dialog> карточки остаются обычными

  const view = $('mdl-view'), dots = $('mdl-dots');
  let cards = [], dotButtons = [];
  let index = 0, opener = null, animating = false, closing = false, touchX = 0, touchY = 0;

  /** Читает текущие карточки: вызывается при старте и после того, как каталог заменил карточки. */
  function collect() {
    cards = [...list.querySelectorAll('.srv')];
    dots.replaceChildren();
    dotButtons = cards.map((card, i) => {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'mdl__dot';
      b.setAttribute('aria-label', 'Сервер ' + (i + 1) + ' из ' + cards.length + ': ' + readServerCard(card).title);
      b.addEventListener('click', () => go(i));
      dots.append(b);
      return b;
    });
    // Карточки становятся кнопками: клик и клавиши обрабатывает делегирование на списке (ниже)
    cards.forEach((card) => {
      card.tabIndex = 0;
      card.setAttribute('role', 'button');
      card.setAttribute('aria-haspopup', 'dialog');
      card.setAttribute('aria-label', readServerCard(card).title + ': подробнее');
    });
    if (dialog.open && cards.length) {     // каталог пришёл, пока окно открыто: остаёмся на месте, но уже на новых данных
      opener = null;
      index = Math.min(index, cards.length - 1);
      fill(index);
    }
  }

  function fill(i) {
    const d = readServerCard(cards[i]);
    paintIcon($('mdl-icon'), d.icon, d.iconUrl);
    $('mdl-ver').textContent = d.version;
    $('mdl-title').textContent = d.title;
    $('mdl-desc').textContent = d.description;
    $('mdl-hint').textContent = [d.message, HINTS[d.reason] || (d.active ? HINT_ON : HINT_OFF)].filter(Boolean).join(' ');
    view.dataset.state = d.active ? 'on' : 'off';
    $('mdl-badges').replaceChildren(...d.badges.map((b) => {
      if (b.until) return timerBadge(Number(b.until));   // таймер в окне тоже тикает
      const span = document.createElement('span');
      span.className = 'badge badge--' + b.kind;
      span.textContent = b.text;
      return span;
    }));
    $('mdl-count').textContent = (i + 1) + ' / ' + cards.length;
    dotButtons.forEach((b, k) => b.setAttribute('aria-current', k === i ? 'true' : 'false'));
  }

  /** Переход на сервер i с анимацией (dir: 'next' | 'prev'); по кругу. */
  function go(target, dir) {
    const next = (target + cards.length) % cards.length;
    if (animating || next === index) return;
    const direction = dir || (next > index ? 'next' : 'prev');
    animating = true;
    view.dataset.anim = 'out-' + direction;
    setTimeout(() => {
      index = next;
      fill(index);
      view.dataset.anim = 'in-' + direction;
      void view.offsetWidth;            // фиксируем стартовое положение, затем плавно возвращаем
      delete view.dataset.anim;
      animating = false;
    }, SLIDE_MS);
  }

  function open(i, from) {
    if (closing || dialog.open) return;
    index = i; opener = from || null;
    delete view.dataset.anim;
    fill(index);
    document.documentElement.classList.add('modal-open');
    dialog.showModal();
    requestAnimationFrame(() => dialog.classList.add('is-open'));
  }

  function close() {
    if (!dialog.open || closing) return;
    closing = true;
    dialog.classList.remove('is-open');
    setTimeout(() => { dialog.close(); }, MODAL_MS);
  }

  dialog.addEventListener('close', () => {
    closing = false; animating = false;
    dialog.classList.remove('is-open');
    document.documentElement.classList.remove('modal-open');
    if (opener && typeof opener.focus === 'function') opener.focus();
  });
  // Esc: вместо мгновенного закрытия — то же плавное закрытие
  dialog.addEventListener('cancel', (e) => { e.preventDefault(); close(); });
  // Клик по затемнённому фону (цель клика — сам <dialog>)
  dialog.addEventListener('click', (e) => { if (e.target === dialog) close(); });
  $('mdl-close').addEventListener('click', close);
  $('mdl-prev').addEventListener('click', () => go(index - 1, 'prev'));
  $('mdl-next').addEventListener('click', () => go(index + 1, 'next'));
  // «К скачиванию»: закрыть окно и плавно прокрутить к блоку скачивания
  $('mdl-cta').addEventListener('click', (e) => {
    e.preventDefault();
    close();
    setTimeout(() => {
      const target = $('download');
      if (target) target.scrollIntoView({ behavior: REDUCED_MOTION ? 'auto' : 'smooth' });
    }, MODAL_MS + 20);
  });
  dialog.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowLeft') { e.preventDefault(); go(index - 1, 'prev'); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); go(index + 1, 'next'); }
  });
  // Свайп пальцем: влево — следующий сервер, вправо — предыдущий
  dialog.addEventListener('touchstart', (e) => {
    const t = e.changedTouches[0]; touchX = t.clientX; touchY = t.clientY;
  }, { passive: true });
  dialog.addEventListener('touchend', (e) => {
    const t = e.changedTouches[0], dx = t.clientX - touchX, dy = t.clientY - touchY;
    if (Math.abs(dx) >= SWIPE_MIN_PX && Math.abs(dx) > Math.abs(dy) * 1.2) go(index + (dx < 0 ? 1 : -1), dx < 0 ? 'next' : 'prev');
  }, { passive: true });
  window.addEventListener('pagehide', () => document.documentElement.classList.remove('modal-open'));

  // Клик, Enter и Пробел на карточке — делегированием, чтобы работало и с карточками, которые заменил каталог
  list.addEventListener('click', (e) => {
    const card = e.target.closest('.srv'), i = cards.indexOf(card);
    if (i >= 0) open(i, card);
  });
  list.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' && e.key !== ' ' && e.key !== 'Spacebar') return;
    const card = e.target.closest('.srv'), i = cards.indexOf(card);
    if (i >= 0 && e.target === card) { e.preventDefault(); open(i, card); }
  });

  collect();
  return collect;
}

/* ───────────── Новости → окно с листанием ─────────────
   Карточки в разделе — только NEWS_LIMIT последних; окно листает все записи до самой старой (без зацикливания).
   Данные берутся из массива (а не из DOM, где карточек всего три); без сети — из запасных карточек HTML. */
function initNewsModal() {
  const dialog = $('news-modal'), list = $('news-list');
  if (!dialog || !list || typeof dialog.showModal !== 'function') return () => {};

  const view = $('nm-view'), prev = $('nm-prev'), next = $('nm-next');
  let items = [], cards = [];
  let index = 0, opener = null, animating = false, closing = false, touchX = 0, touchY = 0;

  /** Запасные данные — из карточек, которые уже есть в HTML. */
  function fromCards() {
    return [...list.querySelectorAll('.news__item')].map((li) => {
      const t = (sel) => { const el = li.querySelector(sel); return el ? el.textContent.trim() : ''; };
      return { date: t('.news__date'), title: t('h3'), full: t('p:not(.news__date)') };
    });
  }

  /** Вызывается при старте и когда NEWS.json заменил карточки: setItems(все записи) или setItems() — взять из карточек. */
  function setItems(all) {
    items = all && all.length ? all : fromCards();
    cards = [...list.querySelectorAll('.news__item')];
    cards.forEach((card) => {
      card.tabIndex = 0;
      card.setAttribute('role', 'button');
      card.setAttribute('aria-haspopup', 'dialog');
      const h = card.querySelector('h3');
      card.setAttribute('aria-label', (h ? h.textContent.trim() : 'Новость') + ': читать полностью');
    });
    if (dialog.open && items.length) { opener = null; index = Math.min(index, items.length - 1); fill(); }
  }

  function fill() {
    const n = items[index], active = document.activeElement;
    $('nm-date').textContent = n.date || 'Новости';
    $('nm-title').textContent = n.title;
    $('nm-body').textContent = n.full;
    $('nm-count').textContent = (index + 1) + ' / ' + items.length;
    prev.disabled = index === 0;
    next.disabled = index === items.length - 1;
    // фокус не должен пропасть, если нажатая стрелка стала неактивной (дошли до края)
    if (active === prev && prev.disabled) next.focus();
    else if (active === next && next.disabled) prev.focus();
    view.scrollTop = 0;
  }

  /** dir: 'next' — к более старой записи, 'prev' — к более новой. */
  function go(target, dir) {
    if (animating || target < 0 || target >= items.length || target === index) return;
    animating = true;
    view.dataset.anim = 'out-' + dir;
    setTimeout(() => {
      index = target;
      fill();
      view.dataset.anim = 'in-' + dir;
      void view.offsetWidth;
      delete view.dataset.anim;
      animating = false;
    }, SLIDE_MS);
  }

  function open(i, from) {
    if (closing || dialog.open || !items[i]) return;
    index = i; opener = from || null;
    delete view.dataset.anim;
    fill();
    document.documentElement.classList.add('modal-open');
    dialog.showModal();
    requestAnimationFrame(() => dialog.classList.add('is-open'));
  }

  function close() {
    if (!dialog.open || closing) return;
    closing = true;
    dialog.classList.remove('is-open');
    setTimeout(() => { dialog.close(); }, MODAL_MS);
  }

  dialog.addEventListener('close', () => {
    closing = false; animating = false;
    dialog.classList.remove('is-open');
    document.documentElement.classList.remove('modal-open');
    if (opener && typeof opener.focus === 'function') opener.focus();
  });
  dialog.addEventListener('cancel', (e) => { e.preventDefault(); close(); });
  dialog.addEventListener('click', (e) => { if (e.target === dialog) close(); });
  $('nm-close').addEventListener('click', close);
  prev.addEventListener('click', () => go(index - 1, 'prev'));
  next.addEventListener('click', () => go(index + 1, 'next'));
  dialog.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowLeft') { e.preventDefault(); go(index - 1, 'prev'); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); go(index + 1, 'next'); }
  });
  // Свайп: влево — более старая запись, вправо — более новая
  dialog.addEventListener('touchstart', (e) => {
    const t = e.changedTouches[0]; touchX = t.clientX; touchY = t.clientY;
  }, { passive: true });
  dialog.addEventListener('touchend', (e) => {
    const t = e.changedTouches[0], dx = t.clientX - touchX, dy = t.clientY - touchY;
    if (Math.abs(dx) >= SWIPE_MIN_PX && Math.abs(dx) > Math.abs(dy) * 1.2) go(index + (dx < 0 ? 1 : -1), dx < 0 ? 'next' : 'prev');
  }, { passive: true });
  window.addEventListener('pagehide', () => document.documentElement.classList.remove('modal-open'));

  list.addEventListener('click', (e) => {
    const card = e.target.closest('.news__item'), i = cards.indexOf(card);
    if (i >= 0) open(i, card);
  });
  list.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' && e.key !== ' ' && e.key !== 'Spacebar') return;
    const card = e.target.closest('.news__item'), i = cards.indexOf(card);
    if (i >= 0 && e.target === card) { e.preventDefault(); open(i, card); }
  });

  setItems();
  return setItems;
}

/* ───────────── Старт ───────────── */
function init() {
  const year = $('year'); if (year) year.textContent = String(new Date().getFullYear());
  initRelease();
  initNews(initNewsModal());
  initScreenshots();
  setInterval(tickTimers, 1000);
  initServers(initServerModal());   // сначала модалка на запасных карточках из HTML, затем каталог подменяет их и обновляет модалку
}
init();
