@echo off
setlocal EnableExtensions
chcp 65001 >nul
cd /d "%~dp0"

for %%I in (.) do set "FOLDER_NAME=%%~nxI"

echo ========================================
echo   Git Push Button
echo ========================================
echo.

git rev-parse --git-dir >nul 2>&1
if not errorlevel 1 goto after_init

echo Это не git-репозиторий.
set /p "INIT_ANS=Создать репозиторий здесь? (y/n): "
if /i "%INIT_ANS%"=="y" goto do_init
if /i "%INIT_ANS%"=="д" goto do_init
echo Отменено.
goto end

:do_init
set /p "BR=Имя ветки (Enter = main): "
if "%BR%"=="" set "BR=main"
git init -b "%BR%"
if errorlevel 1 (
  echo Ошибка git init.
  goto end
)
echo Репозиторий создан. Ветка: %BR%
echo.

:after_init
echo Проверяю служебные файлы...

if not exist "README.md" (
  (
    echo # %FOLDER_NAME%
    echo.
    echo Краткое описание.
  ) > README.md
  echo   + README.md
)

if not exist ".gitignore" (
  (
    echo Thumbs.db
    echo Desktop.ini
    echo .DS_Store
    echo .vscode/
    echo .idea/
    echo *.swp
    echo .env
    echo .env.*
    echo *.pem
    echo *.key
    echo node_modules/
    echo dist/
    echo target/
    echo src-tauri/target/
    echo __pycache__/
    echo *.pyc
    echo .venv/
    echo venv/
    echo *.log
    echo *.tmp
    echo *.temp
    echo *.part
  ) > .gitignore
  echo   + .gitignore
)

if not exist ".gitattributes" (
  (
    echo * text=auto eol=lf
    echo *.bat text eol=crlf
    echo *.cmd text eol=crlf
    echo *.ps1 text eol=lf
    echo *.png binary
    echo *.jpg binary
    echo *.ico binary
    echo *.zip binary
    echo *.7z binary
  ) > .gitattributes
  echo   + .gitattributes
)

if not exist "CHANGELOG.md" (
  (
    echo # Changelog
    echo.
    echo ## [Unreleased]
    echo.
    echo ## [0.1.0] - %date%
    echo.
    echo ### Added
    echo - Project created
  ) > CHANGELOG.md
  echo   + CHANGELOG.md
)

echo.

git remote get-url origin >nul 2>&1
if not errorlevel 1 goto after_remote

echo Remote origin не найден.
echo.
set /p "GH_USER=Твой GitHub-логин: "
if "%GH_USER%"=="" (
  echo Логин обязателен.
  goto end
)
set /p "GH_REPO=Название репозитория (Enter = %FOLDER_NAME%): "
if "%GH_REPO%"=="" set "GH_REPO=%FOLDER_NAME%"
set "ORIGIN_URL=https://github.com/%GH_USER%/%GH_REPO%.git"
git remote add origin "%ORIGIN_URL%"
if errorlevel 1 (
  echo Не удалось добавить origin.
  goto end
)
echo Добавлен origin:
echo   %ORIGIN_URL%
echo.
set /p "HAS_REPO=Репозиторий на GitHub уже создан? (y/n): "
if /i "%HAS_REPO%"=="y" goto after_remote
if /i "%HAS_REPO%"=="д" goto after_remote
echo.
echo Создай пустой репозиторий:
echo   https://github.com/new
echo Имя: %GH_REPO%
echo Без галочки "Add a README".
echo После создания нажми Enter здесь.
pause

:after_remote
echo === Git status ===
git status
echo.

git diff --quiet 2>nul
set "DIFF=%errorlevel%"
git diff --cached --quiet 2>nul
set "CACHED=%errorlevel%"

if "%DIFF%"=="0" if "%CACHED%"=="0" (
  echo Нет изменений. Только push...
  goto do_push
)

echo === Добавляю файлы ===
git add -A
if errorlevel 1 (
  echo Ошибка git add.
  goto end
)

set /p "MSG=Сообщение коммита (Enter = update): "
if "%MSG%"=="" set "MSG=update %date% %time%"

echo === Commit: %MSG% ===
git commit -m "%MSG%"
if errorlevel 1 (
  echo Коммит не создан.
  goto end
)
echo.

:do_push
echo === Push ===
git push -u origin HEAD
if errorlevel 1 (
  echo.
  echo Ошибка push.
  echo.
  echo Если GitHub не авторизован, один раз сделай:
  echo   1. Установи GitHub CLI: https://cli.github.com
  echo   2. Выполни: gh auth login
  echo.
  echo Или настрой SSH-ключ / Git Credential Manager.
) else (
  echo.
  echo Готово.
)

:end
echo.
pause