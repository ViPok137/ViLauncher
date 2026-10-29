'use strict';
// ─── LOGIN ───────────────────────────────────────────────────────────────────
const loginScreen = document.getElementById('login-screen');
const appEl       = document.getElementById('app');

async function initLogin() {
  const user = await api.authGetUser();
  if (user) {
    showApp(user);
    return;
  }
  // Подставляем сохранённые данные
  const saved = await api.authGetSaved();
  if (saved.username) document.getElementById('li-user').value = saved.username;
  if (saved.password) document.getElementById('li-pass').value = saved.password;
  // Если оба поля заполнены — авто-логин
  if (saved.username && saved.password) {
    await doLogin();
  }
}

async function doLogin() {
  const username = document.getElementById('li-user').value.trim();
  const password = document.getElementById('li-pass').value;
  const remember = document.getElementById('li-rem').checked;
  const errEl    = document.getElementById('login-err');
  const btn      = document.getElementById('btn-login');

  errEl.classList.remove('show');
  if (!username || !password) {
    errEl.textContent = '// Введи логин и пароль';
    errEl.classList.add('show');
    return;
  }

  btn.disabled = true;
  btn.textContent = 'ВХОД...';

  const r = await api.authLogin({ username, password, remember });

  btn.disabled = false;
  btn.textContent = 'ВОЙТИ';

  if (r.ok) {
    showApp(r.username);
  } else {
    errEl.textContent = '// ' + (r.error || 'Ошибка входа');
    errEl.classList.add('show');
  }
}

function showApp(username) {
  loginScreen.classList.add('hidden');
  appEl.style.display = '';
  updatePlayerCard(username);
}

async function updatePlayerCard(username) {
  const uname      = document.getElementById('sb-uname');
  const skinDef    = document.getElementById('sb-skin-default');
  const skinCanvas = document.getElementById('sb-skin-canvas');

  if (uname) uname.textContent = username.toUpperCase();
  if (skinDef) {
    skinDef.textContent = (username[0] || '?').toUpperCase();
    skinDef.style.display = 'flex';
  }
  if (skinCanvas) skinCanvas.style.display = 'none';

  try {
    const { skinUrl } = await api.skinGet(username);
    // skinUrl теперь data:image/png;base64,... — никаких CORS проблем
    if (!skinUrl || !skinCanvas) return;

    const img = new Image();
    img.onload = () => {
      try {
        // ТЗ 2.3: нативный Canvas2D drawImage вместо попиксельного цикла.
        // Критично для чёткости: canvas.width/height (внутреннее разрешение)
        // ДОЛЖНО точно совпадать с CSS-размером элемента — иначе браузер
        // масштабирует через CSS и появляется блюр независимо от imageSmoothingEnabled.
        const SIZE = 64; // совпадает с CSS .sb-skin canvas { width:64px; height:64px }

        skinCanvas.width  = SIZE;
        skinCanvas.height = SIZE;
        const ctx = skinCanvas.getContext('2d');
        ctx.imageSmoothingEnabled = false; // отключаем сглаживание — сохраняем pixel-art
        ctx.clearRect(0, 0, SIZE, SIZE);

        // Лицо (первый слой): координаты 8,8 размер 8x8 в текстуре скина → сразу в целевой размер
        ctx.drawImage(img, 8, 8, 8, 8, 0, 0, SIZE, SIZE);
        // Оверлей/шлем (второй слой): координаты 40,8 размер 8x8 — поверх лица
        ctx.drawImage(img, 40, 8, 8, 8, 0, 0, SIZE, SIZE);

        skinCanvas.style.display = 'block';
        if (skinDef) skinDef.style.display = 'none';
      } catch(err) { console.warn('skin render:', err); }
    };
    img.onerror = () => {}; // нет скина — остаётся буква
    img.src = skinUrl;
  } catch {}
}

document.getElementById('btn-login')?.addEventListener('click', doLogin);
document.getElementById('li-pass')?.addEventListener('keydown', e => { if (e.key === 'Enter') doLogin(); });
document.getElementById('li-user')?.addEventListener('keydown', e => { if (e.key === 'Enter') document.getElementById('li-pass')?.focus(); });

document.getElementById('btn-logout')?.addEventListener('click', async () => {
  await api.authLogout();
  loginScreen.classList.remove('hidden');
  appEl.style.display = 'none';
  document.getElementById('li-pass').value = '';
  document.getElementById('login-err')?.classList.remove('show');
});

const $ = id => document.getElementById(id);

// ─── WINDOW ──────────────────────────────────────────────────────────────────
$('wmin').onclick = () => api.minimize();
$('wmax').onclick = () => api.maximize();
$('wcls').onclick = () => api.close();

// ─── NAV ─────────────────────────────────────────────────────────────────────
document.querySelectorAll('.ni').forEach(el => {
  el.onclick = () => {
    document.querySelectorAll('.ni').forEach(n => n.classList.remove('on'));
    document.querySelectorAll('.pg').forEach(p => p.classList.remove('on'));
    el.classList.add('on');
    $('pg-' + el.dataset.p).classList.add('on');
    if (el.dataset.p === 'news')       loadNews();
    if (el.dataset.p === 'settings')   fillSettings();
    if (el.dataset.p === 'appearance') initAppearanceTab();
  };
});

// ─── NICKNAME ────────────────────────────────────────────────────────────────
const NICK_RE = /^[a-zA-Z0-9_]{3,16}$/;

function nickError(msg) {
  $('nick-err').textContent = msg;
  $('nick-err').style.display = msg ? 'block' : 'none';
}

function isNickValid(nick) {
  return !!nick && NICK_RE.test(nick);
}

// Никнейм — только отображение, всегда равен логину. Редактирование убрано.
const nickInput = $('nick');
let currentNick = '';

// ─── STATE ───────────────────────────────────────────────────────────────────
let mcInstalled  = false;
let modsReady    = false;
let isInstalling = false;
let serverOnline = null; // null = ещё не проверено, true/false = известный статус (ТЗ 3.4)

function curServer() { return SERVERS[currentServer] || null; }
function curActive() { const s = curServer(); return !!(s && s.active); }

function updatePlayBtn() {
  // Неактивный сервер: карточка видна, но играть нельзя.
  const srv = curServer();
  if (srv && !srv.active) {
    const b = $('btn-play');
    b.disabled = true;
    if (b.textContent !== '⏳ ЗАПУСК...') b.textContent = '🔒 СКОРО';
    return;
  }
  // ТЗ 3.4: кнопка ЗАПУСТИТЬ заблокирована пока сервер точно не подтверждён онлайн.
  // serverOnline === null (ещё проверяется) тоже блокирует — не даём играть в офлайн-неопределённости.
  const btn = $('btn-play');
  const blocked = !isNickValid(currentNick) || !mcInstalled || !modsReady
                || isInstalling || serverOnline !== true;
  btn.disabled = blocked;

  // Подсказка причины блокировки прямо в тексте кнопки (только если не идёт запуск/установка)
  if (!isInstalling && btn.textContent !== '⏳ ЗАПУСК...') {
    if (serverOnline === false && mcInstalled && modsReady) {
      btn.textContent = '⛔ СЕРВЕР НЕДОСТУПЕН';
    } else if (serverOnline === null && mcInstalled && modsReady) {
      btn.textContent = '⏳ ПРОВЕРКА СЕРВЕРА...';
    } else if (mcInstalled && modsReady && serverOnline === true) {
      btn.textContent = '▶ ЗАПУСТИТЬ';
    }
  }
}

// Статус выбранного сервера: «Нужна установка» / «Синхронизация…» / «Готово» / «Недоступно» / ошибка.
function setSvState(kind, text) {
  const el = $('sv-state');
  if (!el) return;
  el.className = 'sv-state ' + kind;
  el.textContent = text;
}

// ─── INIT ────────────────────────────────────────────────────────────────────
(async () => {
  // Сначала инициализируем логин
  await initLogin();

  // Никнейм всегда = имени профиля (логину) — без возможности редактирования
  const currentUser = await api.authGetUser();
  if (currentUser) {
    currentNick = currentUser;
    if (nickInput) nickInput.textContent = currentUser;
    api.setNick(currentUser);
  }

  checkLauncherUpdate();
  fillSettings();
  await loadServers();
  api.onServersChanged(() => loadServers());
  setInterval(pingAll, 30000);
})();

// Активное состояние выбранного сервера: недоступен / нужна установка / синхронизация.
async function applyServerState() {
  const srv = curServer();
  const inactive = $('sv-inactive');
  if (!srv) return;
  if (!srv.active) {
    mcInstalled = false; modsReady = false;
    $('install-box').style.display = 'none';
    $('mods-box').style.display    = 'none';
    if (inactive) {
      inactive.style.display = 'block';
      inactive.textContent = srv.subscription
        ? 'Закрытый сервер: доступ только по подписке. Скоро в лаунчере.'
        : 'Этот сервер пока недоступен — он появится в лаунчере позже.';
    }
    setSvState('off', 'Недоступно');
    updatePlayBtn();
    return;
  }
  if (inactive) inactive.style.display = 'none';
  const { installed } = await api.installCheck();
  mcInstalled = installed;
  if (!installed) {
    showInstallBlock();
    setSvState('need', 'Нужна установка');
  } else {
    hideInstallBlock();
    if (srv.type === 'vanilla') { modsReady = true; setSvState('ready', 'Готово'); }
    else runModsSync();
  }
  updatePlayBtn();
}

// ─── INSTALL BLOCK ───────────────────────────────────────────────────────────
function showInstallBlock() {
  $('install-box').style.display = 'block';
  $('mods-box').style.display    = 'none';
  $('btn-play').disabled = true;
}

function hideInstallBlock() {
  $('install-box').style.display = 'none';
  $('mods-box').style.display    = 'block';
}

$('btn-install').onclick = async () => {
  if (isInstalling) return;
  isInstalling = true;
  setSvState('busy', 'Установка…');
  $('btn-install').disabled = true;
  $('btn-install').textContent = '⏳ УСТАНОВКА...';

  api.onInstallProg(({ phase, done, total, name, pct }) => {
    $('install-phase').textContent = phase;
    $('install-file').textContent  = name;
    $('install-bar').style.width   = pct + '%';
    $('install-pct').textContent   = pct + '%';
  });

  const r = await api.installStart();
  isInstalling = false;

  if (r.ok) {
    mcInstalled = true;
    hideInstallBlock();
    if (curServer() && curServer().type === 'vanilla') { modsReady = true; setSvState('ready', 'Готово'); }
    else runModsSync();
  } else {
    setSvState('err', 'Ошибка установки');
    $('install-phase').textContent = '❌ Ошибка: ' + (r.error || 'неизвестная ошибка');
    $('btn-install').disabled = false;
    $('btn-install').textContent = '↺ ПОВТОРИТЬ';
  }
  updatePlayBtn();
};

// ─── MODS SYNC ───────────────────────────────────────────────────────────────
let modsSyncRunning = false;

// Регистрируем обработчики один раз глобально (не внутри функции!)
let _modsProgRegistered = false;

function fmt(bytes) {
  if (!bytes) return '';
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} КБ`;
  return `${(bytes / 1024 / 1024).toFixed(1)} МБ`;
}

async function runModsSync() {
  if (modsSyncRunning) return;
  modsSyncRunning = true;
  modsReady = false;
  setSvState('busy', 'Синхронизация…');
  updatePlayBtn();

  const totalBar  = $('mbar');
  const filebar   = $('mods-filebar');
  const st        = $('mst');
  const fname     = $('mods-fname');
  const fsize     = $('mods-fsize');
  const fcounter  = $('mods-counter');

  // Сброс UI
  totalBar.style.width  = '0%';
  filebar.style.width   = '0%';
  fname.textContent     = '';
  fsize.textContent     = '';
  fcounter.textContent  = '';
  st.className          = 'mods-st chk';
  st.textContent        = '⏳ Подключаюсь к GitHub...';

  // Регистрируем обработчики только один раз
  if (!_modsProgRegistered) {
    _modsProgRegistered = true;

    api.onModsStatus(({ text }) => {
      st.textContent = '⏳ ' + text;
    });

    api.onModsProg(({ fileIndex, total, name: n, filePct, fileDone, fileSize, done: fileDone_ }) => {
      // Общий прогресс по файлам
      const totalPct = total > 0 ? Math.round((fileIndex / total) * 100) : 0;
      totalBar.style.width = totalPct + '%';
      fcounter.textContent = `${fileIndex}/${total}`;

      // Прогресс текущего файла
      filebar.style.width = (filePct || 0) + '%';
      fname.textContent   = n || '';

      // Размер
      if (fileSize > 0) {
        fsize.textContent = `${fmt(fileDone)} / ${fmt(fileSize)}`;
      } else if (fileDone > 0) {
        fsize.textContent = fmt(fileDone);
      } else {
        fsize.textContent = '';
      }

      // Статус
      if (fileDone_) {
        st.className   = 'mods-st chk';
        st.textContent = `⬇ Скачано ${fileIndex} из ${total}`;
      } else {
        st.className   = 'mods-st chk';
        st.textContent = `⬇ Скачиваю ${fileIndex + 1} из ${total}...`;
      }
    });
  }

  let r;
  try {
    r = await api.modsSync();
  } catch (err) {
    r = { ok: false, error: err.message || 'Неизвестная ошибка' };
  }

  totalBar.style.width = '100%';
  filebar.style.width  = '100%';
  fname.textContent    = '';
  fsize.textContent    = '';
  modsReady       = true;
  modsSyncRunning = false;

  if (r.ok) {
    setSvState('ready', 'Готово');
    st.className   = 'mods-st ok';
    fcounter.textContent = '';
    st.textContent = r.updated
      ? `✓ Моды обновлены до версии ${r.version}`
      : `✓ ${r.msg}`;
  } else {
    setSvState('err', 'Ошибка синхронизации');
    st.className   = 'mods-st err';
    const err  = r.error || 'Ошибка синхронизации';
    const hint = /HTTP 4/.test(err) ? ' — manifest.json не найден на GitHub' :
                 /Timeout/.test(err) ? ' — GitHub недоступен, проверь интернет' : '';
    st.textContent = '✗ ' + err + hint;
  }

  updatePlayBtn();
}

// ─── LAUNCHER UPDATE ─────────────────────────────────────────────────────────
// Подписанные обновления (tauri-plugin-updater): проверка при старте и вручную, прогресс скачивания,
// установка и автоматический перезапуск лаунчера.
async function checkLauncherUpdate(manual) {
  let r;
  try { r = await api.launcherCheck(); } catch (e) { r = { hasUpdate: false, error: String(e) }; }
  if (r.hasUpdate) {
    $('bn-txt').textContent = `⬆ Новая версия лаунчера: ${r.remote} (у тебя ${r.local})`;
    $('banner').classList.add('on');
  } else if (manual) {
    showActionStatus(r.error ? '✗ Не удалось проверить обновления: ' + r.error : `✓ Установлена последняя версия (${r.local || ''})`, !!r.error);
  }
  return r;
}
api.onLauncherDl(({ pct }) => {
  $('bn-txt').textContent = `⬇ Скачиваю обновление: ${pct || 0}%`;
});
$('bn-upd').onclick = async () => {
  $('bn-upd').disabled = true;
  $('bn-txt').textContent = '⬇ Скачиваю обновление: 0%';
  const r = await api.launcherUpdate();
  // при успехе лаунчер сам перезапустится; сюда попадаем только при ошибке
  if (r && r.ok === false) {
    $('bn-upd').disabled = false;
    $('bn-txt').textContent = '✗ ' + (r.error || 'Не удалось обновить лаунчер');
  }
};
$('bn-skip').onclick = () => $('banner').classList.remove('on');

// ─── LAUNCH ──────────────────────────────────────────────────────────────────
// Слушаем ошибки Java (если процесс упал сразу)
api.onGameError(({ error }) => {
  $('mst').className = 'mods-st err';
  $('mst').textContent = '✗ ' + error.split('\n')[0]; // первая строка в статус
  console.error('Game error:', error);
});

$('btn-play').onclick = async () => {
  const nick = currentNick;
  if (!isNickValid(nick)) return;

  $('btn-play').disabled = true;
  $('btn-play').textContent = '⏳ ЗАПУСК...';

  // Адрес, instance и модпак определяет лаунчер по выбранному серверу (servers.json); играть можно только на активном.
  const srv = curServer();
  if (!srv || !srv.active) { updatePlayBtn(); return; }

  const r = await api.launch({ nickname: nick });
  // Discord — статус "играет"
  if (r.ok) {
    try {
      const ping = await api.serverPing(srv.key);
      await api.discordSetPlaying({ playerCount: ping.online ? ping.online_count : null });
    } catch {}
  }

  setTimeout(() => {
    $('btn-play').disabled = false;
    $('btn-play').textContent = '▶ ЗАПУСТИТЬ';
    updatePlayBtn();
  }, 5000);

  if (!r.ok) {
    $('mst').className = 'mods-st err';
    $('mst').textContent = '✗ ' + (r.error || 'Ошибка запуска');
  } else {
    $('mst').className = 'mods-st ok';
    $('mst').textContent = '✓ Майнкрафт запускается...';
    if (r.logPath) {
      $('mods-fname').textContent = 'Лог: ' + r.logPath;
    }
  }
};

// ─── СЕРВЕРЫ (servers.json) ──────────────────────────────────────────────────
// Все серверы видны в сетке карточек. «Играть» доступна только у активных (active=true); у остальных —
// бейдж «Скоро». Включение сервера — правка servers.json в репозитории, без правки кода.
let SERVERS = [];          // карточки с бэкенда + поля для интерфейса
let currentServer = 0;     // индекс выбранного сервера в SERVERS
const pings = {};          // id → последний результат пинга

function mapServer(v) {
  const loader = v.loader === 'forge' ? 'Forge' : v.loader === 'neoforge' ? 'NeoForge' : '';
  const mc = v.mcVersion === 'latest' ? 'последняя версия' : v.mcVersion;
  return Object.assign({}, v, {
    key: v.id,
    ver: 'Minecraft ' + mc + (loader ? ' · ' + loader : v.type === 'vanilla' ? ' · без модов' : ''),
    addr: v.address || '',
  });
}

async function loadServers() {
  const r = await api.listServers();
  if (!r || !r.ok) return;
  const prevId = curServer() ? curServer().key : null;
  SERVERS = r.servers.map(mapServer);
  currentServer = Math.max(0, SERVERS.findIndex(s => s.id === r.selected));
  renderServerGrid();
  renderServerDetail();
  fillSettings();
  window.dispatchEvent(new CustomEvent('server-changed', { detail: { idx: currentServer, server: curServer() } }));
  // состояние пересчитываем при первой загрузке и когда сменился выбранный сервер/его активность
  if (prevId !== (curServer() && curServer().key) || !mcInstalledKnown) {
    mcInstalledKnown = true;
    await applyServerState();
  }
  pingAll();
}
let mcInstalledKnown = false;

function renderServerGrid() {
  const grid = $('sv-grid');
  if (!grid) return;
  grid.innerHTML = '';
  SERVERS.forEach((s, i) => {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'svc' + (i === currentServer ? ' on' : '') + (s.active ? '' : ' off');
    card.dataset.id = s.id;
    const badges = [];
    if (!s.active) badges.push('<span class="svc-badge soon">Скоро</span>');
    if (s.subscription) badges.push('<span class="svc-badge sub">по подписке</span>');
    if (s.active && s.installed) badges.push('<span class="svc-badge ok">установлен</span>');
    const p = pings[s.id];
    const online = !s.active ? '' : p === undefined ? '<span class="svc-ping">…</span>'
      : p.online ? `<span class="svc-ping on"><i></i>${p.online_count !== undefined ? p.online_count + '/' + p.max_count : 'онлайн'} · ${p.ping} мс</span>`
      : '<span class="svc-ping off"><i></i>недоступен</span>';
    card.innerHTML = `
      <div class="svc-top"><span class="svc-ic">${escapeHtml(s.icon || '⛏')}</span>
        <span class="svc-name">${escapeHtml(s.type === 'vanilla' && s.subscription ? 'Ваниль · по подписке' : s.name)}</span></div>
      <div class="svc-ver">${escapeHtml(s.ver)}</div>
      <div class="svc-desc">${escapeHtml(s.description || '')}</div>
      <div class="svc-foot">${badges.join('')}${online}</div>`;
    card.addEventListener('click', () => selectServer(i));
    grid.appendChild(card);
  });
}

function renderServerDetail() {
  const s = curServer();
  if (!s) return;
  if ($('sv-name'))  $('sv-name').textContent  = s.name;
  if ($('sv-ver'))   $('sv-ver').textContent   = s.ver;
  if ($('sv-addr'))  $('sv-addr').textContent  = s.addr || (s.active ? '' : 'адрес появится позже');
  if ($('sv-icon'))  $('sv-icon').textContent  = s.icon || '⛏';
  if ($('pg-sub-home')) $('pg-sub-home').textContent = '// ' + s.name + ' · ' + s.ver;
  const dot = $('sv-dot'), txt = $('sv-status-txt');
  const p = pings[s.id];
  if (!s.active) {
    if (dot) dot.className = 'sv-dot';
    if (txt) { txt.className = 'sv-status-txt'; txt.textContent = 'Сервер пока недоступен'; }
  } else if (p === undefined) {
    if (dot) dot.className = 'sv-dot';
    if (txt) { txt.className = 'sv-status-txt'; txt.textContent = 'Проверяю...'; }
  }
}

async function selectServer(idx) {
  if (idx < 0 || idx >= SERVERS.length || idx === currentServer) return;
  if (isInstalling || modsSyncRunning) { showActionStatus('Дождись завершения установки/синхронизации', true); return; }
  const r = await api.selectServer(SERVERS[idx].id);
  if (!r || !r.ok) { showActionStatus('✗ ' + ((r && r.error) || 'Не удалось выбрать сервер'), true); return; }
  currentServer = idx;
  modsReady = false;
  serverOnline = pings[SERVERS[idx].id] ? !!pings[SERVERS[idx].id].online : null;
  renderServerGrid();
  renderServerDetail();
  window.dispatchEvent(new CustomEvent('server-changed', { detail: { idx, server: SERVERS[idx] } }));
  await applyServerState();
  fillSettings();
  pingAll();
}

// ─── SERVER PING ─────────────────────────────────────────────────────────────
// Пингуются только активные серверы с адресом; результат выбранного определяет доступность кнопки «Играть».
async function pingAll() {
  const targets = SERVERS.filter(s => s.active && s.addr);
  await Promise.all(targets.map(async s => {
    try { pings[s.id] = await api.serverPing(s.id); } catch { pings[s.id] = { online: false }; }
  }));
  const cur = curServer();
  if (cur && cur.active) serverOnline = pings[cur.id] ? !!pings[cur.id].online : false;
  renderServerGrid();
  pingServer();
}

function pingServer() {
  const dot = $('sv-dot');
  const txt = $('sv-status-txt');
  const cur = curServer();
  if (!dot || !txt || !cur || !cur.active) { updatePlayBtn(); return; }
  const r = pings[cur.id];
  if (!r) { updatePlayBtn(); return; }
  if (r.online) {
    dot.className = 'sv-dot online';
    txt.className = 'sv-status-txt online';
    txt.textContent = r.online_count !== undefined
      ? `Онлайн · ${r.online_count}/${r.max_count} · ${r.ping} мс`
      : `Онлайн · ${r.ping} мс`;
  } else {
    dot.className = 'sv-dot offline';
    txt.className = 'sv-status-txt offline';
    txt.textContent = 'Сервер недоступен';
  }
  updatePlayBtn(); // ТЗ 3.4: пересчитываем доступность кнопки «Играть» при каждом опросе
}


// ─── SETTINGS ACTIONS ────────────────────────────────────────────────────────
function showActionStatus(msg, isErr = false) {
  const el = $('action-status');
  if (!el) return;
  el.textContent = msg;
  el.className = 'action-status show' + (isErr ? ' err' : '');
  setTimeout(() => el.classList.remove('show'), 4000);
}

$('btn-check-mods')?.addEventListener('click', async () => {
  $('btn-check-mods').disabled = true;
  const r = await api.checkModsForce();
  $('btn-check-mods').disabled = false;
  if (r.ok) showActionStatus('✓ Версия сброшена — моды скачаются при следующем запуске');
  else showActionStatus('✗ ' + r.error, true);
});

$('btn-reinstall-client')?.addEventListener('click', async () => {
  if (!confirm('Удалить клиент выбранного сервера полностью и начать установку заново?\nДругие серверы, Java и настройки лаунчера не затрагиваются.')) return;
  $('btn-reinstall-client').disabled = true;
  const r = await api.reinstallClient();
  $('btn-reinstall-client').disabled = false;
  if (r.ok) {
    showActionStatus('✓ Клиент удалён — перейди на вкладку ИГРАТЬ и нажми УСТАНОВИТЬ');
    // Обновляем статус установки
    mcInstalled = false;
    modsReady   = false;
    showInstallBlock();
    // Переходим на вкладку Играть
    document.querySelectorAll('.ni').forEach(n => n.classList.remove('on'));
    document.querySelectorAll('.pg').forEach(p => p.classList.remove('on'));
    document.querySelector('.ni[data-p="home"]')?.classList.add('on');
    $('pg-home')?.classList.add('on');
  } else {
    showActionStatus('✗ ' + r.error, true);
  }
});

$('btn-open-dir')?.addEventListener('click', () => api.openLauncherDir());
$('btn-check-upd')?.addEventListener('click', async () => {
  $('btn-check-upd').disabled = true;
  await checkLauncherUpdate(true);
  $('btn-check-upd').disabled = false;
});


// ─── MC RUNNING STATE ────────────────────────────────────────────────────────
api.onMcRunning(({ running }) => {
  const btn = $('btn-play');
  if (!btn) return;
  if (running) {
    btn.disabled = true;
    btn.textContent = '🎮 ИГРА ЗАПУЩЕНА';
    btn.style.opacity = '0.7';
  } else {
    btn.style.opacity = '';
    btn.textContent = '▶ ЗАПУСТИТЬ';
    updatePlayBtn(); // восстанавливаем состояние
  }
});


// ─── LEGAL LINKS (Privacy Policy + Terms of Service) ─────────────────────────
const PRIVACY_URL = 'https://github.com/ViPok137/ViLauncher/blob/main/Privacy%20Policy';
const TOS_URL     = 'https://github.com/ViPok137/ViLauncher/blob/main/Terms%20of%20Service';

// Вешаем на все копии ссылок (в настройках и в сайдбаре)
['link-privacy', 'link-privacy-sb'].forEach(id => {
  $(id)?.addEventListener('click', () => api.openUrl(PRIVACY_URL));
});
['link-tos', 'link-tos-sb'].forEach(id => {
  $(id)?.addEventListener('click', () => api.openUrl(TOS_URL));
});

// ─── NEWS ─────────────────────────────────────────────────────────────────────
let newsLoaded = false;
async function loadNews() {
  if (newsLoaded) return;
  const items = await api.fetchNews();
  newsLoaded = true;
  const list = $('nw-list');
  list.innerHTML = '';
  if (!items?.length) {
    list.innerHTML = '<div style="color:var(--muted);font-family:\'Share Tech Mono\',monospace;font-size:12px">// Новостей нет</div>';
    return;
  }
  // Сортируем от новых к старым
  const sorted = [...items].sort((a, b) => {
    const da = new Date(a.date || 0), db = new Date(b.date || 0);
    return db - da;
  });
  sorted.forEach(it => {
    const d = document.createElement('div');
    d.className = 'nw-card';
    d.innerHTML = `<div class="nw-date">// ${e(it.date)}</div><div class="nw-title">${e(it.title)}</div><div class="nw-body">${e(it.body)}</div>`;
    list.appendChild(d);
  });
}

function e(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

// ─── SETTINGS ────────────────────────────────────────────────────────────────
async function fillSettings() {
  const info = await api.getAppInfo();
  const ver  = 'v' + (info.version || '1.0.0');

  if ($('ver-txt')) $('ver-txt').textContent = ver;
  if ($('s-lver'))  $('s-lver').textContent  = ver;
  if ($('s-mver'))  $('s-mver').textContent  = info.modsVersion || 'Не установлены';
  const cs = curServer();
  if ($('s-server')) $('s-server').textContent = cs ? cs.name + (cs.addr ? ' · ' + cs.addr : '') : (info.server || '—');
  if ($('s-mc'))     $('s-mc').textContent     = cs ? cs.ver.replace(/^Minecraft /, '') : '—';
  if ($('s-elec'))  $('s-elec').textContent  = info.electron    || '—';
  if ($('s-os'))    $('s-os').textContent     = info.os          || '—';
  if ($('s-java'))  $('s-java').textContent   = info.java        || '—';
  if ($('s-ram'))   $('s-ram').textContent    = info.ram         || '—';

  // ТЗ 3a: minimizeToTray checkbox
  const cbTray = $('cb-minimize-tray');
  if (cbTray) {
    cbTray.checked = info.minimizeToTray !== false;
    cbTray.onchange = () => api.settingSet({ key: 'minimizeToTray', value: cbTray.checked });
  }

  // ТЗ 3.2: обновляем состояние dev-панели при каждом открытии настроек
  await refreshDevPanel();

  await loadRamSettings();
}

// ─── RAM SETTINGS UI ─────────────────────────────────────────────────────────
let ramData = null;

async function loadRamSettings() {
  ramData = await api.ramGet();
  if (!ramData) return;

  const maxSlider = $('ram-max-slider');
  const minSlider = $('ram-min-slider');
  const maxVal    = $('ram-max-val');
  const minVal    = $('ram-min-val');
  const info      = $('ram-info');
  const autoBtn   = $('btn-ram-auto');

  if (!maxSlider) return;

  // Ограничиваем максимум слайдера по RAM системы
  const sysMax = Math.min(Math.floor(ramData.totalGb * 0.75), 32);
  maxSlider.max = sysMax;
  minSlider.max = Math.min(sysMax - 1, 8);

  info.textContent = `Всего RAM: ${ramData.totalGb} ГБ · Рекомендуется: ${ramData.autoMin}–${ramData.autoMax} ГБ`;

  // Устанавливаем текущие значения
  const isAuto = ramData.currentMax === 'auto';
  if (isAuto) {
    maxSlider.value = ramData.autoMax;
    minSlider.value = ramData.autoMin;
    autoBtn.classList.add('active');
    maxSlider.disabled = true;
    minSlider.disabled = true;
  } else {
    maxSlider.value = parseInt(ramData.currentMax) || ramData.autoMax;
    minSlider.value = parseInt(ramData.currentMin) || ramData.autoMin;
    autoBtn.classList.remove('active');
    maxSlider.disabled = false;
    minSlider.disabled = false;
  }
  maxVal.textContent = maxSlider.value + ' ГБ';
  minVal.textContent = minSlider.value + ' ГБ';

  // Слайдеры
  maxSlider.oninput = () => {
    if (parseInt(maxSlider.value) <= parseInt(minSlider.value)) {
      minSlider.value = Math.max(1, parseInt(maxSlider.value) - 1);
      minVal.textContent = minSlider.value + ' ГБ';
    }
    maxVal.textContent = maxSlider.value + ' ГБ';
    autoBtn.classList.remove('active');
  };
  minSlider.oninput = () => {
    if (parseInt(minSlider.value) >= parseInt(maxSlider.value)) {
      maxSlider.value = Math.min(sysMax, parseInt(minSlider.value) + 1);
      maxVal.textContent = maxSlider.value + ' ГБ';
    }
    minVal.textContent = minSlider.value + ' ГБ';
    autoBtn.classList.remove('active');
  };

  // Кнопка Авто
  $('btn-ram-auto').onclick = async () => {
    await api.ramSet({ maxG: 'auto', minG: 'auto' });
    maxSlider.value   = ramData.autoMax;
    minSlider.value   = ramData.autoMin;
    maxVal.textContent = ramData.autoMax + ' ГБ';
    minVal.textContent = ramData.autoMin + ' ГБ';
    maxSlider.disabled = true;
    minSlider.disabled = true;
    autoBtn.classList.add('active');
    showRamSaved();
  };

  // Кнопка Сохранить
  $('btn-ram-save').onclick = async () => {
    await api.ramSet({
      maxG: maxSlider.value,
      minG: minSlider.value,
    });
    maxSlider.disabled = false;
    minSlider.disabled = false;
    autoBtn.classList.remove('active');
    showRamSaved();
  };
}

function showRamSaved() {
  const el = $('ram-saved');
  if (!el) return;
  el.classList.add('show');
  setTimeout(() => el.classList.remove('show'), 3000);
}

// ─── APPEARANCE TAB (скин) ────────────────────────────────────────────────────
let appearanceInited = false;
let mrCurrentType = 'shader';
let mrSearchTimeout = null;

async function initAppearanceTab() {
  if (appearanceInited) return;
  appearanceInited = true;

  // ── Кастомный скин ──
  await refreshSkinPreview();

  $('btn-skin-upload').onclick = async () => {
    const r = await api.skinUpload();
    if (r.ok) {
      showSkinSaved('✓ Скин загружен');
      await refreshSkinPreview();
      // Обновляем и аватарку в сайдбаре
      if (currentNick) updatePlayerCard(currentNick);
    } else if (r.error) {
      showSkinSaved('✗ ' + r.error, true);
    }
  };

  $('btn-skin-reset').onclick = async () => {
    await api.skinReset();
    showSkinSaved('✓ Сброшено — используется скин с сервера');
    await refreshSkinPreview();
    if (currentNick) updatePlayerCard(currentNick);
  };
}

// ─── РАЗДЕЛ СЕРВЕРА: шейдеры и ресурспаки (Modrinth) ──────────────────────────
// Окно Modrinth живёт на главной странице внутри раздела сервера (панель открывается по клику);
// инициализируется один раз при первом открытии вкладки «Шейдеры и ресурспаки».
let serverToolsInited = false;
function initServerTools() {
  if (serverToolsInited) { refreshInstalledList(); return; }
  serverToolsInited = true;
  // ── Переключатель Шейдеры / Ресурспаки ──
  $('mr-tab-shader').onclick = () => switchMrType('shader');
  $('mr-tab-rp').onclick     = () => switchMrType('resourcepack');

  // ── Поиск с debounce ──
  $('mr-search').addEventListener('input', () => {
    clearTimeout(mrSearchTimeout);
    mrSearchTimeout = setTimeout(() => doMrSearch($('mr-search').value), 400);
  });

  // ── Прогресс скачивания ──
  api.onModrinthDlProg(({ filename, pct }) => {
    const btn = document.querySelector(`[data-filename="${CSS.escape(filename)}"]`);
    if (btn) btn.textContent = `${pct}%`;
  });

  // Список шейдеров/ресурспаков перерисовывается при смене сервера (активные паки — по серверу)
  window.addEventListener('server-changed', () => { refreshInstalledList(); });

  // Первая загрузка
  doMrSearch('');
  refreshInstalledList();
}


function switchMrType(type) {
  mrCurrentType = type;
  $('mr-tab-shader').classList.toggle('on', type === 'shader');
  $('mr-tab-rp').classList.toggle('on', type === 'resourcepack');
  doMrSearch($('mr-search').value);
  refreshInstalledList();
}

async function refreshSkinPreview() {
  const skinDef = $('av-skin-default');
  const canvas  = $('av-skin-canvas');
  const status  = $('av-skin-status');

  if (!currentNick) return;
  const { skinUrl, isCustom } = await api.skinGet(currentNick);

  status.textContent = isCustom
    ? '📌 Используется свой скин'
    : (skinUrl ? '🌐 Используется скин с сервера' : '— Скин не задан');

  if (skinDef) { skinDef.textContent = (currentNick[0]||'?').toUpperCase(); skinDef.style.display = 'flex'; }
  if (canvas) canvas.style.display = 'none';

  if (!skinUrl || !canvas) return;

  const img = new Image();
  img.onload = () => {
    try {
      // ТЗ 2.3: нативный drawImage вместо попиксельного цикла.
      // SIZE должен точно совпадать с CSS-размером canvas — задаём явно
      // (родительский .sb-skin контейнер здесь 48px, а не общий 64px)
      const SIZE = 48;
      canvas.width  = SIZE;
      canvas.height = SIZE;
      canvas.style.width  = SIZE + 'px';
      canvas.style.height = SIZE + 'px';

      const ctx = canvas.getContext('2d');
      ctx.imageSmoothingEnabled = false;
      ctx.clearRect(0, 0, SIZE, SIZE);
      ctx.drawImage(img, 8,  8, 8, 8, 0, 0, SIZE, SIZE); // лицо
      ctx.drawImage(img, 40, 8, 8, 8, 0, 0, SIZE, SIZE); // оверлей/шлем

      canvas.style.display = 'block';
      if (skinDef) skinDef.style.display = 'none';
    } catch {}
  };
  img.src = skinUrl;
}

function showSkinSaved(text, isErr = false) {
  const el = $('skin-saved');
  el.textContent = text;
  el.style.color = isErr ? 'var(--p)' : 'var(--ok)';
  el.classList.add('show');
  setTimeout(() => el.classList.remove('show'), 3000);
}

async function doMrSearch(query) {
  const results = $('mr-results');
  results.innerHTML = '<div style="font-family:\'Share Tech Mono\',monospace;font-size:10px;color:var(--muted)">Ищу...</div>';

  const r = await api.modrinthSearch({ query, type: mrCurrentType, limit: 15 });
  if (!r.ok || !r.hits?.length) {
    results.innerHTML = '<div style="font-family:\'Share Tech Mono\',monospace;font-size:10px;color:var(--muted)">Ничего не найдено</div>';
    return;
  }

  // ТЗ 2d: проверяем что уже есть в shared/ чтобы показать Toggle вместо Download
  const srv = SERVERS[currentServer] || SERVERS[0];
  const serverKey = srv.key || '0';
  const [installedR, activeR] = await Promise.all([
    api.modrinthInstalled({ type: mrCurrentType }),
    api.modrinthActive({ type: mrCurrentType, serverKey }),
  ]);
  const installedFiles = new Set(installedR.files || []);
  const activeFiles    = new Set(activeR.files    || []);

  results.innerHTML = '';
  r.hits.forEach(hit => {
    const card = document.createElement('div');
    card.className = 'mr-card';

    // Определяем есть ли такой файл в shared (по совпадению projectId в названии файла — эвристика)
    // Точное совпадение filename будет при повторном поиске после скачивания
    const matchedFile = [...installedFiles].find(f => f.toLowerCase().includes(
      hit.title.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '').slice(0, 10)
    ));
    const isDownloaded = !!matchedFile;
    const isActive     = matchedFile ? activeFiles.has(matchedFile) : false;

    let actionBtn;
    if (isDownloaded) {
      // ТЗ 2d: уже в shared/ — показываем Toggle «Включить/Выключить для этого сервера»
      actionBtn = `<label class="mr-toggle-label" title="Включить для ${srv.name}">
        <input type="checkbox" class="mr-toggle-cb" data-file="${escapeHtml(matchedFile)}" ${isActive ? 'checked' : ''}>
        <span>${isActive ? '✓ Вкл' : 'Выкл'}</span>
      </label>`;
    } else {
      actionBtn = `<button class="mr-dl-btn" data-id="${hit.id}">⬇ Скачать</button>`;
    }

    card.innerHTML = `
      <img class="mr-icon" src="${hit.iconUrl || ''}" onerror="this.style.visibility='hidden'">
      <div class="mr-info">
        <div class="mr-title">${escapeHtml(hit.title)}</div>
        <div class="mr-desc">${escapeHtml(hit.description || '')} · ${formatDownloads(hit.downloads)} загрузок</div>
      </div>
      ${actionBtn}
    `;

    // Обработчик кнопки Download
    const dlBtn = card.querySelector('.mr-dl-btn');
    if (dlBtn) dlBtn.onclick = (e) => downloadMrProject(hit.id, e.target, serverKey);

    // Обработчик Toggle
    const cb = card.querySelector('.mr-toggle-cb');
    if (cb) cb.onchange = async () => {
      const label = cb.nextElementSibling;
      const r2 = await api.modrinthToggle({
        filename:  cb.dataset.file,
        type:      mrCurrentType,
        serverKey: serverKey,
        enabled:   cb.checked,
      });
      if (r2.ok && label) label.textContent = cb.checked ? '✓ Вкл' : 'Выкл';
    };

    results.appendChild(card);
  });
}

async function downloadMrProject(projectId, btnEl, serverKey) {
  btnEl.disabled = true;
  btnEl.textContent = '...';

  const vr = await api.modrinthVersions({ projectId });
  if (!vr.ok || !vr.versions?.length) {
    btnEl.textContent = '✗ Нет версий';
    setTimeout(() => { btnEl.textContent = '⬇ Скачать'; btnEl.disabled = false; }, 2000);
    return;
  }

  const version = vr.versions[0];
  const file    = version.files.find(f => f.primary) || version.files[0];
  if (!file) { btnEl.textContent = '✗ Нет файла'; return; }

  btnEl.setAttribute('data-filename', file.filename);
  btnEl.textContent = '0%';

  // ТЗ 2a: скачиваем в shared/ и сразу активируем для текущего сервера
  const r = await api.modrinthDownload({
    url:       file.url,
    filename:  file.filename,
    type:      mrCurrentType,
    serverKey: serverKey || (SERVERS[currentServer] || SERVERS[0]).key || '0',
  });
  if (r.ok) {
    btnEl.textContent = '✓ Установлено';
    refreshInstalledList();
    // Перерисовываем результаты чтобы кнопка стала Toggle
    setTimeout(() => doMrSearch($('mr-search').value), 800);
  } else {
    btnEl.textContent = '✗ Ошибка';
    setTimeout(() => { btnEl.textContent = '⬇ Скачать'; btnEl.disabled = false; }, 2000);
  }
}

async function refreshInstalledList() {
  const list = $('mr-installed');
  const srv  = SERVERS[currentServer] || SERVERS[0];
  const serverKey = srv.key || '0';

  const [instR, actR] = await Promise.all([
    api.modrinthInstalled({ type: mrCurrentType }),
    api.modrinthActive({ type: mrCurrentType, serverKey }),
  ]);

  if (!instR.ok || !instR.files?.length) {
    list.innerHTML = '<div style="font-family:\'Share Tech Mono\',monospace;font-size:10px;color:var(--muted)">Ничего не установлено</div>';
    return;
  }

  const activeSet = new Set(actR.files || []);
  list.innerHTML = '';
  instR.files.forEach(filename => {
    const isActive = activeSet.has(filename);
    const item = document.createElement('div');
    item.className = 'mr-installed-item';
    item.innerHTML = `
      <span>${escapeHtml(filename)}</span>
      <div style="display:flex;gap:8px;align-items:center">
        <label style="font-size:10px;color:var(--c);cursor:pointer">
          <input type="checkbox" class="mr-active-cb" data-file="${escapeHtml(filename)}" ${isActive ? 'checked' : ''}
            style="accent-color:var(--c)"> Активен
        </label>
        <span class="mr-remove-btn" title="Удалить из shared/">✕</span>
      </div>
    `;
    item.querySelector('.mr-active-cb').onchange = async (e) => {
      await api.modrinthToggle({ filename, type: mrCurrentType, serverKey, enabled: e.target.checked });
    };
    item.querySelector('.mr-remove-btn').onclick = async () => {
      await api.modrinthRemove({ filename, type: mrCurrentType, serverKey });
      refreshInstalledList();
    };
    list.appendChild(item);
  });
}

function formatDownloads(n) {
  if (n >= 1000000) return (n/1000000).toFixed(1) + 'M';
  if (n >= 1000) return (n/1000).toFixed(1) + 'K';
  return String(n || 0);
}

function escapeHtml(s) {
  const d = document.createElement('div');
  d.textContent = s || '';
  return d.innerHTML;
}

// ─── ТЗ 2.1: CSP compliance — все обработчики вынесены сюда, без inline onclick ──
document.getElementById('sv-tab-0')?.addEventListener('click', () => selectServer(0));

// ─── ТЗ 3: ПАНЕЛЬ РАЗРАБОТЧИКА ───────────────────────────────────────────────

// Отображает/скрывает dev-панель и синхронизирует чекбоксы с бэкендом
async function refreshDevPanel() {
  const status = await api.devStatus();
  const panel  = $('dev-panel');
  const badge  = $('dev-badge');

  // Показываем/скрываем панель и бейдж
  if (panel) panel.classList.toggle('hidden', !status.devMode);
  if (badge) badge.style.display = status.devMode ? 'inline-block' : 'none';

  if (!status.devMode) return;

  // ТЗ 3.2: синхронизируем состояние чекбоксов из settings.json
  const cbUpdates   = $('cb-dev-no-updates');
  const cbAnticheat = $('cb-dev-no-anticheat');

  if (cbUpdates) {
    cbUpdates.checked = !!status.devDisableUpdates;
    cbUpdates.onchange = async () => {
      await api.devFlagSet({ flag: 'devDisableUpdates', value: cbUpdates.checked });
    };
  }
  if (cbAnticheat) {
    cbAnticheat.checked = !!status.devDisableAnticheat;
    cbAnticheat.onchange = async () => {
      await api.devFlagSet({ flag: 'devDisableAnticheat', value: cbAnticheat.checked });
    };
  }
}

// ТЗ 3.1: Ctrl+Alt+Shift+C → кастомный модальный диалог ввода кода
// (prompt() не работает в Electron с contextIsolation=true)
document.addEventListener('keydown', async (e) => {
  if (!(e.ctrlKey && e.altKey && e.shiftKey && e.code === 'KeyC')) return;
  e.preventDefault();

  // Создаём модальный оверлей прямо в DOM лаунчера
  const overlay = document.createElement('div');
  overlay.id = 'dev-modal-overlay';
  overlay.style.cssText = [
    'position:fixed;inset:0;z-index:9999',
    'background:rgba(4,8,16,.85)',
    'display:flex;align-items:center;justify-content:center',
    'backdrop-filter:blur(4px)',
  ].join(';');

  overlay.innerHTML = `
    <div style="background:#0a1628;border:1px solid #00f5ff;border-radius:10px;padding:28px 32px;
                width:320px;box-shadow:0 0 40px rgba(0,245,255,.15)">
      <div style="font-family:'Orbitron',monospace;font-size:13px;font-weight:700;
                  color:#00f5ff;letter-spacing:3px;margin-bottom:6px">ПАНЕЛЬ РАЗРАБОТЧИКА</div>
      <div style="font-family:'Share Tech Mono',monospace;font-size:10px;color:#4a7090;margin-bottom:18px">
        Введи код доступа (DEV_CODE из config.js)
      </div>
      <input id="dev-code-input" type="password" autocomplete="off"
        style="width:100%;box-sizing:border-box;padding:10px 14px;
               background:#040810;border:1px solid #00f5ff;border-radius:6px;
               color:#fff;font-size:18px;letter-spacing:6px;outline:none;font-family:monospace">
      <div style="display:flex;gap:10px;margin-top:16px">
        <button id="dev-modal-cancel"
          style="flex:1;padding:9px;background:transparent;border:1px solid #2a4060;
                 color:#4a7090;border-radius:6px;cursor:pointer;
                 font-family:'Share Tech Mono',monospace;font-size:11px">
          Отмена
        </button>
        <button id="dev-modal-ok"
          style="flex:1;padding:9px;background:rgba(0,245,255,.08);border:1px solid #00f5ff;
                 color:#00f5ff;border-radius:6px;cursor:pointer;
                 font-family:'Share Tech Mono',monospace;font-size:11px">
          OK
        </button>
      </div>
      <div id="dev-modal-err" style="font-family:'Share Tech Mono',monospace;font-size:10px;
           color:#ff2d78;margin-top:10px;min-height:16px;text-align:center"></div>
    </div>`;

  document.body.appendChild(overlay);
  const input = $('dev-code-input');
  const errEl = $('dev-modal-err');
  input.focus();

  async function tryAuth() {
    const code = input.value.trim();
    if (!code) return;
    const r = await api.devAuth({ code });
    if (r.ok) {
      document.body.removeChild(overlay);
      // Переходим на страницу настроек и обновляем панель
      document.querySelectorAll('.ni').forEach(n => n.classList.remove('on'));
      document.querySelectorAll('.pg').forEach(p => p.classList.remove('on'));
      document.querySelector('.ni[data-p="settings"]')?.classList.add('on');
      $('pg-settings')?.classList.add('on');
      await fillSettings();
    } else {
      errEl.textContent = '✗ Неверный код доступа';
      input.value = '';
      input.focus();
      setTimeout(() => { errEl.textContent = ''; }, 2000);
    }
  }

  $('dev-modal-ok').onclick     = tryAuth;
  $('dev-modal-cancel').onclick = () => document.body.removeChild(overlay);
  input.addEventListener('keydown', (ke) => {
    if (ke.key === 'Enter')  tryAuth();
    if (ke.key === 'Escape') document.body.removeChild(overlay);
  });
});

// Проверяем devMode при старте — восстанавливаем состояние панели
(async () => {
  const s = await api.devStatus();
  if (s.devMode) {
    const badge = $('dev-badge');
    if (badge) badge.style.display = 'inline-block';
  }
})();
