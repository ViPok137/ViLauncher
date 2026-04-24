; ============================================================
; ViLauncher — Inno Setup Script
; Требует: Inno Setup 6.x с Unicode поддержкой
; Сборка: ISCC.exe launcher_setup.iss
; ============================================================

#define AppName        "ViLauncher"
#define AppVersion     "0.3.4"
#define AppPublisher   "ViPok"
#define AppExeName     "ViLauncher.exe"
#define UpdaterExeName "ViLauncherUpdater.exe"
#define AppURL         "https://github.com/ViPok137/ViLauncher"
#define MySourceDir    "win-unpacked"

; ── Глобальные флаги компиляции ─────────────────────────────
; Стабильный Unicode — UTF-8, совместимо с Win10/11
#pragma option -u+

[Setup]
; Уникальный GUID — НЕ МЕНЯЙ после первого релиза!
AppId={{B7E29F4A-1C3D-4E8B-9A2F-D6C5E0F3B841}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} v{#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases

; ── Пути установки ──────────────────────────────────────────
; %LOCALAPPDATA%\ViLauncher — не требует прав администратора
DefaultDirName={localappdata}\{#AppName}
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; ── Mutex — блокируем установку если лаунчер/апдейтер запущен ──
AppMutex={#AppName}_Running_Mutex_v1

; ── Внешний вид ─────────────────────────────────────────────
WizardStyle=modern
SetupIconFile={#MySourceDir}\resources\app.ico

; ── Вывод ───────────────────────────────────────────────────
OutputDir=installer_output
OutputBaseFilename=ViLauncher_Setup_v{#AppVersion}

; ── Сжатие ──────────────────────────────────────────────────
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes

; ── Удаление ────────────────────────────────────────────────
; Иконка в "Программы и компоненты" — показываем апдейтер
UninstallDisplayIcon={app}\{#UpdaterExeName}
UninstallDisplayName={#AppName}

; ── Версия для Windows ──────────────────────────────────────
MinVersion=10.0

; ── Запись текущей версии для апдейтера ─────────────────────
; Апдейтер читает {localappdata}\ViLauncher\version.txt
; чтобы понять нужно ли скачивать обновление

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Ярлык на рабочий стол — по умолчанию включён
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; \
  GroupDescription: "{cm:AdditionalIcons}"

; Автозапуск с Windows — по умолчанию выключен
Name: "startupicon"; Description: "Запускать при старте Windows"; \
  GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; ── Апдейтер (главный запускаемый файл) ──
; Кладём отдельно чтобы его можно было обновить независимо
Source: "dist\{#UpdaterExeName}"; DestDir: "{app}"; \
  Flags: ignoreversion

; ── Основной лаунчер ──
Source: "{#MySourceDir}\{#AppExeName}"; DestDir: "{app}"; \
  Flags: ignoreversion

; ── Все остальные файлы Electron-приложения ──
Source: "{#MySourceDir}\*"; DestDir: "{app}"; \
  Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
; Папка данных — права текущего пользователя
Name: "{localappdata}\{#AppName}"; Permissions: users-full

[Icons]
; ── Меню Пуск — запускаем через апдейтер ──
Name: "{group}\{#AppName}"; \
  Filename: "{app}\{#UpdaterExeName}"; \
  IconFilename: "{app}\{#AppExeName}"

; ── Поиск Windows ──
Name: "{autoprograms}\{#AppName}"; \
  Filename: "{app}\{#UpdaterExeName}"; \
  IconFilename: "{app}\{#AppExeName}"

; ── Рабочий стол (по задаче) ──
Name: "{autodesktop}\{#AppName}"; \
  Filename: "{app}\{#UpdaterExeName}"; \
  IconFilename: "{app}\{#AppExeName}"; \
  Tasks: desktopicon

; ── Автозагрузка (по задаче) ──
Name: "{userstartup}\{#AppName}"; \
  Filename: "{app}\{#UpdaterExeName}"; \
  IconFilename: "{app}\{#AppExeName}"; \
  Tasks: startupicon

[Registry]
; Регистрация в «Программы и компоненты»
Root: HKCU; Subkey: "Software\{#AppPublisher}\{#AppName}"; \
  ValueType: string; ValueName: "InstallPath"; ValueData: "{app}"; \
  Flags: uninsdeletekey

; Сохраняем версию для апдейтера
Root: HKCU; Subkey: "Software\{#AppPublisher}\{#AppName}"; \
  ValueType: string; ValueName: "Version"; ValueData: "{#AppVersion}"; \
  Flags: uninsdeletekey

[INI]
; Записываем version.txt который читает апдейтер
Filename: "{localappdata}\{#AppName}\version.txt"; \
  Section: ""; Key: ""; String: "{#AppVersion}"

[Run]
; Запуск после установки — через апдейтер (он сам запустит лаунчер)
Filename: "{app}\{#UpdaterExeName}"; \
  Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; \
  Flags: nowait postinstall skipifsilent

[UninstallRun]
; Завершаем ОБА процесса перед удалением
Filename: "taskkill.exe"; Parameters: "/F /IM {#AppExeName}"; \
  Flags: runhidden skipifdoesntexist
Filename: "taskkill.exe"; Parameters: "/F /IM {#UpdaterExeName}"; \
  Flags: runhidden skipifdoesntexist

[Code]
// ── Проверяем запущен ли лаунчер или апдейтер ──────────────
function InitializeSetup(): Boolean;
var
  Msg: String;
begin
  Result := True;
  if CheckForMutexes('{#AppName}_Running_Mutex_v1') then
  begin
    Msg := '{#AppName} сейчас запущен.' + #13#10 +
           'Пожалуйста, закройте его и запустите установщик снова.';
    MsgBox(Msg, mbError, MB_OK);
    Result := False;
    Exit;
  end;
end;

// ── Записываем version.txt после установки ─────────────────
// INI-секция выше не пишет "чистый" текст, делаем через код
procedure CurStepChanged(CurStep: TSetupStep);
var
  VersionFile: String;
begin
  if CurStep = ssPostInstall then
  begin
    VersionFile := ExpandConstant('{localappdata}\{#AppName}\version.txt');
    SaveStringToFile(VersionFile, '{#AppVersion}', False);
  end;
end;
