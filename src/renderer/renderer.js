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
  const uname   = document.getElementById('sb-uname');
  const skinDef = document.getElementById('sb-skin-default');
  const skinCanvas = document.getElementById('sb-skin-canvas');

  if (uname) uname.textContent = username.toUpperCase();
  if (skinDef) skinDef.textContent = (username[0] || '?').toUpperCase();

  try {
    const { skinUrl } = await api.skinGet(username);
    if (!skinUrl || !skinCanvas) return;

    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => {
      // Рисуем увеличенную голову (лицо 8x8 + оверлей из 8x8)
      const ctx = skinCanvas.getContext('2d');
      ctx.imageSmoothingEnabled = false;
      skinCanvas.width  = 36;
      skinCanvas.height = 36;
      // Масштаб: пиксели скина (64x64) → 1 пиксель скина = 4.5 пикселя canvas
      const scale = 36 / 8;
      // Лицо: x=8,y=8,w=8,h=8 в текстуре (первый слой)
      ctx.drawImage(img, 8, 8, 8, 8, 0, 0, 36, 36);
      // Оверлей головы: x=40,y=8,w=8,h=8 (второй слой / шлем)
      ctx.drawImage(img, 40, 8, 8, 8, 0, 0, 36, 36);
      skinCanvas.style.display = 'block';
      if (skinDef) skinDef.style.display = 'none';
    };
    img.onerror = () => {};
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
    if (el.dataset.p === 'news')     loadNews();
    if (el.dataset.p === 'settings') fillSettings();
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

const nickInput = $('nick');
nickInput.oninput = () => {
  const nick = nickInput.value.trim();

  if (!nick) {
    nickError('');
  } else if (nick.length < 3) {
    nickError('Минимум 3 символа');
  } else if (nick.length > 16) {
    nickError('Максимум 16 символов');
  } else if (!NICK_RE.test(nick)) {
    nickError('Только латиница, цифры и _');
  } else {
    nickError('');
    api.setNick(nick);
  }

  updatePlayBtn();
};

// ─── STATE ───────────────────────────────────────────────────────────────────
let mcInstalled  = false;
let modsReady    = false;
let isInstalling = false;

function updatePlayBtn() {
  const nick = nickInput.value.trim();
  $('btn-play').disabled = !isNickValid(nick) || !mcInstalled || !modsReady || isInstalling;
}

// ─── INIT ────────────────────────────────────────────────────────────────────
(async () => {
  // Сначала инициализируем логин
  await initLogin();

  // Никнейм всегда = имени профиля (логину)
  const currentUser = await api.authGetUser();
  if (currentUser) {
    nickInput.value = currentUser;
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
  const nick = nickInput.value.trim();
  if (!isNickValid(nick)) return;

  $('btn-play').disabled = true;
  $('btn-play').textContent = '⏳ ЗАПУСК...';

  const r = await api.launch({ nickname: nick });

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
      dot.className = 'sv-dot offline';
      txt.className = 'sv-status-txt offline';
      const tabDotOff = $('sv-tab-dot-' + currentServer);
      if (tabDotOff) tabDotOff.className = 'sv-tab-dot offline';
      txt.textContent = 'Сервер недоступен';
    }
  } catch {
    dot.className = 'sv-dot offline';
    txt.className = 'sv-status-txt offline';
    txt.textContent = 'Ошибка проверки';
  }
}

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
  items.forEach(it => {
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
