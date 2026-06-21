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

function updatePlayBtn() {
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
  pingServer();
  setInterval(pingServer, 30000);

  const { installed } = await api.installCheck();
  mcInstalled = installed;

  if (!installed) {
    showInstallBlock();
  } else {
    hideInstallBlock();
    runModsSync();
  }

  updatePlayBtn();
})();

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
    runModsSync();
  } else {
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
    st.className   = 'mods-st ok';
    fcounter.textContent = '';
    st.textContent = r.updated
      ? `✓ Моды обновлены до версии ${r.version}`
      : `✓ ${r.msg}`;
  } else {
    st.className   = 'mods-st err';
    const err  = r.error || 'Ошибка синхронизации';
    const hint = /HTTP 4/.test(err) ? ' — manifest.json не найден на GitHub' :
                 /Timeout/.test(err) ? ' — GitHub недоступен, проверь интернет' : '';
    st.textContent = '✗ ' + err + hint;
  }

  updatePlayBtn();
}

// ─── LAUNCHER UPDATE ─────────────────────────────────────────────────────────
let pendingUrl = null;
async function checkLauncherUpdate() {
  const r = await api.launcherCheck();
  if (!r.hasUpdate) return;
  pendingUrl = r.url;
  $('bn-txt').textContent = `⬆ Новая версия лаунчера: ${r.remote} (у тебя ${r.local})`;
  $('banner').classList.add('on');
}
$('bn-upd').onclick = async () => {
  if (!pendingUrl) return;
  $('bn-upd').disabled = true;
  api.onLauncherDl(({ pct }) => {
    $('bn-txt').textContent = `⏳ Скачиваю... ${pct}%`;
  });
  const r = await api.launcherUpdate({ url: pendingUrl });
  if (!r.ok) {
    $('bn-txt').textContent = '❌ ' + r.error;
    $('bn-upd').disabled = false;
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

  const r = await api.launch({ nickname: nick });
  // Discord — статус "играет"
  if (r.ok) {
    try {
      const ping = await api.serverPing();
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

// ─── SERVER TABS ─────────────────────────────────────────────────────────────
const SERVERS = [
  {
    name: 'VIPOK SERVER', ver: 'Minecraft 1.20.1 · Forge',
    addr: 'subjects-emirates.gl.joinmc.link', icon: '⛏',
  },
  // Добавь данные второго сервера сюда когда будет готов:
  // { name: 'СЕРВЕР 2', ver: '1.20.1 · Forge', addr: 'server2.example.com', icon: '🏰' },
  // { name: 'СЕРВЕР 3', ver: '1.19.4 · Fabric', addr: 'server3.example.com', icon: '🌿' },
];

let currentServer = 0;

function selectServer(idx) {
  if (idx >= SERVERS.length) return;
  currentServer = idx;
  document.querySelectorAll('.sv-tab').forEach((t, i) => t.classList.toggle('on', i === idx));
  const s = SERVERS[idx];
  if ($('sv-name'))  $('sv-name').textContent  = s.name;
  if ($('sv-ver'))   $('sv-ver').textContent   = s.ver;
  if ($('sv-addr'))  $('sv-addr').textContent  = s.addr;
  if ($('sv-icon'))  $('sv-icon').textContent  = s.icon || '⛏';
  // Сбрасываем статус и пингуем новый сервер
  const dot = $('sv-dot'), txt = $('sv-status-txt');
  if (dot) dot.className = 'sv-dot';
  if (txt) { txt.className = 'sv-status-txt'; txt.textContent = 'Проверяю...'; }
  pingServer();
}

// ─── SERVER PING ─────────────────────────────────────────────────────────────
async function pingServer() {
  const dot = $('sv-dot');
  const txt = $('sv-status-txt');
  if (!dot || !txt) return;

  try {
    const r = await api.serverPing();
    if (r.online) {
      serverOnline = true; // ТЗ 3.4
      dot.className = 'sv-dot online';
      txt.className = 'sv-status-txt online';
      const tabDot = $('sv-tab-dot-' + currentServer);
      if (tabDot) tabDot.className = 'sv-tab-dot online';
      if (r.online_count !== undefined) {
        txt.textContent = `Онлайн · ${r.online_count}/${r.max_count} · ${r.ping} мс`;
      } else {
        txt.textContent = `Онлайн · ${r.ping} мс`;
      }
    } else {
      serverOnline = false; // ТЗ 3.4
      dot.className = 'sv-dot offline';
      txt.className = 'sv-status-txt offline';
      const tabDotOff = $('sv-tab-dot-' + currentServer);
      if (tabDotOff) tabDotOff.className = 'sv-tab-dot offline';
      txt.textContent = 'Сервер недоступен';
    }
  } catch {
    serverOnline = false; // ТЗ 3.4: ошибка проверки = недоступен, кнопку не разблокируем
    dot.className = 'sv-dot offline';
    txt.className = 'sv-status-txt offline';
    txt.textContent = 'Ошибка проверки';
  }
  updatePlayBtn(); // ТЗ 3.4: пересчитываем доступность кнопки ЗАПУСТИТЬ при каждом опросе
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
  if (!confirm('Удалить папку client полностью и начать установку заново?')) return;
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
  if ($('s-elec'))  $('s-elec').textContent  = info.electron    || '—';
  if ($('s-os'))    $('s-os').textContent     = info.os          || '—';
  if ($('s-java'))  $('s-java').textContent   = info.java        || '—';
  if ($('s-ram'))   $('s-ram').textContent    = info.ram         || '—';

  // Загружаем настройки RAM
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

// ─── APPEARANCE TAB (скин + Modrinth) ─────────────────────────────────────────
let appearanceInited = false;
let mrCurrentType = 'shader';
let mrSearchTimeout = null;

async function initAppearanceTab() {
  if (appearanceInited) {
    refreshInstalledList();
    return;
  }
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

  results.innerHTML = '';
  r.hits.forEach(hit => {
    const card = document.createElement('div');
    card.className = 'mr-card';
    card.innerHTML = `
      <img class="mr-icon" src="${hit.iconUrl || ''}" onerror="this.style.visibility='hidden'">
      <div class="mr-info">
        <div class="mr-title">${escapeHtml(hit.title)}</div>
        <div class="mr-desc">${escapeHtml(hit.description || '')} · ${formatDownloads(hit.downloads)} загрузок</div>
      </div>
      <button class="mr-dl-btn" data-id="${hit.id}">⬇ Скачать</button>
    `;
    card.querySelector('.mr-dl-btn').onclick = (e) => downloadMrProject(hit.id, e.target);
    results.appendChild(card);
  });
}

async function downloadMrProject(projectId, btnEl) {
  btnEl.disabled = true;
  btnEl.textContent = '...';

  const vr = await api.modrinthVersions({ projectId });
  if (!vr.ok || !vr.versions?.length) {
    btnEl.textContent = '✗ Нет версий';
    setTimeout(() => { btnEl.textContent = '⬇ Скачать'; btnEl.disabled = false; }, 2000);
    return;
  }

  // Берём самую новую версию, primary файл (или первый файл)
  const version = vr.versions[0];
  const file = version.files.find(f => f.primary) || version.files[0];
  if (!file) {
    btnEl.textContent = '✗ Нет файла';
    return;
  }

  btnEl.setAttribute('data-filename', file.filename);
  btnEl.textContent = '0%';

  const r = await api.modrinthDownload({ url: file.url, filename: file.filename, type: mrCurrentType });
  if (r.ok) {
    btnEl.textContent = '✓ Установлено';
    refreshInstalledList();
  } else {
    btnEl.textContent = '✗ Ошибка';
    setTimeout(() => { btnEl.textContent = '⬇ Скачать'; btnEl.disabled = false; }, 2000);
  }
}

async function refreshInstalledList() {
  const list = $('mr-installed');
  const r = await api.modrinthInstalled({ type: mrCurrentType });
  if (!r.ok || !r.files?.length) {
    list.innerHTML = `<div style="font-family:'Share Tech Mono',monospace;font-size:10px;color:var(--muted)">Ничего не установлено</div>`;
    return;
  }
  list.innerHTML = '';
  r.files.forEach(filename => {
    const item = document.createElement('div');
    item.className = 'mr-installed-item';
    item.innerHTML = `<span>${escapeHtml(filename)}</span><span class="mr-remove-btn">✕</span>`;
    item.querySelector('.mr-remove-btn').onclick = async () => {
      await api.modrinthRemove({ filename, type: mrCurrentType });
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
