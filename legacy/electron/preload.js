'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('api', {
  minimize: () => ipcRenderer.send('win-min'),
  maximize: () => ipcRenderer.send('win-max'),
  close:    () => ipcRenderer.send('win-close'),

  getNick:  ()     => ipcRenderer.invoke('nick-get'),
  setNick:  nick   => ipcRenderer.invoke('nick-set', nick),

  fetchNews: () => ipcRenderer.invoke('news-fetch'),

  launcherCheck:  ()   => ipcRenderer.invoke('launcher-check'),
  launcherUpdate: data => ipcRenderer.invoke('launcher-update', data),
  onLauncherDl:   cb   => ipcRenderer.on('launcher-dl-progress', (_, d) => cb(d)),

  // Установка Minecraft+Forge
  installCheck:  ()  => ipcRenderer.invoke('install-check'),
  installStart:  ()  => ipcRenderer.invoke('install-start'),
  onJavaProg:    cb  => ipcRenderer.on('java-progress', (_, d) => cb(d)),
  onInstallProg: cb  => ipcRenderer.on('install-progress', (_, d) => cb(d)),

  // Моды
  modsSync:   ()  => ipcRenderer.invoke('mods-sync'),
  onModsProg:   cb => ipcRenderer.on('mods-progress', (_, d) => cb(d)),
  onModsStatus: cb => ipcRenderer.on('mods-status',   (_, d) => cb(d)),

  launch:      data => ipcRenderer.invoke('game-launch', data),
  serverPing:  ()   => ipcRenderer.invoke('server-ping'),
  // Auth
  authLogin:   data => ipcRenderer.invoke('auth-login', data),
  authLogout:  ()   => ipcRenderer.invoke('auth-logout'),
  authGetSaved:()   => ipcRenderer.invoke('auth-get-saved'),
  authGetUser: ()   => ipcRenderer.invoke('auth-get-user'),
  // Skin
  skinGet:     u    => ipcRenderer.invoke('skin-get', u),
  skinUpload:  ()   => ipcRenderer.invoke('skin-upload'),
  skinReset:   ()   => ipcRenderer.invoke('skin-reset'),
  // Modrinth (шейдеры, ресурспаки)
  modrinthSearch:    data => ipcRenderer.invoke('modrinth-search', data),
  modrinthVersions:  data => ipcRenderer.invoke('modrinth-versions', data),
  modrinthDownload:  data => ipcRenderer.invoke('modrinth-download', data),
  modrinthInstalled: data => ipcRenderer.invoke('modrinth-installed', data),
  modrinthActive:    data => ipcRenderer.invoke('modrinth-active', data),
  modrinthToggle:    data => ipcRenderer.invoke('modrinth-toggle', data),
  modrinthRemove:    data => ipcRenderer.invoke('modrinth-remove', data),
  onModrinthDlProg:  cb   => ipcRenderer.on('modrinth-dl-progress', (_, d) => cb(d)),
  // RAM
  ramGet:      ()      => ipcRenderer.invoke('ram-get'),
  ramSet:      data    => ipcRenderer.invoke('ram-set', data),
  getAppInfo:       ()   => ipcRenderer.invoke('app-info'),
  settingSet:  data    => ipcRenderer.invoke('setting-set', data),
  reinstallClient:  ()   => ipcRenderer.invoke('reinstall-client'),
  checkModsForce:   ()   => ipcRenderer.invoke('check-mods'),
  openLauncherDir:  ()   => ipcRenderer.invoke('open-launcher-dir'),
  discordSetPlaying: d   => ipcRenderer.invoke('discord-set-playing', d),
  discordSetLauncher:()  => ipcRenderer.invoke('discord-set-launcher'),
  onGameError:  cb => ipcRenderer.on('game-error',  (_, d) => cb(d)),
  openUrl: url => ipcRenderer.invoke('open-url', url),
  onMcRunning:  cb => ipcRenderer.on('mc-running',   (_, d) => cb(d)),
  // ТЗ 3: панель разработчика
  devAuth:    data => ipcRenderer.invoke('dev-auth',     data),
  devFlagSet: data => ipcRenderer.invoke('dev-flag-set', data),
  devStatus:  ()   => ipcRenderer.invoke('dev-status'),
  // ТЗ 4: античит
  scanCheats: ()   => ipcRenderer.invoke('scan-cheats'),
});
