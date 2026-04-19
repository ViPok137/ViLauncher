; ============================================================
; ViLauncher — Inno Setup Script
; Требует: Inno Setup 6.x с Unicode поддержкой
; Сборка: ISCC.exe launcher_setup.iss
; ============================================================

#define AppName       "ViLauncher"
#define AppVersion    "0.3.1"
#define AppPublisher  "ViPok"
#define AppExeName    "ViLauncher.exe"
#define AppURL        "https://github.com/ViPok137/ViLauncher"
#define MySourceDir   "win-unpacked"

; ── Глобальные флаги компиляции ─────────────────────────────
; Стабильный Unicode — все строки в UTF-8, совместимо с Win10/11
#pragma option -u+

[Setup]
; Уникальный GUID — не меняй после первого релиза!
; Чтобы сгенерировать новый: Tools → Generate GUID в Inno Setup IDE
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
; Устанавливаем для текущего пользователя (не всей системы)
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; ── Mutex — один экземпляр лаунчера ─────────────────────────
AppMutex={#AppName}_Running_Mutex_v1

; ── Внешний вид ─────────────────────────────────────────────
WizardStyle=modern
; Иконка установщика (раскомментируй когда будет icon.ico)
; SetupIconFile=icon.ico
; SetupIconFile={#MySourceDir}\resources\icon.ico

; ── Вывод ───────────────────────────────────────────────────
OutputDir=installer_output
OutputBaseFilename=ViLauncher_Setup_v{#AppVersion}

; ── Сжатие ──────────────────────────────────────────────────
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes

; ── Unicode и кодировка (Stable Unicode) ────────────────────
; Inno Setup 6.x по умолчанию Unicode — явно указываем для надёжности
; Все языковые файлы должны быть UTF-8 with BOM или UTF-16
; Русские символы в названиях папок/ярлыков будут работать корректно

; ── Удаление ────────────────────────────────────────────────
; НЕ удаляем папку игры при деинсталляции (там данные пользователя)
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}

; ── Версия для Windows ──────────────────────────────────────
MinVersion=10.0

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Ярлык на рабочий стол — по умолчанию выключен
Name: "desktopicon";  Description: "{cm:CreateDesktopIcon}"; \
  GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

; Автозапуск с Windows — по умолчанию выключен
Name: "startupicon";  Description: "Запускать при старте Windows"; \
  GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Основной EXE — всегда обновляем
Source: "{#MySourceDir}\{#AppExeName}";   DestDir: "{app}"; \
  Flags: ignoreversion

; Все остальные файлы из win-unpacked рекурсивно
Source: "{#MySourceDir}\*";               DestDir: "{app}"; \
  Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
; Создаём папку для логов лаунчера — права текущего пользователя
Name: "{localappdata}\{#AppName}"; Permissions: users-full

[Icons]
; Меню Пуск
Name: "{group}\{#AppName}";               Filename: "{app}\{#AppExeName}"
; Поиск Windows (autoprograms)
Name: "{autoprograms}\{#AppName}";        Filename: "{app}\{#AppExeName}"
; Рабочий стол (по задаче)
Name: "{autodesktop}\{#AppName}";         Filename: "{app}\{#AppExeName}"; \
  Tasks: desktopicon
; Автозагрузка (по задаче)
Name: "{userstartup}\{#AppName}";         Filename: "{app}\{#AppExeName}"; \
  Tasks: startupicon

[Registry]
; Регистрируем приложение для корректного отображения в «Программы и компоненты»
Root: HKCU; Subkey: "Software\{#AppPublisher}\{#AppName}"; \
  ValueType: string; ValueName: "InstallPath"; ValueData: "{app}"; \
  Flags: uninsdeletekey

[Run]
; Запустить после установки
Filename: "{app}\{#AppExeName}"; \
  Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; \
  Flags: nowait postinstall skipifsilent

[UninstallRun]
; Завершаем процесс перед удалением если запущен
Filename: "taskkill.exe"; Parameters: "/F /IM {#AppExeName}"; \
  Flags: runhidden skipifdoesntexist

[Code]
// ── Проверяем запущен ли лаунчер перед установкой/обновлением ──
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
  end;
end;
