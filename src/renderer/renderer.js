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
  const saved = await api.getNick();
  if (saved) {
    nickInput.value = saved;
    if (!isNickValid(saved)) nickError('Никнейм некорректен — исправь перед игрой');
  }

  checkLauncherUpdate();
  fillSettings();
  pingServer();      // проверяем статус сервера сразу
  setInterval(pingServer, 30000); // и каждые 30 сек

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
      txt.textContent = `Онлайн · ${r.ping} мс`;
    } else {
      dot.className = 'sv-dot offline';
      txt.className = 'sv-status-txt offline';
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

  // Версия в сайдбаре
  if ($('ver-txt')) $('ver-txt').textContent = ver;

  // Страница настроек
  if ($('s-lver'))  $('s-lver').textContent  = ver;
  if ($('s-mver'))  $('s-mver').textContent  = info.modsVersion || 'Не установлены';
  if ($('s-elec'))  $('s-elec').textContent  = info.electron    || '—';
  if ($('s-os'))    $('s-os').textContent     = info.os          || '—';
  if ($('s-java'))  $('s-java').textContent   = info.java        || '—';
}
