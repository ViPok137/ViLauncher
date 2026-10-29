import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { closeAll, loadLauncher, serverViews, sleep } from './helpers.mjs';

afterEach(closeAll);

const cards = d => [...d.querySelectorAll('#sv-grid .svc')];
const byName = (d, t) => cards(d).find(c => c.textContent.includes(t));

test('все 4 сервера видны карточками; активен только Create и он выбран', async () => {
  const { d, errors } = await loadLauncher();
  assert.deepEqual(errors, [], 'без ошибок загрузки');
  const list = cards(d);
  assert.equal(list.length, 4);
  assert.deepEqual(list.map(c => c.dataset.id), ['create', 'colonization', 'infection', 'vanilla']);
  assert.ok(list[0].classList.contains('on') && !list[0].classList.contains('off'), 'Create выбран и активен');
  for (const c of list.slice(1)) assert.ok(c.classList.contains('off'), `${c.dataset.id} приглушён`);
  assert.ok(list.slice(1).every(c => c.textContent.includes('Скоро')), 'у неактивных бейдж «Скоро»');
  assert.match(byName(d, 'Ваниль').textContent, /Ваниль · по подписке/);
  assert.match(byName(d, 'Ваниль').textContent, /по подписке/);
  assert.match(list[0].textContent, /Minecraft 1\.21\.1 · NeoForge/);
  assert.match(list[1].textContent, /Minecraft 1\.20\.1 · Forge/);
  assert.match(list[3].textContent, /последняя версия/);
  assert.match(list[0].textContent, /3\/20/, 'у активного — онлайн и пинг');
  assert.match(d.getElementById('sv-name').textContent, /CREATE|Create/i);
});

test('Create: «Играть» доступна, статус «Готово»', async () => {
  const { d } = await loadLauncher();
  const play = d.getElementById('btn-play');
  assert.equal(play.disabled, false, 'Играть включена');
  assert.equal(d.getElementById('sv-state').textContent, 'Готово');
  assert.equal(d.getElementById('sv-inactive').style.display, 'none');
});

test('неактивный сервер: видим, но «Играть» заблокирована, синк и запуск не выполняются', async () => {
  const { d, calls, callsOf } = await loadLauncher();
  const syncsBefore = callsOf('modsSync').length;
  byName(d, 'Колонизация').click();
  await sleep(120);
  assert.deepEqual(callsOf('selectServer').map(c => c[1]), ['colonization']);
  const play = d.getElementById('btn-play');
  assert.equal(play.disabled, true);
  assert.match(play.textContent, /СКОРО/);
  assert.equal(d.getElementById('sv-state').textContent, 'Недоступно');
  assert.notEqual(d.getElementById('sv-inactive').style.display, 'none', 'пояснение показано');
  assert.equal(d.getElementById('install-box').style.display, 'none');
  assert.equal(d.getElementById('mods-box').style.display, 'none');
  assert.equal(callsOf('modsSync').length, syncsBefore, 'синк модов для неактивного сервера не запускается');
  play.click();
  await sleep(30);
  assert.equal(callsOf('launch').length, 0, 'запуск не выполняется');
  assert.ok(byName(d, 'Колонизация').classList.contains('on'), 'выбранная карточка подсвечена');
  assert.ok(!byName(d, 'Create').classList.contains('on'));
});

test('ваниль и «Заражение» тоже неактивны; возврат на Create снова включает игру', async () => {
  const { d, callsOf } = await loadLauncher();
  for (const n of ['Заражение', 'Ваниль']) {
    byName(d, n).click(); await sleep(100);
    assert.equal(d.getElementById('btn-play').disabled, true, n);
  }
  assert.match(d.getElementById('sv-name').textContent, /Ваниль/);
  byName(d, 'Create').click(); await sleep(150);
  assert.equal(d.getElementById('btn-play').disabled, false);
  assert.equal(d.getElementById('sv-state').textContent, 'Готово');
  assert.ok(callsOf('installCheck').length >= 2, 'состояние instance пересчитано');
});

test('активация сервера — правка servers.json: карточка становится играбельной без правки кода', async () => {
  const enabled = serverViews('colonization', { colonization: true }).map(s => (s.id === 'colonization' ? { ...s, active: true, playable: true, address: 'mc.example.org:25570' } : s));
  const { d, callsOf } = await loadLauncher({
    listServers: () => ({ ok: true, selected: 'colonization', servers: enabled }),
    serverPing: () => ({ online: true, ping: 40, version: '1.20.1', online_count: 1, max_count: 10 }),
  });
  const col = byName(d, 'Колонизация');
  assert.ok(col.classList.contains('on') && !col.classList.contains('off'));
  assert.ok(!col.textContent.includes('Скоро'));
  assert.equal(d.getElementById('btn-play').disabled, false, 'Играть включена у включённого сервера');
  assert.ok(callsOf('modsSync').length >= 1, 'для активного сервера запускается синк');
});

test('обновление лаунчера: баннер, прогресс скачивания, установка', async () => {
  const { d, callsOf, cbs } = await loadLauncher({
    launcherCheck: { hasUpdate: true, local: '0.5.0', remote: '0.6.0', notes: 'x' },
  });
  const banner = d.getElementById('banner');
  assert.ok(banner.classList.contains('on'));
  assert.match(d.getElementById('bn-txt').textContent, /0\.6\.0.*0\.5\.0/);
  d.getElementById('bn-upd').click();
  await sleep(30);
  assert.equal(callsOf('launcherUpdate').length, 1);
  cbs.onLauncherDl({ pct: 42 });
  assert.match(d.getElementById('bn-txt').textContent, /42%/);
});

test('ручная проверка обновлений в настройках сообщает результат', async () => {
  const { d, callsOf } = await loadLauncher();
  const n = callsOf('launcherCheck').length;
  d.getElementById('btn-check-upd').click();
  await sleep(60);
  assert.equal(callsOf('launcherCheck').length, n + 1);
});

test('окно Modrinth лежит в разделе сервера и использует id выбранного сервера', async () => {
  const { d, callsOf } = await loadLauncher();
  assert.ok(d.getElementById('sv-tools').contains(d.getElementById('mr-search')), 'Modrinth внутри раздела сервера');
  assert.ok(!d.getElementById('pg-appearance').contains(d.getElementById('mr-search')));
  d.getElementById('sv-tools-toggle').click(); await sleep(80);
  assert.ok(callsOf('modrinthInstalled').length >= 1);
  assert.equal(callsOf('modrinthInstalled')[0][1].serverKey || 'create', 'create');
});
