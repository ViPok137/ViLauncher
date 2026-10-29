// Сборка фронтенда: мост к Tauri (ES-модуль на @tauri-apps/api → один IIFE-файл) и локальные шрифты.
// Запускается автоматически перед `tauri dev` / `tauri build` (beforeDevCommand / beforeBuildCommand).
// Результат: src/renderer/vendor/{bridge.js, fonts.css, fonts/*.woff2} — в git не хранится.
import { build } from 'esbuild';
import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'src', 'renderer', 'vendor');
rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, 'fonts'), { recursive: true });

await build({
  entryPoints: [join(root, 'src', 'renderer', 'tauri-bridge.src.js')],
  outfile: join(out, 'bridge.js'),
  bundle: true,
  format: 'iife',
  target: 'es2020',
  legalComments: 'none',
  logLevel: 'warning',
});

// Шрифты лежат внутри приложения: без обращений к Google Fonts (работает офлайн, строгий CSP: font-src 'self').
const fonts = [
  ['orbitron', 'Orbitron', [400, 700, 900]],
  ['share-tech-mono', 'Share Tech Mono', [400]],
  ['rajdhani', 'Rajdhani', [400, 600]],
];
let css = '/* Сгенерировано scripts/build-frontend.mjs (@fontsource, лицензия OFL) */\n';
for (const [pkg, family, weights] of fonts) {
  for (const w of weights) {
    const name = `${pkg}-latin-${w}-normal.woff2`;
    const src = join(root, 'node_modules', '@fontsource', pkg, 'files', name);
    if (!existsSync(src)) throw new Error(`Нет файла шрифта ${src} — выполни npm ci`);
    copyFileSync(src, join(out, 'fonts', name));
    css += `@font-face{font-family:'${family}';font-style:normal;font-weight:${w};font-display:swap;src:url('fonts/${name}') format('woff2');}\n`;
  }
}
writeFileSync(join(out, 'fonts.css'), css);
console.log('[build-frontend] готово →', out);
