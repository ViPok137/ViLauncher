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

/** Числовой id записи или null: принимаются числа и строки из одних цифр. */
function newsId(n) {
  if (typeof n.id === 'number' && Number.isFinite(n.id)) return n.id;
  if (typeof n.id === 'string' && /^\d+$/.test(n.id.trim())) return Number(n.id);
  return null;
}

/**
 * Принимает массив записей или объект { news: [...] }; всё, что не строка, отбрасывается.
 * Порядок «сначала новые»: записи с числовым id — по убыванию id; записи без id — в обратном порядке файла
 * (считаем, что новое дописывают в конец). Затем берётся NEWS_LIMIT первых.
 */
function normalizeNews(data) {
  const list = Array.isArray(data) ? data : (data && Array.isArray(data.news) ? data.news : []);
  const entries = list
    .filter((n) => n && typeof n === 'object' && typeof n.title === 'string' && n.title.trim())
    .map((n, i) => ({ n, i, id: newsId(n) }));
  const withId = entries.filter((e) => e.id !== null).sort((x, y) => y.id - x.id || x.i - y.i);
  const noId = entries.filter((e) => e.id === null).reverse();
  return [...withId, ...noId]
    .slice(0, NEWS_LIMIT)
    .map(({ n }) => ({
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

/* ───────────── Карточки серверов → модальное окно ─────────────
   Содержимое окна читается из самих карточек .srv и выводится только через textContent. */
const REDUCED_MOTION = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
const MODAL_MS = REDUCED_MOTION ? 0 : 300;   // открытие/закрытие (совпадает с transition в CSS)
const SLIDE_MS = REDUCED_MOTION ? 0 : 180;   // смена сервера внутри окна
const SWIPE_MIN_PX = 48;                     // минимальный сдвиг пальца для листания
const HINT_ON = 'Установи лаунчер, выбери этот сервер в списке и нажми «Играть». Адрес подставится сам.';
const HINT_OFF = 'Сервер пока закрыт: карточка уже есть в лаунчере, а доступ откроется позже. Следи за новостями.';

function readServerCard(card) {
  const text = (sel) => { const el = card.querySelector(sel); return el ? el.textContent.trim() : ''; };
  return {
    icon: text('.srv__ic'),
    title: text('h3'),
    version: text('.srv__ver'),
    description: text('p:not(.srv__ver)'),
    active: card.classList.contains('srv--on'),
    badges: [...card.querySelectorAll('.badge')].map((b) => ({
      text: b.textContent.trim(),
      kind: ['ok', 'soon', 'sub'].find((k) => b.classList.contains('badge--' + k)) || 'soon',
    })),
  };
}

function initServerModal() {
  const dialog = $('srv-modal');
  const cards = [...document.querySelectorAll('#servers .srv')];
  if (!dialog || typeof dialog.showModal !== 'function' || !cards.length) return; // без поддержки <dialog> карточки остаются обычными

  const view = $('mdl-view'), dots = $('mdl-dots');
  let index = 0, opener = null, animating = false, closing = false, touchX = 0, touchY = 0;

  // Точки-индикаторы (кнопки)
  const dotButtons = cards.map((card, i) => {
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'mdl__dot';
    b.setAttribute('aria-label', 'Сервер ' + (i + 1) + ' из ' + cards.length + ': ' + readServerCard(card).title);
    b.addEventListener('click', () => go(i));
    dots.append(b);
    return b;
  });

  function fill(i) {
    const d = readServerCard(cards[i]);
    $('mdl-icon').textContent = d.icon;
    $('mdl-ver').textContent = d.version;
    $('mdl-title').textContent = d.title;
    $('mdl-desc').textContent = d.description;
    $('mdl-hint').textContent = d.active ? HINT_ON : HINT_OFF;
    view.dataset.state = d.active ? 'on' : 'off';
    $('mdl-badges').replaceChildren(...d.badges.map((b) => {
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

  // Карточки становятся кнопками: клик, Enter и Пробел
  cards.forEach((card, i) => {
    const title = readServerCard(card).title;
    card.tabIndex = 0;
    card.setAttribute('role', 'button');
    card.setAttribute('aria-haspopup', 'dialog');
    card.setAttribute('aria-label', title + ': подробнее');
    card.addEventListener('click', () => open(i, card));
    card.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') { e.preventDefault(); open(i, card); }
    });
  });
}

/* ───────────── Старт ───────────── */
function init() {
  const year = $('year'); if (year) year.textContent = String(new Date().getFullYear());
  initRelease();
  initNews();
  initScreenshots();
  initServerModal();
}
init();
