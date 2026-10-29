// Проверка конфигурации безопасности и согласованности проекта. Запуск: npm run check:capabilities
// В CI выполняется перед сборкой; с флагом --release дополнительно требует настоящий публичный ключ обновлений.
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = p => readFileSync(join(root, p), 'utf8');
const json = p => JSON.parse(read(p));
const release = process.argv.includes('--release');
const tagArg = (() => { const i = process.argv.indexOf('--tag'); return i > 0 ? process.argv[i + 1] : null; })();
const errors = [], warns = [];
const must = (cond, msg) => { if (!cond) errors.push(msg); };

// ── tauri.conf.json ──
const conf = json('src-tauri/tauri.conf.json');
must(conf.app?.withGlobalTauri === false, 'app.withGlobalTauri должен быть false (мост использует @tauri-apps/api)');
const csp = conf.app?.security?.csp;
must(typeof csp === 'string' && csp.length > 20, 'app.security.csp не задан (null)');
if (typeof csp === 'string') {
  const dir = n => (csp.split(';').map(x => x.trim()).find(x => x.startsWith(n + ' ')) || '');
  must(!/'unsafe-eval'/.test(csp), "CSP: запрещён 'unsafe-eval'");
  must(!/'unsafe-inline'/.test(dir('script-src')), "CSP: в script-src запрещён 'unsafe-inline'");
  must(/script-src 'self'/.test(csp) && /default-src 'self'/.test(csp), "CSP: нужны default-src 'self' и script-src 'self'");
  must(/object-src 'none'/.test(csp), "CSP: нужен object-src 'none'");
  must(/connect-src[^;]*ipc:/.test(csp), 'CSP: connect-src должен разрешать ipc: (иначе не работают команды)');
  must(!/https?:\/\/[^\s;]*\*/.test(csp) && !/(^|[ ;])\*($|[ ;])/.test(csp), 'CSP: никаких маскарадных источников (*)');
  must(!/fonts\.googleapis|fonts\.gstatic/.test(csp), 'CSP: шрифты должны быть локальными');
}
must(conf.bundle?.createUpdaterArtifacts === true, 'bundle.createUpdaterArtifacts должен быть true');
must((conf.bundle?.targets || []).includes('nsis'), 'bundle.targets должен включать nsis');
const up = conf.plugins?.updater;
must(!!up, 'plugins.updater не настроен');
must(Array.isArray(up?.endpoints) && up.endpoints.length > 0 && up.endpoints.every(e => e.startsWith('https://')), 'updater.endpoints: нужен минимум один https-адрес');
const placeholder = !up?.pubkey || /ВСТАВЬ|REPLACE/i.test(up.pubkey);
if (placeholder) (release ? errors : warns).push('updater.pubkey — заглушка: сгенерируй ключи (npx tauri signer generate) и вставь содержимое .pub файла');
else must(/^[A-Za-z0-9+/=]{60,}$/.test(up.pubkey.trim()), 'updater.pubkey выглядит не как содержимое .pub файла (одна строка base64)');

// ── capabilities ──
const cap = json('src-tauri/capabilities/default.json');
for (const p of cap.permissions) {
  must(p !== 'core:default' && !p.endsWith(':default'), `capabilities: набор ${p} слишком широкий — перечисли нужные права явно`);
  must(!/shell|fs:|http:|opener|process|os:|dialog/.test(p), `capabilities: лишнее право ${p}`);
}
must(cap.windows?.length === 1 && cap.windows[0] === 'main', 'capabilities: права только для окна main');
must(!cap.remote, 'capabilities: remote-доступ запрещён');

// ── мост ↔ команды Rust ──
const main = read('src-tauri/src/main.rs');
const registered = new Set([...main.matchAll(/commands::([a-z_0-9]+),/g)].map(m => m[1]));
const bridge = read('src/renderer/tauri-bridge.src.js');
const called = new Set([...bridge.matchAll(/call\('([a-z_0-9]+)'/g)].map(m => m[1]));
for (const c of called) must(registered.has(c), `мост вызывает команду ${c}, которой нет в generate_handler!`);
for (const c of registered) if (!called.has(c)) warns.push(`команда ${c} зарегистрирована, но мост её не вызывает`);

// ── фронтенд без eval и внешних ресурсов ──
for (const f of ['src/renderer/renderer.js', 'src/renderer/game-settings.js', 'src/renderer/tauri-bridge.src.js']) {
  const t = read(f);
  must(!/\beval\s*\(|new Function\s*\(|document\.write\s*\(/.test(t), `${f}: eval/new Function/document.write запрещены`);
}
const html = read('src/renderer/index.html');
must(!/<script(?![^>]*\bsrc=)[^>]*>\s*\S/.test(html), 'index.html: inline-скрипты запрещены CSP');
must(!/\son(click|change|input|load|error)=/i.test(html), 'index.html: inline-обработчики событий запрещены CSP');
must(!/https?:\/\/(fonts|cdn|unpkg|cdnjs)\./.test(html), 'index.html: внешние ресурсы запрещены (шрифты и библиотеки локальные)');

// ── версии: tauri.conf.json = Cargo.toml = тег релиза (иначе updater не увидит новую версию) ──
const cargoVer = (read('src-tauri/Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m) || [])[1];
must(conf.version === cargoVer, `версии не совпадают: tauri.conf.json ${conf.version} ≠ src-tauri/Cargo.toml ${cargoVer}`);
if (tagArg) must(tagArg.replace(/^v/, '') === conf.version, `тег ${tagArg} не совпадает с версией приложения ${conf.version} — обнови версию в tauri.conf.json и Cargo.toml`);

// ── прочее ──
const pkg = json('package.json');
must(!('electron' in (pkg.devDependencies || {})) && !('electron-builder' in (pkg.devDependencies || {})), 'package.json: Electron больше не нужен');
const servers = json('servers.json');
must(servers.servers.filter(s => s.active).map(s => s.id).join() === 'create' || process.argv.includes('--any-active'), 'servers.json: сейчас активным должен быть только create');

for (const w of warns) console.log('⚠ ', w);
if (errors.length) { for (const e of errors) console.log('✗ ', e); process.exit(1); }
console.log(`✓ Конфигурация в порядке (команд: ${registered.size}${warns.length ? `, предупреждений: ${warns.length}` : ''})`);
