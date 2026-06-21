// ══════════════════════════════════════════════
//  НАСТРОЙКИ — редактируй только этот файл
// ══════════════════════════════════════════════
module.exports = {
  SERVER_NAME:   'ViPok server',
  SERVER_IP:     'subjects-emirates.gl.joinmc.link',
  SERVER_PORT:   25565,
  MC_VERSION:    '1.20.1',

  // GitHub репо с модами (manifest.json, hashes.json, NEWS.json, папки mods/ config/)
  // ПИШИ ТОЛЬКО НАЗВАНИЕ, БЕЗ ССЫЛОК!
  MODS_OWNER:  'ViPok137',
  MODS_REPO:   'ViLauncher',
  MODS_BRANCH: 'main',

  // GitHub репо с релизами лаунчера
  LAUNCHER_OWNER: 'ViPok137',
  LAUNCHER_REPO:  'ViLauncher',

  DEFAULT_USERNAME: 'Player',

  // ТЗ 3.3: Discord Rich Presence — Client ID с https://discord.com/developers/applications
  DISCORD_CLIENT_ID: '1518298426725372004',

  // ПРИМЕЧАНИЕ (ТЗ 2.2, 2.5):
  // JAVA_PATH больше не используется — лаунчер всегда использует изолированную
  // портативную Java 17 в %APPDATA%/.mc-launcher/runtime/java17/, системная Java игнорируется.
  // RAM_MAX/RAM_MIN больше не задаются здесь — память подбирается автоматически (autoRam())
  // или настраивается игроком вручную через UI (сохраняется в settings.json).
};