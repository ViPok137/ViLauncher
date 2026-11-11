/* Сезонное оформление: снег, гирлянда и поздравление. Настройки — в js/season-config.js.
 * Вне заданных дат ничего не загружается и не рисуется (даже css/season.css). Все элементы декоративные: aria-hidden, без кликов. */
(() => {
  'use strict';
  const cfg = window.VI_SEASON;
  if (!cfg || typeof cfg !== 'object') return;

  const param = new URLSearchParams(location.search).get('season');
  const force = param === 'on' ? true : param === 'off' ? false : null;

  /** 'ММ-ДД' → число ММДД (1210) или NaN, если формат неверный. */
  const md = (s) => {
    const m = /^(\d{2})-(\d{2})$/.exec(String(s || ''));
    if (!m) return NaN;
    const mo = +m[1], d = +m[2];
    return mo >= 1 && mo <= 12 && d >= 1 && d <= 31 ? mo * 100 + d : NaN;
  };

  function isActive(now) {
    if (force !== null) return force;
    if (cfg.enabled !== true) return false;
    const s = md(cfg.start), e = md(cfg.end), t = (now.getMonth() + 1) * 100 + now.getDate();
    if (Number.isNaN(s) || Number.isNaN(e)) return false;
    return s <= e ? (t >= s && t <= e) : (t >= s || t <= e);   // 12-10…01-15 переходит через Новый год
  }

  const now = new Date();
  if (!isActive(now)) return;

  const el = (tag, cls) => { const n = document.createElement(tag); n.className = cls; n.setAttribute('aria-hidden', 'true'); return n; };
  const reduced = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  function snow() {
    const box = el('div', 'season-snow');
    const count = window.innerWidth < 600 ? 16 : 32;
    for (let i = 0; i < count; i++) {
      const f = document.createElement('span');
      f.className = 'season-flake';
      const set = (k, v) => f.style.setProperty(k, v);   // через CSSOM: inline-style атрибут запрещён CSP
      set('--x', (Math.random() * 100).toFixed(1) + '%');
      set('--s', (3 + Math.random() * 4).toFixed(1) + 'px');
      set('--d', (9 + Math.random() * 9).toFixed(1) + 's');
      set('--delay', '-' + (Math.random() * 18).toFixed(1) + 's');
      set('--dx', (Math.random() * 80 - 40).toFixed(0) + 'px');
      set('--o', (0.45 + Math.random() * 0.5).toFixed(2));
      box.append(f);
    }
    document.body.append(box);
  }

  const COLORS = ['#ff5d6c', '#ffd166', '#2ee6a6', '#4cc9f0'];
  let garland = null, bulbs = 0;
  function drawGarland() {
    const n = Math.max(8, Math.round(window.innerWidth / 38));
    if (!garland || n === bulbs) return;
    bulbs = n;
    garland.replaceChildren();
    for (let i = 0; i < n; i++) {
      const b = document.createElement('span');
      b.className = 'season-bulb';
      const set = (k, v) => b.style.setProperty(k, v);
      set('--l', ((i + 0.5) / n * 100).toFixed(2) + '%');
      set('--y', (6 + Math.round(Math.abs(Math.sin(i * 0.9)) * 10)) + 'px');   // лёгкий «провис» гирлянды
      set('--c', COLORS[i % COLORS.length]);
      set('--dl', (-(i * 0.37 % 2.4)).toFixed(2) + 's');
      garland.append(b);
    }
  }

  function build() {
    // Классы на <body>: season-on — общий признак, season-caps / season-icicles — статичные украшения из css/season.css
    document.body.classList.add('season-on');
    if (cfg.caps !== false) document.body.classList.add('season-caps');
    if (cfg.icicles !== false) document.body.classList.add('season-icicles');
    if (cfg.snow !== false && !reduced) snow();
    const header = document.querySelector('header.top');
    if (cfg.garland !== false && header) {
      garland = el('div', 'season-garland');
      header.append(garland);
      drawGarland();
      let t;
      window.addEventListener('resize', () => { clearTimeout(t); t = setTimeout(drawGarland, 200); });
    }
    const text = (now.getMonth() === 0 && cfg.messageNewYear) ? cfg.messageNewYear : cfg.message;
    const eyebrow = document.querySelector('.hero .eyebrow');
    if (typeof text === 'string' && text.trim() && eyebrow) {
      const g = document.createElement('p');
      g.className = 'season-greet';
      g.textContent = text.trim().slice(0, 120);
      eyebrow.after(g);
    }
  }

  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = 'css/season.css';
  link.onload = build;          // рисуем только после загрузки стилей, чтобы не мелькали «голые» элементы
  document.head.append(link);
})();
