import os
import sys
import shutil
import winshell
from pathlib import Path
from win32com.client import Dispatch
from PySide6.QtWidgets import (QApplication, QMainWindow, QPushButton, QVBoxLayout, 
                             QWidget, QCheckBox, QLabel, QProgressBar, QMessageBox)
from PySide6.QtCore import Qt, QThread, Signal

class InstallWorker(QThread):
    progress = Signal(int)
    finished = Signal(bool, str)

    def __init__(self, source_dir, install_dir, exe_name, options):
        super().__init__()
        self.source_dir = source_dir
        self.install_dir = install_dir
        self.exe_name = exe_name
        self.options = options # {'desktop': bool, 'start_menu': bool, 'startup': bool}

    def run(self):
        try:
            # 1. Копирование файлов
            if os.path.exists(self.install_dir):
                shutil.rmtree(self.install_dir)
            
            # Эмуляция прогресса для красоты
            self.progress.emit(20)
            shutil.copytree(self.source_dir, self.install_dir)
            self.progress.emit(60)

            target_exe = os.path.join(self.install_dir, self.exe_name)
            shell = Dispatch('WScript.Shell')

            # 2. Ярлык на рабочий стол
            if self.options['desktop']:
                desktop = winshell.desktop()
                shortcut = shell.CreateShortCut(os.path.join(desktop, "ViLauncher.lnk"))
                shortcut.Targetpath = target_exe
                shortcut.WorkingDirectory = self.install_dir
                shortcut.IconLocation = target_exe
                shortcut.save()

            # 3. Меню Пуск (чтобы находилось поиском)
            if self.options['start_menu']:
                start_menu = os.path.join(os.environ['APPDATA'], r'Microsoft\Windows\Start Menu\Programs')
                shortcut = shell.CreateShortCut(os.path.join(start_menu, "ViLauncher.lnk"))
                shortcut.Targetpath = target_exe
                shortcut.WorkingDirectory = self.install_dir
                shortcut.save()

            # 4. Автозагрузка
            if self.options['startup']:
                startup = winshell.startup()
                shortcut = shell.CreateShortCut(os.path.join(startup, "ViLauncher.lnk"))
                shortcut.Targetpath = target_exe
                shortcut.WorkingDirectory = self.install_dir
                shortcut.save()

            self.progress.emit(100)
            self.finished.emit(True, "Установка успешно завершена!")
        except Exception as e:
            self.finished.emit(False, str(e))

class InstallerWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("ViLauncher Setup")
        self.setFixedSize(400, 300)

        # Пути (предполагаем, что папка win-unpacked лежит рядом с установщиком)
        self.base_path = getattr(sys, '_MEIPASS', os.path.dirname(os.path.abspath(__file__)))
        self.source = os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "win-unpacked")
        self.target = os.path.join(os.environ['LOCALAPPDATA'], "ViLauncher")
        self.exe_name = "ViLauncher.exe" # Укажи здесь точное имя своего файла

        self.init_ui()

    def init_ui(self):
        layout = QVBoxLayout()

        self.label = QLabel("Установка ViLauncher")
        self.label.setStyleSheet("font-size: 18px; font-weight: bold; margin-bottom: 10px;")
        layout.addWidget(self.label)

        self.cb_desktop = QCheckBox("Создать ярлык на рабочем столе")
        self.cb_desktop.setChecked(True)
        layout.addWidget(self.cb_desktop)

        self.cb_start_menu = QCheckBox("Добавить в меню Пуск (Поиск Windows)")
        self.cb_start_menu.setChecked(True)
        layout.addWidget(self.cb_start_menu)

        self.cb_startup = QCheckBox("Запускать автоматически при старте системы")
        layout.addWidget(self.cb_startup)

        self.progress_bar = QProgressBar()
        self.progress_bar.setVisible(False)
        layout.addWidget(self.progress_bar)

        self.btn_install = QPushButton("Установить")
        self.btn_install.setFixedHeight(40)
        self.btn_install.clicked.connect(self.start_installation)
        layout.addWidget(self.btn_install)

        container = QWidget()
        container.setLayout(layout)
        self.setCentralWidget(container)

    def start_installation(self):
        if not os.path.exists(self.source):
            QMessageBox.critical(self, "Ошибка", f"Не найдена папка {self.source}")
            return

        self.btn_install.setEnabled(False)
        self.progress_bar.setVisible(True)
        
        options = {
            'desktop': self.cb_desktop.isChecked(),
            'start_menu': self.cb_start_menu.isChecked(),
            'startup': self.cb_startup.isChecked()
        }

        self.worker = InstallWorker(self.source, self.target, self.exe_name, options)
        self.worker.progress.connect(self.progress_bar.setValue)
        self.worker.finished.connect(self.on_finished)
        self.worker.start()

    def on_finished(self, success, message):
        if success:
            QMessageBox.information(self, "Готово", message)
            sys.exit()
        else:
            QMessageBox.critical(self, "Ошибка", f"Произошла ошибка: {message}")
            self.btn_install.setEnabled(True)

if __name__ == "__main__":
    app = QApplication(sys.argv)
    window = InstallerWindow()
    window.show()
    sys.exit(app.exec())