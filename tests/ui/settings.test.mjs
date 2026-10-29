import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { closeAll, loadLauncher, sleep } from './helpers.mjs';

// Объекты из окна jsdom живут в другом «мире» — сравниваем по JSON.
const plain = v => JSON.parse(JSON.stringify(v));

afterEach(closeAll);

const OPTIONS = [
  { key: 'fov', value: '0.0' }, { key: 'renderDistance', value: '12' }, { key: 'lang', value: 'en_us' },
  { key: 'fullscreen', value: 'false' }, { key: 'key_key.forward', value: 'key.keyboard.w' },
];

async function openPanel(overrides) {
  const ctx = await loadLauncher(overrides);
  ctx.d.getElementById('sv-tools-toggle').click();
  await sleep(40);
  return ctx;
}

test('панель сервера свёрнута, открывается кликом, Modrinth инициализируется один раз', async () => {
  const { d, callsOf } = await loadLauncher();
  assert.equal(d.getElementById('sv-tools-content').hidden, true);
  d.getElementById('sv-tools-toggle').click(); await sleep(60);
  assert.equal(d.getElementById('sv-tools-content').hidden, false);
  const n = callsOf('modrinthSearch').length;
  d.getElementById('sv-tools-toggle').click(); d.getElementById('sv-tools-toggle').click(); await sleep(60);
  assert.equal(callsOf('modrinthSearch').length, n, 'повторное открытие не делает лишних запросов');
});

test('настройки Minecraft: значения из options.txt, сохраняются только изменённые', async () => {
  const sets = [];
  const { d, callsOf } = await openPanel({
    gameOptionsGet: { ok: true, exists: true, options: OPTIONS, running: false },
    gameOptionsSet: ch => { sets.push(ch); return { ok: true, saved: Object.keys(ch).length }; },
    defaultsReset: { ok: true, restored: 1 },
  });
  d.querySelector('[data-svtab="mc"]').click(); await sleep(80);
  const rows = [...d.querySelectorAll('#gs-mc-grid .gs-row')];
  assert.equal(rows.length, 15);
  const row = t => rows.find(r => r.textContent.includes(t));
  assert.equal(row('FOV').querySelector('input').value, '70');
  assert.equal(row('Язык').querySelector('select').value, 'en_us');
  assert.match(d.getElementById('gs-mc-adv').textContent, /key_key\.forward/, 'остальные параметры (управление) — в расширенном списке');

  const fov = row('FOV').querySelector('input'); fov.value = '90'; fov.dispatchEvent(new d.defaultView.Event('input'));
  const fs = row('Полный экран').querySelector('input'); fs.checked = true; fs.dispatchEvent(new d.defaultView.Event('input'));
  d.getElementById('gs-mc-save').click(); await sleep(60);
  assert.deepEqual(plain(sets[0]), { fov: '0.5', fullscreen: 'true' });

  d.getElementById('gs-mc-reset').click(); await sleep(60);
  assert.deepEqual(plain(callsOf('defaultsReset')[0][1]), { path: 'options.txt' }, '«вернуть по умолчанию» → defaults.json');
});

test('настройки модов: список, правка, ошибка синтаксиса → сохранить без проверки, сброс файла', async () => {
  const writes = [], resets = [];
  const { d } = await openPanel({
    modConfigsList: { ok: true, files: [{ path: 'config/create-common.toml', size: 5 }, { path: 'config/a/b.json', size: 2 }], resettable: ['config/create-common.toml'], running: false },
    modConfigRead: p => ({ ok: true, text: p.endsWith('toml') ? 'a = 1\n' : '{}' }),
    modConfigWrite: x => { writes.push(x); return x.force ? { ok: true } : x.text.includes('BAD') ? { ok: false, error: 'Проверка синтаксиса не пройдена: x', validation: true } : { ok: true }; },
    defaultsReset: x => { resets.push(x); return { ok: true, restored: 1 }; },
  });
  d.querySelector('[data-svtab="mods"]').click(); await sleep(80);
  const items = [...d.querySelectorAll('.gs-cfg-item')];
  assert.deepEqual(items.map(i => i.textContent), ['create-common.toml', 'a/b.json']);
  items[0].click(); await sleep(60);
  const ta = d.getElementById('gs-cfg-text');
  assert.equal(ta.value, 'a = 1\n');
  assert.equal(d.getElementById('gs-cfg-reset').disabled, false, 'сброс доступен: файл есть в defaults.json');
  ta.value = 'a = BAD'; ta.dispatchEvent(new d.defaultView.Event('input'));
  d.getElementById('gs-cfg-save').click(); await sleep(60);
  assert.equal(d.getElementById('gs-cfg-force').hidden, false);
  d.getElementById('gs-cfg-force').click(); await sleep(60);
  assert.equal(writes.at(-1).force, true);
  d.getElementById('gs-cfg-reset').click(); await sleep(80);
  assert.deepEqual(plain(resets.at(-1)), { path: 'config/create-common.toml' });
  items[1].click(); await sleep(60);
  assert.equal(d.getElementById('gs-cfg-reset').disabled, true, 'файл без версии по умолчанию сбросить нельзя');
  d.getElementById('gs-cfg-resetall').click(); await sleep(80);
  assert.equal(resets.at(-1).path, undefined, '«вернуть ВСЕ» — без пути');
});

test('во время игры сохранение блокируется; античит показывает окно предупреждения', async () => {
  const { d, cbs } = await openPanel({ gameOptionsGet: { ok: true, exists: true, options: OPTIONS, running: false } });
  d.querySelector('[data-svtab="mc"]').click(); await sleep(80);
  cbs.onMcRunning({ running: true }); await sleep(20);
  assert.equal(d.getElementById('gs-mc-save').disabled, true);
  assert.equal(d.getElementById('gs-mc-busy').hidden, false);
  cbs.onAnticheatAlert({ message: 'Игра закрыта: обнаружены запрещённые файлы\n\nЗапрещённые файлы (читы):\n• wurst.jar\n\nСоединение с сервером прервано.' });
  await sleep(20);
  const ov = d.querySelector('.ac-overlay');
  assert.ok(ov && ov.textContent.includes('wurst.jar'));
  ov.querySelector('button').click();
  assert.equal(d.querySelector('.ac-overlay'), null);
});
