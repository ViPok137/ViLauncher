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
    if (el.dataset.p === 'settings') fillSettings();
  };
});

// ─── NICKNAME ────────────────────────────────────────────────────────────────
const nickInput = $('nick');
nickInput.oninput = () => {
  const v = nickInput.value.trim();
  api.setNick(v);
  $('btn-play').disabled = !v || modsBlocked;
};

// ─── INIT ─────────────────────────────────────────────────────────────────────
let modsBlocked = true;

(async () => {
  // Загружаем сохранённый никнейм
  const saved = await api.getNick();
  if (saved) nickInput.value = saved;

  // Параллельно: проверяем апдейт лаунчера + грузим моды
  checkLauncherUpdate();
  runModsSync();
})();

// ─── LAUNCHER UPDATE ──────────────────────────────────────────────────────────
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
    $('bn-txt').textContent = '❌ Ошибка: ' + r.error;
    $('bn-upd').disabled = false;
  }
};
$('bn-skip').onclick = () => $('banner').classList.remove('on');

// ─── MODS SYNC ───────────────────────────────────────────────────────────────
async function runModsSync() {
  const bar = $('mbar'), st = $('mst'), fl = $('mfile');
  modsBlocked = true;
  $('btn-play').disabled = true;
  st.className = 'mods-st chk';
  st.textContent = '⏳ Проверяю моды...';
  bar.style.width = '0%';

  api.onModsProg(({ done, total, name }) => {
    bar.style.width = (total > 0 ? Math.round(done/total*100) : 0) + '%';
    fl.textContent = name;
    st.textContent = `⬇ Загрузка ${done}/${total}...`;
  });

  const r = await api.modsSync();
  bar.style.width = '100%';
  fl.textContent = '';

  if (r.ok) {
    st.className = 'mods-st ok';
    st.textContent = r.updated ? `✓ Моды обновлены (${r.version})` : `✓ ${r.msg}`;
  } else {
    st.className = 'mods-st err';
    st.textContent = '✗ Ошибка: ' + (r.error || '');
  }

  modsBlocked = false;
  if (nickInput.value.trim()) $('btn-play').disabled = false;
}

// ─── LAUNCH ──────────────────────────────────────────────────────────────────
$('btn-play').onclick = async () => {
  const nick = nickInput.value.trim();
  if (!nick) return;
  $('btn-play').disabled = true;
  $('btn-play').textContent = '⏳ ЗАПУСК...';

  const r = await api.launch({ nickname: nick });

  setTimeout(() => {
    $('btn-play').disabled = false;
    $('btn-play').textContent = '▶ ЗАПУСТИТЬ';
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
  const list = $('nw-list');
  const items = await api.fetchNews();
  newsLoaded = true;
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

// ─── SETTINGS ─────────────────────────────────────────────────────────────────
function fillSettings() {
  try { $('s-elec').textContent = process.versions?.electron || '—'; } catch {}
  $('s-os').textContent = navigator.platform || '—';
}

function e(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}
