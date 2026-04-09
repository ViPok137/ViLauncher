'use strict';
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
    if (el.dataset.p === 'news') loadNews();
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
  const saved = await api.getNick();
  if (saved) {
    nickInput.value = saved;
    if (!isNickValid(saved)) nickError('Никнейм некорректен — исправь перед игрой');
  }

  checkLauncherUpdate();

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

async function runModsSync() {
  if (modsSyncRunning) return;   // защита от двойного запуска
  modsSyncRunning = true;
  modsReady = false;
  updatePlayBtn();

  const bar = $('mbar'), st = $('mst'), fl = $('mfile');
  st.className   = 'mods-st chk';
  st.textContent = '⏳ Проверяю моды...';
  bar.style.width = '0%';
  fl.textContent  = '';

  api.onModsProg(({ done, total, name }) => {
    bar.style.width = (total > 0 ? Math.round(done / total * 100) : 0) + '%';
    fl.textContent  = name;
    st.textContent  = `⬇ Загрузка ${done}/${total}...`;
  });

  let r;
  try {
    r = await api.modsSync();
  } catch (err) {
    r = { ok: false, error: err.message || 'Неизвестная ошибка' };
  }

  bar.style.width = '100%';
  fl.textContent  = '';
  modsReady = true;
  modsSyncRunning = false;

  if (r.ok) {
    st.className   = 'mods-st ok';
    st.textContent = r.updated
      ? `✓ Моды обновлены (${r.version})`
      : `✓ ${r.msg}`;
  } else {
    st.className   = 'mods-st err';
    const err = r.error || 'Ошибка синхронизации';
    // Подсказка если manifest.json ещё не выложен
    const hint = /HTTP 4/.test(err) ? ' (manifest.json не найден на GitHub)' : '';
    st.textContent = '✗ ' + err + hint;
    // Кнопка всё равно разблокируется — можно играть без обновления модов
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
$('btn-play').onclick = async () => {
  const nick = nickInput.value.trim();
  if (!isNickValid(nick)) return; // двойная защита

  $('btn-play').disabled = true;
  $('btn-play').textContent = '⏳ ЗАПУСК...';

  const r = await api.launch({ nickname: nick });

  setTimeout(() => {
    $('btn-play').disabled = false;
    $('btn-play').textContent = '▶ ЗАПУСТИТЬ';
    updatePlayBtn();
  }, 4000);

  if (!r.ok) {
    $('mst').className = 'mods-st err';
    $('mst').textContent = '✗ ' + (r.error || 'Ошибка запуска');
  }
};

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
