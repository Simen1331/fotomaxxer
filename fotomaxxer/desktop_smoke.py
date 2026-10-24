"""Intern GUI-røykprøve: ekte WebEngine, IPC og prosessarbeidere uten brukerdata."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request

from PySide6.QtCore import QObject, QTimer, QUrl
from PySide6.QtWebEngineCore import QWebEnginePage

from api.jobs import runner
from .desktop_window import local_url
from .packaging_probe import run


class DesktopSmoke(QObject):
    def __init__(self, window, instance, output: Path, portrait: Path):
        super().__init__(window)
        self.window, self.instance = window, instance
        self.output, self.portrait = output, portrait.resolve()
        self.report = {"frozen": bool(getattr(sys, "frozen", False)), "checks": []}
        self.started = time.monotonic()
        self.checking_dom = False
        self.work_started = False
        self.ready_to_close = threading.Event()
        self.close_checked = False
        self.timer = QTimer(self)
        self.timer.setInterval(100)
        self.timer.timeout.connect(self.tick)
        self.timer.start()

    def check(self, ok: bool, label: str):
        self.report["checks"].append({"ok": bool(ok), "label": label})

    def tick(self):
        if self.ready_to_close.is_set() and not self.close_checked:
            self.close_checked = True
            self.check(runner.is_running(), "Lukking starter mens en bakgrunnsjobb kjører")
            self.window.begin_close()
            self.check(runner.is_closing(), "Nye jobber er stengt før avslutning")
        elif not self.work_started and self.window.loaded and not self.checking_dom:
            self.checking_dom = True
            self.window.page.runJavaScript(
                "(() => { const root=document.getElementById('root'); return JSON.stringify({"
                "mounted: !!root && root.childElementCount>0, text:root?.innerText||'',"
                "width:window.innerWidth,height:window.innerHeight,"
                "images:Array.from(document.images).filter(i=>i.complete&&i.naturalWidth>0).length}); })()",
                self.dom_ready,
            )
        if not self.work_started and time.monotonic() - self.started > 40:
            self.check(False, "React ble klar innen tidsfristen")
            self.window.exit_code = 1
            self.window.begin_close()
            self.timer.stop()

    def dom_ready(self, result):
        self.checking_dom = False
        if isinstance(result, str) and result:
            result = json.loads(result)
        if not result or not result.get("mounted") or "Laster biblioteket" in result.get("text", ""):
            return
        if result.get("width", 0) < 300 or result.get("height", 0) < 200:
            return
        if "Ingen bilder her." not in result.get("text", "") and result.get("images", 0) == 0:
            return
        self.work_started = True
        self.check(True, "React rendres i Qt WebEngine")
        self.report["dom"] = result
        self.report["ui_ready_seconds"] = round(time.monotonic() - self.started, 3)
        self.check(local_url(self.window.view.url(), self.window.server.origin), "Vinduet viser appens tildelte origin")
        self.check(not self.window.page.acceptNavigationRequest(
            QUrl("https://example.com/"), QWebEnginePage.NavigationType.NavigationTypeLinkClicked, True,
        ), "Navigasjon til en fremmed nettside avvises")
        self.check(not self.window.page.acceptNavigationRequest(
            QUrl("file:///etc/passwd"), QWebEnginePage.NavigationType.NavigationTypeLinkClicked, True,
        ), "Navigasjon til lokale filer avvises")
        self.check(self.window.profile.isOffTheRecord(), "WebEngine-profilen lagrer ingen varige sesjonsdata")
        # La Chromium levere den malte rammen etter DOM-/bildesjekken.
        QTimer.singleShot(300, self.snapshot)

    def snapshot(self):
        self.window.grab().save(str(self.output / "window.png"))
        runner.start("desktop-smoke", self.work)

    def work(self, job):
        try:
            server = self.window.server
            for headers, label in (({}, "API uten token avvises"),
                                   ({"X-FotoMaxxer-Session": server.token, "Origin": "https://example.com"}, "API med fremmed origin avvises")):
                try:
                    server.opener.open(Request(server.origin + "/api/health", headers=headers), timeout=5).close()
                    self.check(False, label)
                except HTTPError as error:
                    self.check(error.code == 403, label)
            command = [sys.executable] if getattr(sys, "frozen", False) else [sys.executable, "-m", "fotomaxxer.desktop"]
            second = subprocess.run(command + ["--home", os.environ["FOTOMAXXER_HOME"]],
                                    capture_output=True, timeout=15)
            self.check(second.returncode == 0 and self.instance.activations >= 1, "Ny oppstart fokuserer eksisterende instans")
            # Tjeneren avsluttes mens jobben og dens ekte spawn-arbeidere arbeider.
            self.ready_to_close.set()
            result = run(str(self.output / "pipeline"), str(self.portrait), workers=2)
            self.check(result == 0, "Pakket bilde- og ansiktspipeline fra Qt-prosessen")
            self.report["pipeline"] = json.loads((self.output / "pipeline" / "report.json").read_text(encoding="utf-8"))
        except Exception as error:
            self.report["error"] = f"{type(error).__name__}: {error}"
            self.check(False, "GUI-røykprøven fullførte")
            self.ready_to_close.set()

    def finish(self):
        self.timer.stop()
        self.check(self.window.server.stopped() and runner.wait(0), "Tjener og bakgrunnsjobb er avsluttet før vinduet lukkes")
        self.report["seconds"] = round(time.monotonic() - self.started, 2)
        self.report["ok"] = "error" not in self.report and all(c["ok"] for c in self.report["checks"])
        self.window.exit_code = 0 if self.report["ok"] else 1
        (self.output / "desktop-report.json").write_text(
            json.dumps(self.report, indent=2, ensure_ascii=False), encoding="utf-8",
        )
