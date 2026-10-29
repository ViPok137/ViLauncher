/* ViLauncher — мост window.api для Tauri (ES-модуль, собирается esbuild в vendor/bridge.js).
 * Использует штатный @tauri-apps/api, глобальный window.__TAURI__ не нужен (withGlobalTauri: false).
 * Контракт методов и событий сохранён, поэтому renderer.js почти не менялся. */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

(function () {
  'use strict';
  // Внутри окна Tauri всегда есть __TAURI_INTERNALS__; в обычном браузере (тесты, предпросмотр) мост не включается.
  if (!window.__TAURI_INTERNALS__ || window.api) return;

  const appWindow = getCurrentWindow();

  const on = (event, map) => cb => { listen(event, e => cb(map ? map(e.payload) : e.payload)); };
  const call = (cmd, args) => invoke(cmd, args || {});

  window.api = {
    // окно
    minimize: () => appWindow.minimize(),
    maximize: () => appWindow.toggleMaximize(),
    close:    () => appWindow.close(),

    getNick: () => call('nick_get'),
    setNick: nick => call('nick_set', { nick }),
    fetchNews: () => call('news_fetch'),

    // установка / Java (модуль 3)
    installCheck:  () => call('install_check'),
    installStart:  () => call('install_start'),
    onJavaProg:    on('java-progress'),
    onInstallProg: on('install-progress'),

    // моды (модуль 1). В Tauri `fileIndex` = число уже завершённых файлов (загрузка параллельная).
    modsSync:     () => call('mods_sync', { force: false }),
    modsCancel:   () => call('mods_cancel'),
    onModsProg:   on('mods-progress', p => Object.assign({}, p, { fileIndex: p.completed })),
    onModsStatus: on('mods-status'),

    // запуск
    // сервер, адрес и instance определяет лаунчер по выбранному серверу (servers.json)
    launch: d => call('game_launch', { nickname: d && d.nickname }),
    onGameError: on('game-error'),
    onMcRunning: on('mc-running'),

    // авторизация: заглушка входа, как в оригинале (пароль «Запомнить меня» — в системном хранилище)
    authLogin:    d => call('auth_login', { username: d && d.username, password: d && d.password, remember: d && d.remember }),
    authLogout:   () => call('auth_logout'),
    authGetSaved: () => call('auth_get_saved'),
    authGetUser:  () => call('auth_get_user'),

    // Microsoft (опционально, в интерфейсе не используется)
    msLogin:   () => call('ms_login'),
    msCancel:  () => call('ms_cancel'),
    msRestore: () => call('ms_restore'),
    msLogout:  () => call('ms_logout'),
    onMsCode:  on('ms-device-code'),

    // RAM / настройки / dev
    ramGet: () => call('ram_get'),
    ramSet: d => call('ram_set', { maxG: d.maxG, minG: d.minG }),
    getAppInfo: () => call('app_info'),
    settingSet: d => call('setting_set', { key: d.key, value: d.value }),
    reinstallClient: () => call('reinstall_client'),
    checkModsForce:  () => call('check_mods'),
    openLauncherDir: () => call('open_launcher_dir'),
    openUrl: url => call('open_url', { url }),
    devAuth:    d => call('dev_auth', { code: d.code }),
    devFlagSet: d => call('dev_flag_set', { flag: d.flag, value: d.value }),
    devStatus:  () => call('dev_status'),

    // сервер / обновления
    serverPing:     serverId => call('server_ping', { serverId: serverId || null }),
    // Обновления лаунчера: tauri-plugin-updater (подписанные релизы GitHub), перезапуск после установки
    launcherCheck:  () => call('updater_check'),
    launcherUpdate: () => call('updater_install'),
    onLauncherDl:   on('launcher-dl-progress'),

    // Серверы и instance'ы
    listServers:       () => call('list_servers'),
    selectServer:      id => call('select_server', { id }),
    getInstanceStatus: id => call('get_instance_status', { id: id || null }),
    refreshServers:    () => call('refresh_servers'),
    onServersChanged:  on('servers-changed'),

    // скины
    skinGet:    username => call('skin_get', { username }),
    skinUpload: () => call('skin_upload'),
    skinReset:  () => call('skin_reset'),

    // Modrinth (шейдеры / ресурспаки)
    modrinthSearch:    d => call('modrinth_search', { data: d }),
    modrinthVersions:  d => call('modrinth_versions', { data: d }),
    modrinthDownload:  d => call('modrinth_download', { data: d }),
    modrinthInstalled: d => call('modrinth_installed', { data: d }),
    modrinthActive:    d => call('modrinth_active', { data: d }),
    modrinthToggle:    d => call('modrinth_toggle', { data: d }),
    modrinthRemove:    d => call('modrinth_remove', { data: d }),
    onModrinthDlProg:  on('modrinth-dl-progress'),

    // Discord Rich Presence / античит
    discordSetPlaying:  d => call('discord_set_playing', { playerCount: d && d.playerCount }),
    discordSetLauncher: () => call('discord_set_launcher'),
    scanCheats: () => call('scan_cheats'),
    onAnticheatAlert: on('anticheat-alert'),

    // Настройки игрока: options.txt, конфиги модов, «вернуть по умолчанию»
    gameOptionsGet: () => call('game_options_get'),
    gameOptionsSet: changes => call('game_options_set', { changes }),
    modConfigsList: () => call('mod_configs_list'),
    modConfigRead:  path => call('mod_config_read', { path }),
    modConfigWrite: d => call('mod_config_write', { path: d.path, text: d.text, force: !!d.force }),
    defaultsReset:  d => call('defaults_reset', { path: (d && d.path) || null }),
  };

  document.addEventListener('DOMContentLoaded', () => {
    // Перетаскивание окна за заголовок (замена -webkit-app-region: drag).
    const tb = document.getElementById('tb');
    if (tb) {
      tb.addEventListener('mousedown', e => {
        if (e.button === 0 && !e.target.closest('.wc')) appWindow.startDragging();
      });
    }
  });
})();
