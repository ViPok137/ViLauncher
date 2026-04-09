'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('api', {
  // window
  minimize: () => ipcRenderer.send('win-min'),
  maximize: () => ipcRenderer.send('win-max'),
  close:    () => ipcRenderer.send('win-close'),

  // nickname
  getNick: ()     => ipcRenderer.invoke('nick-get'),
  setNick: nick   => ipcRenderer.invoke('nick-set', nick),

  // news
  fetchNews: () => ipcRenderer.invoke('news-fetch'),

  // launcher self-update
  launcherCheck:  ()    => ipcRenderer.invoke('launcher-check'),
  launcherUpdate: data  => ipcRenderer.invoke('launcher-update', data),
  onLauncherDl:   cb    => ipcRenderer.on('launcher-dl-progress', (_, d) => cb(d)),

  // mods
  modsSync:    () => ipcRenderer.invoke('mods-sync'),
  onModsProg:  cb => ipcRenderer.on('mods-progress', (_, d) => cb(d)),

  // launch
  launch: data => ipcRenderer.invoke('game-launch', data),
});
