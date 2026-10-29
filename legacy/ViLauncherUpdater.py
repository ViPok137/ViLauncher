"""
ViLauncher Updater  v1.1
========================
Запускается ВМЕСТО лаунчера через ярлык.
Показывает минималистичное окно только если есть обновление.
Если обновлений нет — запускает лаунчер сразу без UI.

Сборка в .exe:
  pip install pyinstaller
  pyinstaller --onefile --noconsole --icon=icon.ico updater.py
  Результат: dist/updater.exe → переименовать в ViLauncherUpdater.exe
"""

import os, sys, json, time, subprocess, tempfile, threading, urllib.request, shutil
from pathlib import Path

# ── Настройки ────────────────────────────────────────────────────────────────
GH_OWNER   = "ViPok137"
GH_REPO    = "ViLauncher"
GH_API_URL = f"https://api.github.com/repos/{GH_OWNER}/{GH_REPO}/releases/latest"

INSTALL_DIR  = Path(os.environ.get("LOCALAPPDATA", Path.home()/"AppData"/"Local")) / "ViLauncher"
LAUNCHER_EXE = INSTALL_DIR / "ViLauncher.exe"
VERSION_FILE = INSTALL_DIR / "version.txt"
TEMP_DIR     = Path(tempfile.gettempdir()) / "vilauncher-update"

ACCENT   = "#00f5ff"   # циановый — как в лаунчере
BG       = "#040810"   # тёмный фон
BG2      = "#0a1628"   # фон полоски прогресса
TEXT     = "#c8e8f8"   # основной текст
MUTED    = "#4a7090"   # вторичный текст
OK_COLOR = "#00ff88"   # зелёный — успех

# ── Логирование ──────────────────────────────────────────────────────────────
def log(msg: str) -> None:
    print(msg)
    try:
        INSTALL_DIR.mkdir(parents=True, exist_ok=True)
        with open(INSTALL_DIR / "updater.log", "a", encoding="utf-8") as f:
            from datetime import datetime
            f.write(f"[{datetime.now():%Y-%m-%d %H:%M:%S}] {msg}\n")
    except Exception:
        pass

# ── Сетевые утилиты ──────────────────────────────────────────────────────────
def fetch_release() :
    try:
        req = urllib.request.Request(
            GH_API_URL,
            headers={"User-Agent": "ViLauncherUpdater/1.1",
                     "Accept": "application/vnd.github.v3+json"}
        )
        with urllib.request.urlopen(req, timeout=8) as r:
            return json.loads(r.read())
    except Exception as e:
        log(f"fetch_release error: {e}")
        return None

def find_asset(release: dict) :
    for a in release.get("assets", []):
        n = a["name"].lower()
        if ("setup" in n or "install" in n) and n.endswith(".exe"):
            return a["browser_download_url"], a["name"]
    for a in release.get("assets", []):
        if a["name"].lower().endswith(".exe"):
            return a["browser_download_url"], a["name"]
    return None, None

def is_valid_exe(p: Path) -> bool:
    """Проверяет что файл является валидным Windows EXE (PE-формат).
    Сигнатура: первые 2 байта = 4D 5A (MZ) — обязательна для любого .exe."""
    if not p.exists() or p.stat().st_size < 1024:  # минимальный размер exe
        return False
    try:
        with open(p, "rb") as f:
            header = f.read(2)
        return header == b'\x4d\x5a'  # MZ signature
    except Exception:
        return False

def get_version() -> str:
    try: return VERSION_FILE.read_text(encoding="utf-8").strip()
    except: return "0.0.0"

def save_version(v: str) -> None:
    try: INSTALL_DIR.mkdir(parents=True, exist_ok=True); VERSION_FILE.write_text(v, encoding="utf-8")
    except: pass

def launch_launcher() -> None:
    # Ждём чуть-чуть — Inno Setup может ещё писать файлы
    time.sleep(1)
    if not LAUNCHER_EXE.exists():
        log(f"[updater] Лаунчер не найден: {LAUNCHER_EXE}")
        # Ищем в той же папке что и апдейтер
        alt = Path(sys.executable).parent / "ViLauncher.exe"
        if alt.exists():
            LAUNCHER_EXE_USE = alt
            log(f"[updater] Найден альтернативный путь: {alt}")
        else:
            log("[updater] ViLauncher.exe не найден нигде, выходим")
            os._exit(1)
    else:
        LAUNCHER_EXE_USE = LAUNCHER_EXE

    try:
        log(f"[updater] Запускаю: {LAUNCHER_EXE_USE}")
        subprocess.Popen(
            [str(LAUNCHER_EXE_USE)],
            creationflags=subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP,
            close_fds=True,
        )
        time.sleep(0.5)
    except Exception as e:
        log(f"[updater] Не удалось запустить лаунчер: {e}")
    os._exit(0)  # жёсткий выход — убивает все потоки и tkinter

def kill_old_process() -> None:
    """Завершает ViLauncher.exe и ждёт пока Windows освободит файлы."""
    try:
        # Шаг 1: graceful завершение
        subprocess.run(
            ["taskkill", "/im", "ViLauncher.exe"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        time.sleep(1)
        # Шаг 2: принудительное завершение если ещё жив
        subprocess.run(
            ["taskkill", "/f", "/im", "ViLauncher.exe"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        # Шаг 3: ждём пока Windows отпустит блокировки файлов
        for _ in range(10):
            time.sleep(0.5)
            result = subprocess.run(
                ["tasklist", "/fi", "imagename eq ViLauncher.exe", "/fo", "csv", "/nh"],
                capture_output=True, text=True,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            if "ViLauncher.exe" not in result.stdout:
                break  # процесс завершён
        time.sleep(1)  # доп. пауза для Windows
        log("[updater] ViLauncher.exe завершён")
    except Exception as e:
        log(f"[updater] kill_old_process: {e}")


# ── GUI ──────────────────────────────────────────────────────────────────────
class UpdaterWindow:
    """Минималистичное окно обновления без рамки в стиле лаунчера."""

    def __init__(self, from_ver: str, to_ver: str):
        import tkinter as tk
        self.tk = tk
        self.from_ver = from_ver
        self.to_ver   = to_ver

        self.root = tk.Tk()
        self.root.title("ViLauncher Updater")
        self.root.resizable(False, False)
        self.root.overrideredirect(True)          # убираем рамку ОС
        self.root.configure(bg=BG)
        self.root.attributes("-topmost", True)    # поверх всех окон

        W, H = 420, 220
        sw = self.root.winfo_screenwidth()
        sh = self.root.winfo_screenheight()
        self.root.geometry(f"{W}x{H}+{(sw-W)//2}+{(sh-H)//2}")

        self._build_ui()
        self._make_draggable()

    def _build_ui(self):
        tk = self.tk
        root = self.root

        # ── Верхняя рамка-акцент (1px) ──
        accent_line = tk.Frame(root, height=2, bg=ACCENT)
        accent_line.pack(fill="x", side="top")

        # ── Заголовок ──
        header = tk.Frame(root, bg=BG, pady=18)
        header.pack(fill="x", padx=24)

        tk.Label(header, text="VI", font=("Orbitron", 18, "bold"),
                 fg=ACCENT, bg=BG).pack(side="left")
        tk.Label(header, text="POK", font=("Orbitron", 18, "bold"),
                 fg="#ff2d78", bg=BG).pack(side="left")
        tk.Label(header, text="  //  LAUNCHER UPDATE",
                 font=("Courier New", 9), fg=MUTED, bg=BG).pack(side="left", pady=2)

        # ── Версии ──
        ver_frame = tk.Frame(root, bg=BG)
        ver_frame.pack(fill="x", padx=24)
        tk.Label(ver_frame,
                 text=f"v{self.from_ver}  →  v{self.to_ver}",
                 font=("Courier New", 11, "bold"),
                 fg=TEXT, bg=BG).pack(side="left")

        # ── Статус ──
        self.status_var = tk.StringVar(value="Подключаюсь к серверу...")
        self.status_lbl = tk.Label(root, textvariable=self.status_var,
                                   font=("Courier New", 9),
                                   fg=MUTED, bg=BG, anchor="w")
        self.status_lbl.pack(fill="x", padx=24, pady=(14, 4))

        # ── Прогресс-бар ──
        bar_bg = tk.Frame(root, bg=BG2, height=6, bd=0)
        bar_bg.pack(fill="x", padx=24)
        bar_bg.pack_propagate(False)

        self.bar_fill = tk.Frame(bar_bg, bg=ACCENT, height=6, width=0)
        self.bar_fill.place(x=0, y=0, height=6)
        self._bar_max_w = 420 - 48  # 372px

        # ── Процент ──
        self.pct_var = tk.StringVar(value="")
        tk.Label(root, textvariable=self.pct_var,
                 font=("Courier New", 9), fg=ACCENT, bg=BG,
                 anchor="e").pack(fill="x", padx=24)

        # ── Нижняя подпись ──
        tk.Label(root, text="© ViPok Server  //  github.com/ViPok137/ViLauncher",
                 font=("Courier New", 7), fg=MUTED, bg=BG
                 ).pack(side="bottom", pady=8)

    def _make_draggable(self):
        """Позволяет перетаскивать окно без рамки."""
        self._drag_x = self._drag_y = 0

        def on_press(e):
            self._drag_x = e.x
            self._drag_y = e.y

        def on_drag(e):
            x = self.root.winfo_x() + e.x - self._drag_x
            y = self.root.winfo_y() + e.y - self._drag_y
            self.root.geometry(f"+{x}+{y}")

        self.root.bind("<ButtonPress-1>", on_press)
        self.root.bind("<B1-Motion>",     on_drag)

    # ── Публичные методы (вызываются из фонового потока) ──────────────────────

    def set_status(self, text: str, color: str = MUTED) -> None:
        self.root.after(0, lambda: (
            self.status_var.set(text),
            self.status_lbl.configure(fg=color)
        ))

    def set_progress(self, pct: int, text: str = "") -> None:
        w = int(self._bar_max_w * pct / 100)
        self.root.after(0, lambda: (
            self.bar_fill.place(x=0, y=0, height=6, width=w),
            self.pct_var.set(text or f"{pct}%"),
        ))
        


    def set_done(self) -> None:
        self.root.after(0, lambda: (
            self.bar_fill.configure(bg=OK_COLOR),
            self.bar_fill.place(x=0, y=0, height=6, width=self._bar_max_w),
            self.status_lbl.configure(fg=OK_COLOR),
        ))

    def close(self) -> None:
        self.root.after(800, self.root.destroy)

    def run(self, worker_fn) -> None:
        """Запускает worker_fn в фоновом потоке, UI в главном."""
        t = threading.Thread(target=worker_fn, daemon=True)
        t.start()
        self.root.mainloop()

# ── Рабочий поток ────────────────────────────────────────────────────────────
def head_request_ok(url: str) -> bool:
    """ТЗ 1b: HEAD-запрос перед скачиванием.
    Если сервер возвращает не 200 — отменяем обновление и логируем ошибку."""
    try:
        req = urllib.request.Request(url, method="HEAD",
                                     headers={"User-Agent": "ViLauncherUpdater/2.0"})
        with urllib.request.urlopen(req, timeout=8) as resp:
            if resp.status != 200:
                log(f"[head] Сервер вернул HTTP {resp.status} — отмена обновления")
                return False
            return True
    except Exception as e:
        log(f"[head] HEAD-запрос не удался: {e}")
        return False


def run_update(win: "UpdaterWindow", url: str, dest: Path, new_ver: str) -> None:
    try:
        # ТЗ 1b: HEAD-запрос ПЕРЕД скачиванием — проверяем что файл существует на сервере
        win.set_status("🔍 Проверяю доступность файла...", ACCENT)
        if not head_request_ok(url):
            win.set_status("⚠ Файл не найден на сервере — обновление отменено", "#ff2d78")
            log("[update] HEAD-запрос вернул не-200 — запускаю текущий лаунчер без обновления")
            time.sleep(2)
            launch_launcher()
            return

        # Скачиваем
        win.set_status("⬇ Скачиваю обновление...", ACCENT)
        TEMP_DIR.mkdir(parents=True, exist_ok=True)
        tmp = dest.with_suffix(dest.suffix + ".tmp")

        req = urllib.request.Request(url, headers={"User-Agent": "ViLauncherUpdater/2.0"})
        with urllib.request.urlopen(req, timeout=120) as resp:
            total = int(resp.headers.get("Content-Length", 0))
            done  = 0
            with open(tmp, "wb") as f:
                while True:
                    chunk = resp.read(65536)
                    if not chunk: break
                    f.write(chunk)
                    done += len(chunk)
                    if total > 0:
                        pct = int(done / total * 100)
                        mb  = done / 1048576
                        mbt = total / 1048576
                        win.set_progress(pct, f"{pct}%  {mb:.1f}/{mbt:.1f} МБ")
        tmp.replace(dest)

        if not is_valid_exe(dest):
            win.set_status("⚠ Файл повреждён, пробую снова...", "#ff2d78")
            dest.unlink(missing_ok=True)
            time.sleep(2); launch_launcher()
            return

        # Устанавливаем
        win.set_progress(100, "100%")
        win.set_status("⚙ Устанавливаю...", TEXT)
        log("[updater] Запускаю установщик тихо")

        # Запускаем установщик с повторной попыткой при ошибке доступа (код 5)
        result = None
        for attempt in range(3):
            if attempt > 0:
                win.set_status(f"⚙ Повторная попытка {attempt+1}/3...", TEXT)
                kill_old_process()  # убеждаемся что процесс мёртв
            result = subprocess.run(
                [str(dest), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"],
                timeout=5 * 60,
            )
            if result.returncode == 0:
                break
            if result.returncode == 5:
                log(f"[updater] Код 5 (ACCESS DENIED) попытка {attempt+1}, жду и повторяю...")
                time.sleep(3)
            else:
                break  # другой код — не повторяем

        dest.unlink(missing_ok=True)  # ТЗ 1c: очищаем temp в любом случае (успех или ошибка)

        if result and result.returncode == 0:
            save_version(new_ver)
            win.set_status(f"✓ Обновлено до v{new_ver}  Запускаю...", OK_COLOR)
        else:
            code = result.returncode if result else -1
            log(f"[updater] Установщик завершился с кодом {code}")
            if code == 5:
                win.set_status("⚠ Нет доступа к файлам — закрой лаунчер вручную", "#ff2d78")
            else:
                win.set_status(f"⚠ Установщик: код {code}", "#ff2d78")
            # Всё равно обновляем version.txt если файлы частично установились
            # (чтобы не зациклиться при следующем запуске)
            save_version(new_ver)

        win.set_done()
        time.sleep(1.5)
        win.root.after(0, win.root.destroy)  # закрываем tkinter из main loop
        time.sleep(1)
        launch_launcher()
        win.run("ViLauncher.exe")

    except Exception as e:
        log(f"[updater] Ошибка в run_update: {e}")
        win.set_status(f"Ошибка: {e}", "#ff2d78")
        time.sleep(3)
        win.close()
        time.sleep(1)
        launch_launcher()

# ── Точка входа ──────────────────────────────────────────────────────────────

def self_relocate_if_needed() -> None:
    """Если запущены из папки установки — копируем себя в TEMP и перезапускаем.
    Это нужно чтобы Inno Setup мог перезаписать ViLauncherUpdater.exe."""
    try:
        my_path = Path(sys.executable if getattr(sys, "frozen", False) else __file__).resolve()
        install_path = INSTALL_DIR.resolve()
        # Проверяем: мы внутри папки установки?
        if str(my_path).startswith(str(install_path)):
            tmp_exe = Path(tempfile.gettempdir()) / "ViLauncherUpdater_run.exe"
            log(f"[updater] Копирую себя в temp: {tmp_exe}")
            shutil.copy2(str(my_path), str(tmp_exe))
            # Запускаем копию из TEMP и выходим
            subprocess.Popen(
                [str(tmp_exe)] + sys.argv[1:],
                creationflags=subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP,
            )
            time.sleep(0.5)
            os._exit(0)  # жёсткий выход — не sys.exit чтобы не было cleanup-хвостов
    except Exception as e:
        log(f"[updater] self_relocate: {e}")  # не критично — продолжаем

def main() -> None:
    self_relocate_if_needed()  # перемещаемся в TEMP если нужно
    log("=" * 50)
    log("[updater] Запущен")

    # Сразу убиваем зависший лаунчер, если он есть
    kill_old_process()

    current = get_version()
    log(f"[updater] Текущая версия: {current}")

    release = fetch_release()
    if not release:
        log("[updater] Нет соединения, запускаю лаунчер")
        launch_launcher()
        return

    remote = release.get("tag_name", "").lstrip("v")
    log(f"[updater] Последняя на сервере: {remote}")

    # Строгая проверка через !=
    def parse_ver(v):
        try: return tuple(int(x) for x in v.strip().split("."))
        except: return (0, 0, 0)

    if remote != "" and parse_ver(remote) != parse_ver(current):
        url, name = find_asset(release)
        if not url:
            log("[updater] Установщик не найден в релизе")
            launch_launcher()
            return

        log(f"[updater] Обновление {current} -> {remote}: {name}")
        dest = TEMP_DIR / name

        try:
            win = UpdaterWindow(from_ver=current, to_ver=remote)
            win.run(lambda: run_update(win, url, dest, remote))
        except ImportError:
            log("[updater] tkinter недоступен, режим без UI")
            run_update(None, url, dest, remote)  # type: ignore
    else:
        log("[updater] Версии совпадают (или нет тега), запускаю лаунчер")
        launch_launcher()

if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        log(f"[updater] Критическая ошибка: {e}")
        launch_launcher()