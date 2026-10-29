/* ViLauncher — раздел сервера: настройки Minecraft и модов, окно предупреждения античита.
 * Окно Modrinth (шейдеры/ресурспаки) перенесено сюда из «Внешнего вида»; его логика осталась в renderer.js. */
(function () {
  'use strict';
  const api = window.api || {};
  const $ = id => document.getElementById(id);
  const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  let gameRunning = false;

  // ───────────────────────── панель сервера ─────────────────────────
  const inited = { packs: false, mc: false, mods: false };

  function openTab(name) {
    document.querySelectorAll('#sv-tools [data-svtab]').forEach(b => b.classList.toggle('on', b.dataset.svtab === name));
    ['packs', 'mc', 'mods'].forEach(t => { const el = $('svt-' + t); if (el) el.hidden = (t !== name); });
    if (inited[name]) { if (name === 'mc') loadOptions(); if (name === 'mods') loadConfigs(); return; }
    inited[name] = true;
    if (name === 'packs' && typeof window.initServerTools === 'function') window.initServerTools();
    if (name === 'mc') { buildMcTab(); loadOptions(); }
    if (name === 'mods') { buildModsTab(); loadConfigs(); }
  }

  function setupPanel() {
    const toggle = $('sv-tools-toggle'), content = $('sv-tools-content');
    if (!toggle || !content) return;
    toggle.addEventListener('click', () => {
      content.hidden = !content.hidden;
      $('sv-tools-arrow').textContent = content.hidden ? '▸' : '▾';
      if (!content.hidden) openTab(document.querySelector('#sv-tools [data-svtab].on')?.dataset.svtab || 'packs');
    });
    document.querySelectorAll('#sv-tools [data-svtab]').forEach(b => b.addEventListener('click', () => openTab(b.dataset.svtab)));
    window.addEventListener('server-changed', e => {
      const n = $('sv-tools-name'); if (n && e.detail && e.detail.server) n.textContent = e.detail.server.name;
    });
  }

  // ───────────────────────── настройки Minecraft (options.txt) ─────────────────────────
  const fnum = n => { let s = Number(n).toFixed(4).replace(/0+$/, ''); return s.endsWith('.') ? s + '0' : s; };
  const MC = [
    { key: 'fov', label: 'Угол обзора (FOV)', type: 'range', min: 30, max: 110, def: 70, fmt: v => v, fromFile: v => Math.round(70 + parseFloat(v) * 40), toFile: v => fnum((v - 70) / 40) },
    { key: 'renderDistance', label: 'Дальность прорисовки (чанки)', type: 'range', min: 2, max: 32, def: 12, fmt: v => v, fromFile: v => parseInt(v, 10), toFile: v => String(v) },
    { key: 'maxFps', label: 'Лимит FPS', type: 'range', min: 10, max: 260, step: 10, def: 120, fmt: v => (+v >= 260 ? '∞' : v), fromFile: v => parseInt(v, 10), toFile: v => String(v) },
    { key: 'guiScale', label: 'Масштаб интерфейса', type: 'select', opts: [['0', 'Авто'], ['1', '1'], ['2', '2'], ['3', '3'], ['4', '4']], def: '0' },
    { key: 'graphicsMode', label: 'Графика', type: 'select', opts: [['0', 'Быстрая'], ['1', 'Красивая'], ['2', 'Сверхкрасивая']], def: '1' },
    { key: 'particles', label: 'Частицы', type: 'select', opts: [['0', 'Все'], ['1', 'Уменьшенные'], ['2', 'Минимальные']], def: '0' },
    { key: 'gamma', label: 'Яркость', type: 'range', min: 0, max: 100, def: 50, fmt: v => v + '%', fromFile: v => Math.round(parseFloat(v) * 100), toFile: v => fnum(v / 100) },
    { key: 'mouseSensitivity', label: 'Чувствительность мыши', type: 'range', min: 0, max: 200, def: 100, fmt: v => v + '%', fromFile: v => Math.round(parseFloat(v) * 200), toFile: v => fnum(v / 200) },
    { key: 'soundCategory_master', label: 'Громкость: общая', type: 'range', min: 0, max: 100, def: 100, fmt: v => v + '%', fromFile: v => Math.round(parseFloat(v) * 100), toFile: v => fnum(v / 100) },
    { key: 'soundCategory_music', label: 'Громкость: музыка', type: 'range', min: 0, max: 100, def: 100, fmt: v => v + '%', fromFile: v => Math.round(parseFloat(v) * 100), toFile: v => fnum(v / 100) },
    { key: 'fullscreen', label: 'Полный экран', type: 'bool', def: false },
    { key: 'enableVsync', label: 'Вертикальная синхронизация', type: 'bool', def: true },
    { key: 'autoJump', label: 'Автопрыжок', type: 'bool', def: false },
    { key: 'viewBobbing', label: 'Покачивание камеры', type: 'bool', def: true },
    { key: 'lang', label: 'Язык', type: 'select', opts: [['ru_ru', 'Русский'], ['en_us', 'English (US)'], ['uk_ua', 'Українська'], ['be_by', 'Беларуская'], ['kk_kz', 'Қазақша'], ['de_de', 'Deutsch'], ['fr_fr', 'Français'], ['es_es', 'Español'], ['pl_pl', 'Polski'], ['zh_cn', '简体中文'], ['ja_jp', '日本語']], def: 'ru_ru' },
  ];
  const mcKeys = new Set(MC.map(c => c.key));
  let mcValues = {};      // key → значение из файла
  let mcDirty = {};       // key → новое значение (строка для файла)

  function setStatus(id, text, kind) {
    const el = $(id); if (!el) return;
    el.textContent = text || ''; el.className = 'gs-status' + (kind ? ' ' + kind : '');
  }
  function runningBanner(id) {
    const el = $(id); if (el) el.hidden = !gameRunning;
  }

  function buildMcTab() {
    $('svt-mc').innerHTML = `
      <div class="ram-box">
        <div class="ram-box-title">НАСТРОЙКИ MINECRAFT</div>
        <div class="gs-note">Изменения записываются в <b>options.txt</b>. Файл по умолчанию скачивается только если его нет; твои правки при обновлении модов не затираются.</div>
        <div class="gs-warn" id="gs-mc-busy" hidden>Игра запущена — закрой Minecraft, иначе он перезапишет изменения при выходе.</div>
        <div class="gs-note" id="gs-mc-nofile" hidden>options.txt ещё не создан — он появится после первого запуска игры. Можно сохранить настройки уже сейчас.</div>
        <div class="gs-grid" id="gs-mc-grid"></div>
        <div class="gs-adv-head"><div class="gs-adv-title">ВСЕ ОСТАЛЬНЫЕ ПАРАМЕТРЫ (в т.ч. управление)</div>
          <input class="gs-text" id="gs-mc-filter" placeholder="поиск…" style="width:160px"></div>
        <div class="gs-adv" id="gs-mc-adv"></div>
        <div class="gs-actions">
          <button class="gs-btn ok" id="gs-mc-save" type="button">💾 СОХРАНИТЬ</button>
          <button class="gs-btn danger" id="gs-mc-reset" type="button">↺ ВЕРНУТЬ ПО УМОЛЧАНИЮ</button>
          <span class="gs-status" id="gs-mc-status"></span>
        </div>
      </div>`;
    $('gs-mc-save').addEventListener('click', saveOptions);
    $('gs-mc-reset').addEventListener('click', resetOptions);
    $('gs-mc-filter').addEventListener('input', renderAdvanced);
  }

  function markDirty(key, fileValue, rowEl) {
    if (mcValues[key] === fileValue) delete mcDirty[key]; else mcDirty[key] = fileValue;
    if (rowEl) rowEl.classList.toggle('dirty', key in mcDirty);
  }

  function renderFriendly() {
    const grid = $('gs-mc-grid'); grid.innerHTML = '';
    for (const c of MC) {
      const row = document.createElement('div'); row.className = 'gs-row';
      const cur = mcValues[c.key];
      let ctl = '';
      if (c.type === 'range') {
        const ui = cur !== undefined && !isNaN(c.fromFile(cur)) ? c.fromFile(cur) : c.def;
        ctl = `<input type="range" min="${c.min}" max="${c.max}" step="${c.step || 1}" value="${ui}"><span class="gs-val">${esc(c.fmt(ui))}</span>`;
      } else if (c.type === 'select') {
        const v = cur !== undefined ? cur : c.def;
        const opts = c.opts.some(o => o[0] === v) ? c.opts : [[v, v], ...c.opts];
        ctl = `<select class="gs-select">${opts.map(o => `<option value="${esc(o[0])}"${o[0] === v ? ' selected' : ''}>${esc(o[1])}</option>`).join('')}</select>`;
      } else {
        const on = cur !== undefined ? cur === 'true' : c.def;
        ctl = `<input type="checkbox"${on ? ' checked' : ''}>`;
      }
      row.innerHTML = `<div class="gs-label">${esc(c.label)}</div><div class="gs-ctl">${ctl}</div>`;
      grid.appendChild(row);
      const input = row.querySelector('input,select');
      input.addEventListener('input', () => {
        let fileVal;
        if (c.type === 'range') { const v = +input.value; row.querySelector('.gs-val').textContent = c.fmt(v); fileVal = c.toFile(v); }
        else if (c.type === 'select') fileVal = input.value;
        else fileVal = input.checked ? 'true' : 'false';
        // диапазон сравниваем по пользовательскому значению, чтобы не считать «изменением» округление
        if (c.type === 'range' && cur !== undefined && c.fromFile(cur) === +input.value) delete mcDirty[c.key];
        else if (c.type === 'range' && cur === undefined && c.def === +input.value) delete mcDirty[c.key];
        else mcDirty[c.key] = fileVal;
        row.classList.toggle('dirty', c.key in mcDirty);
      });
    }
  }

  function renderAdvanced() {
    const box = $('gs-mc-adv'); if (!box) return;
    const f = ($('gs-mc-filter').value || '').toLowerCase();
    const keys = Object.keys(mcValues).filter(k => !mcKeys.has(k) && (!f || k.toLowerCase().includes(f) || mcValues[k].toLowerCase().includes(f)));
    box.innerHTML = '';
    if (!keys.length) { box.innerHTML = '<div class="gs-note" style="margin:8px 0">Нет параметров' + (f ? ' по фильтру' : ' (options.txt пуст или не создан)') + '</div>'; return; }
    for (const k of keys.slice(0, 400)) {
      const row = document.createElement('div'); row.className = 'gs-row';
      row.innerHTML = `<div class="gs-label" style="flex:0 0 40%;word-break:break-all">${esc(k)}</div><input class="gs-text" value="${esc(k in mcDirty ? mcDirty[k] : mcValues[k])}">`;
      box.appendChild(row);
      row.querySelector('input').addEventListener('input', e => markDirty(k, e.target.value, row));
      row.classList.toggle('dirty', k in mcDirty);
    }
    if (keys.length > 400) box.insertAdjacentHTML('beforeend', '<div class="gs-note">Показаны первые 400 — уточни поиск.</div>');
  }

  async function loadOptions() {
    if (!api.gameOptionsGet) { $('svt-mc').innerHTML = '<div class="gs-note">Недоступно в этой сборке.</div>'; return; }
    const r = await api.gameOptionsGet();
    if (!r || !r.ok) { setStatus('gs-mc-status', (r && r.error) || 'Не удалось прочитать options.txt', 'err'); return; }
    gameRunning = !!r.running; runningBanner('gs-mc-busy');
    $('gs-mc-nofile').hidden = r.exists;
    mcValues = {}; mcDirty = {};
    for (const o of r.options) mcValues[o.key] = o.value;
    renderFriendly(); renderAdvanced(); setStatus('gs-mc-status', '');
    $('gs-mc-save').disabled = gameRunning;
  }

  async function saveOptions() {
    const n = Object.keys(mcDirty).length;
    if (!n) { setStatus('gs-mc-status', 'Нет изменений'); return; }
    setStatus('gs-mc-status', 'Сохраняю…');
    const r = await api.gameOptionsSet({ ...mcDirty });
    if (r && r.ok) { setStatus('gs-mc-status', `✓ Сохранено параметров: ${r.saved}`, 'ok'); await loadOptions(); setStatus('gs-mc-status', `✓ Сохранено параметров: ${r.saved}`, 'ok'); }
    else setStatus('gs-mc-status', (r && r.error) || 'Ошибка сохранения', 'err');
  }

  async function resetOptions() {
    if (!confirm('Вернуть настройки Minecraft (options.txt) к значениям по умолчанию?\nТвои изменения будут потеряны.')) return;
    setStatus('gs-mc-status', 'Восстанавливаю…');
    const r = await api.defaultsReset({ path: 'options.txt' });
    if (r && r.ok) { await loadOptions(); setStatus('gs-mc-status', '✓ Настройки по умолчанию восстановлены', 'ok'); }
    else setStatus('gs-mc-status', (r && r.error) || 'Ошибка', 'err');
  }

  // ───────────────────────── настройки модов (config/**) ─────────────────────────
  let cfgFiles = [], cfgResettable = new Set(), cfgCurrent = null, cfgOriginal = '', forceNext = false;

  function buildModsTab() {
    $('svt-mods').innerHTML = `
      <div class="ram-box">
        <div class="ram-box-title">НАСТРОЙКИ МОДОВ</div>
        <div class="gs-note">Файлы из папки <b>config</b>. Версии по умолчанию скачиваются только если файла нет; свои правки можно в любой момент откатить.</div>
        <div class="gs-warn" id="gs-cfg-busy" hidden>Игра запущена — закрой Minecraft, иначе он может перезаписать изменения при выходе.</div>
        <div class="gs-cfg">
          <div>
            <input class="gs-text" id="gs-cfg-filter" placeholder="поиск файла…" style="width:100%;margin-bottom:6px;box-sizing:border-box">
            <div class="gs-cfg-list" id="gs-cfg-list"></div>
          </div>
          <div class="gs-cfg-ed">
            <div class="gs-cfg-name" id="gs-cfg-name">Выбери файл слева</div>
            <textarea class="gs-cfg-text" id="gs-cfg-text" spellcheck="false" disabled></textarea>
            <div class="gs-actions" style="margin-top:0">
              <button class="gs-btn ok" id="gs-cfg-save" type="button" disabled>💾 СОХРАНИТЬ</button>
              <button class="gs-btn" id="gs-cfg-force" type="button" hidden>СОХРАНИТЬ БЕЗ ПРОВЕРКИ</button>
              <button class="gs-btn danger" id="gs-cfg-reset" type="button" disabled>↺ СБРОСИТЬ ФАЙЛ</button>
              <span class="gs-status" id="gs-cfg-status"></span>
            </div>
          </div>
        </div>
        <div class="gs-actions"><button class="gs-btn danger" id="gs-cfg-resetall" type="button">↺ ВЕРНУТЬ ВСЕ НАСТРОЙКИ ПО УМОЛЧАНИЮ</button></div>
      </div>`;
    $('gs-cfg-filter').addEventListener('input', renderCfgList);
    $('gs-cfg-save').addEventListener('click', () => saveCfg(false));
    $('gs-cfg-force').addEventListener('click', () => saveCfg(true));
    $('gs-cfg-reset').addEventListener('click', resetCfg);
    $('gs-cfg-resetall').addEventListener('click', resetAll);
    $('gs-cfg-text').addEventListener('input', () => { $('gs-cfg-save').disabled = gameRunning || $('gs-cfg-text').value === cfgOriginal; $('gs-cfg-force').hidden = true; });
  }

  function renderCfgList() {
    const f = ($('gs-cfg-filter').value || '').toLowerCase(), box = $('gs-cfg-list');
    const list = cfgFiles.filter(x => !f || x.path.toLowerCase().includes(f));
    box.innerHTML = list.length ? '' : '<div class="gs-note" style="padding:8px">Файлы не найдены. Они появятся после первого запуска игры.</div>';
    for (const x of list) {
      const it = document.createElement('div'); it.className = 'gs-cfg-item' + (x.path === cfgCurrent ? ' on' : ''); it.textContent = x.path.replace(/^config\//, '');
      it.title = x.path; it.addEventListener('click', () => openCfg(x.path)); box.appendChild(it);
    }
  }

  async function loadConfigs() {
    if (!api.modConfigsList) { $('svt-mods').innerHTML = '<div class="gs-note">Недоступно в этой сборке.</div>'; return; }
    const r = await api.modConfigsList();
    if (!r || !r.ok) { setStatus('gs-cfg-status', (r && r.error) || 'Не удалось получить список', 'err'); return; }
    gameRunning = !!r.running; runningBanner('gs-cfg-busy');
    cfgFiles = r.files; cfgResettable = new Set(r.resettable || []);
    renderCfgList();
  }

  async function openCfg(path) {
    if (cfgCurrent && $('gs-cfg-text').value !== cfgOriginal && !confirm('Есть несохранённые изменения. Открыть другой файл без сохранения?')) return;
    const r = await api.modConfigRead(path);
    if (!r || !r.ok) { setStatus('gs-cfg-status', (r && r.error) || 'Не удалось открыть файл', 'err'); return; }
    cfgCurrent = path; cfgOriginal = r.text;
    $('gs-cfg-name').textContent = path; const t = $('gs-cfg-text'); t.value = r.text; t.disabled = false;
    $('gs-cfg-save').disabled = true; $('gs-cfg-force').hidden = true;
    $('gs-cfg-reset').disabled = gameRunning || !cfgResettable.has(path);
    $('gs-cfg-reset').title = cfgResettable.has(path) ? '' : 'Для этого файла нет версии по умолчанию в репозитории';
    setStatus('gs-cfg-status', ''); renderCfgList();
  }

  async function saveCfg(force) {
    if (!cfgCurrent) return;
    setStatus('gs-cfg-status', 'Сохраняю…');
    const text = $('gs-cfg-text').value;
    const r = await api.modConfigWrite({ path: cfgCurrent, text, force });
    if (r && r.ok) { cfgOriginal = text; $('gs-cfg-save').disabled = true; $('gs-cfg-force').hidden = true; setStatus('gs-cfg-status', '✓ Сохранено', 'ok'); }
    else {
      setStatus('gs-cfg-status', (r && r.error) || 'Ошибка сохранения', 'err');
      $('gs-cfg-force').hidden = !(r && r.validation);   // ошибка синтаксиса: можно сохранить «как есть»
    }
  }

  async function resetCfg() {
    if (!cfgCurrent || !confirm(`Вернуть файл ${cfgCurrent} к значениям по умолчанию?\nТвои изменения в нём будут потеряны.`)) return;
    setStatus('gs-cfg-status', 'Восстанавливаю…');
    const r = await api.defaultsReset({ path: cfgCurrent });
    if (r && r.ok) { const p = cfgCurrent; cfgOriginal = '\u0000'; cfgCurrent = null; await openCfg(p); setStatus('gs-cfg-status', '✓ Восстановлено по умолчанию', 'ok'); }
    else setStatus('gs-cfg-status', (r && r.error) || 'Ошибка', 'err');
  }

  async function resetAll() {
    if (!confirm('Вернуть ВСЕ настройки (options.txt и конфиги модов из репозитория) к значениям по умолчанию?\nВсе твои изменения в них будут потеряны.')) return;
    setStatus('gs-cfg-status', 'Восстанавливаю…');
    const r = await api.defaultsReset({});
    if (r && r.ok) {
      const p = cfgCurrent; cfgCurrent = null; cfgOriginal = '\u0000'; await loadConfigs(); if (p) await openCfg(p);
      setStatus('gs-cfg-status', `✓ Восстановлено файлов: ${r.restored}`, 'ok');
    } else setStatus('gs-cfg-status', (r && r.error) || 'Ошибка', 'err');
  }

  // ───────────────────────── предупреждение античита ─────────────────────────
  function showAlert(message) {
    document.querySelectorAll('.ac-overlay').forEach(n => n.remove());
    const lines = String(message || '').split('\n'), title = lines.shift() || 'Игра закрыта';
    const o = document.createElement('div'); o.className = 'ac-overlay';
    o.innerHTML = `<div class="ac-box"><div class="ac-title">⚠ ${esc(title)}</div><div class="ac-msg">${esc(lines.join('\n').trim())}</div><div class="gs-actions" style="margin:0"><button class="gs-btn danger" type="button">ПОНЯТНО</button></div></div>`;
    o.querySelector('button').addEventListener('click', () => o.remove());
    document.body.appendChild(o);
  }

  // ───────────────────────── старт ─────────────────────────
  function init() {
    setupPanel();
    if (api.onMcRunning) api.onMcRunning(d => { gameRunning = !!(d && d.running); runningBanner('gs-mc-busy'); runningBanner('gs-cfg-busy'); const b = $('gs-mc-save'); if (b) b.disabled = gameRunning; });
    if (api.onAnticheatAlert) api.onAnticheatAlert(d => showAlert(d && d.message));
    // Заголовок панели — по текущему серверу
    if (window.SERVERS && typeof window.currentServer === 'number' && window.SERVERS[window.currentServer]) {
      const n = $('sv-tools-name'); if (n) n.textContent = window.SERVERS[window.currentServer].name;
    }
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init); else init();
})();
