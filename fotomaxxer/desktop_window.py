"""Qt-vindu, navigasjonsgrense og fokusering av én instans per datamappe."""

from __future__ import annotations

import hashlib
import logging
import os
import time
from pathlib import Path

from PySide6.QtCore import QLockFile, QTimer, Qt, QUrl
from PySide6.QtNetwork import QLocalServer, QLocalSocket
from PySide6.QtWebEngineCore import QWebEnginePage, QWebEngineProfile, QWebEngineUrlRequestInterceptor
from PySide6.QtWebEngineWidgets import QWebEngineView
from PySide6.QtWidgets import QLabel, QMainWindow, QMessageBox

from api.jobs import runner


class SingleInstance:
    def __init__(self, root: Path):
        key = hashlib.sha256(os.path.normcase(str(root.resolve())).encode()).hexdigest()[:32]
        self.name = "fotomaxxer-" + key
        self.lock = QLockFile(str(root / ".desktop.lock"))
        self.lock.setStaleLockTime(0)
        self.server = QLocalServer()
        self.server.setSocketOptions(QLocalServer.SocketOption.UserAccessOption)
        self.owned = False
        self.activations = 0
        self.connections = set()

    def acquire(self) -> bool:
        self.owned = self.lock.tryLock(0)
        return self.owned

    def activate_existing(self) -> bool:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            socket = QLocalSocket()
            socket.connectToServer(self.name)
            if socket.waitForConnected(200):
                socket.write(b"activate\n")
                socket.flush()
                if socket.waitForReadyRead(500) and bytes(socket.readAll()).strip() == b"ok":
                    socket.disconnectFromServer()
                    return True
            socket.abort()
            time.sleep(0.1)
        return False

    def listen(self, window: QMainWindow) -> None:
        QLocalServer.removeServer(self.name)
        if not self.server.listen(self.name):
            raise RuntimeError("Kunne ikke opprette lokal kanal for vindusfokus.")

        def accept():
            while self.server.hasPendingConnections():
                socket = self.server.nextPendingConnection()
                self.connections.add(socket)

                def read(socket=socket):
                    if bytes(socket.readAll()).strip() == b"activate":
                        if window.isMinimized():
                            window.showNormal()
                        window.raise_()
                        window.activateWindow()
                        self.activations += 1
                        socket.write(b"ok\n")
                        socket.flush()
                        socket.disconnectFromServer()

                def cleanup(socket=socket):
                    self.connections.discard(socket)
                    socket.deleteLater()

                socket.readyRead.connect(read)
                socket.disconnected.connect(cleanup)
                if socket.bytesAvailable():
                    read()

        self.server.newConnection.connect(accept)

    def release(self) -> None:
        if self.owned:
            for socket in tuple(self.connections):
                socket.abort()
            self.server.close()
            QLocalServer.removeServer(self.name)
            self.lock.unlock()
            self.owned = False


def local_url(url: QUrl, origin: str) -> bool:
    expected = QUrl(origin)
    return url.scheme() == "http" and url.host() == expected.host() and url.port() == expected.port()


class AppRequests(QWebEngineUrlRequestInterceptor):
    def __init__(self, server, parent):
        super().__init__(parent)
        self.server = server

    def interceptRequest(self, info):
        url = info.requestUrl()
        if local_url(url, self.server.origin):
            info.setHttpHeader(b"X-FotoMaxxer-Session", self.server.token.encode())
        elif url.scheme() == "data" or (url.scheme() == "blob" and url.path().startswith(self.server.origin + "/")):
            return
        else:
            info.block(True)


class AppPage(QWebEnginePage):
    def __init__(self, profile, origin: str, parent):
        super().__init__(profile, parent)
        self.origin = origin

    def acceptNavigationRequest(self, url, navigation_type, is_main_frame):
        return url.toString() == "about:blank" or local_url(url, self.origin)

    def createWindow(self, window_type):
        return None

    def javaScriptConsoleMessage(self, level, message, line, source):
        logging.getLogger("desktop.web").info("JavaScript: %s (%s:%s)", message, source, line)


class DesktopWindow(QMainWindow):
    def __init__(self, server, log_path: Path):
        super().__init__()
        self.server = server
        self.log_path = log_path
        self.exit_code = 0
        self.closing = False
        self.loaded = False
        self.started = time.monotonic()
        self.setWindowTitle("FotoMaxxer")
        self.resize(1280, 850)
        self.label = QLabel("Starter FotoMaxxer …", self)
        self.label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.setCentralWidget(self.label)
        # Profil uten varig cache eller cookies. Tokenet finnes kun i interceptor.
        self.profile = QWebEngineProfile(self)
        self.interceptor = AppRequests(server, self.profile)
        self.profile.setUrlRequestInterceptor(self.interceptor)
        self.profile.downloadRequested.connect(lambda download: download.cancel())
        self.view = QWebEngineView(self)
        self.view.setContextMenuPolicy(Qt.ContextMenuPolicy.NoContextMenu)
        self.page = AppPage(self.profile, server.origin, self.view)
        self.view.setPage(self.page)
        self.view.loadFinished.connect(self._loaded)
        self.timer = QTimer(self)
        self.timer.setInterval(100)
        self.timer.timeout.connect(self._tick)
        self.timer.start()

    def _tick(self):
        if self.closing:
            if self.server.stopped():
                self.close()
            return
        if self.server.error is not None or (self.server.thread.ident is not None and self.server.stopped()):
            self._error("Den lokale tjeneren stoppet uventet.")
        elif not self.loaded:
            if time.monotonic() - self.started > 30:
                self._error("Oppstarten tok for lang tid.")
            elif self.view.url().isEmpty() and self.server.ready():
                self.view.setUrl(QUrl(self.server.origin))

    def _loaded(self, ok: bool):
        if self.closing:
            return
        if not ok:
            self._error("Grensesnittet kunne ikke lastes.")
            return
        self.loaded = True
        self.setCentralWidget(self.view)
        self.view.show()
        logging.info("Desktop-grensesnitt klart")

    def _error(self, message: str):
        self.exit_code = 1
        logging.error(message)
        self.timer.stop()
        QMessageBox.critical(self, "FotoMaxxer", f"{message}\n\nLogg: {self.log_path}")
        self.begin_close()
        self.timer.start()

    def begin_close(self):
        if not self.closing:
            self.closing = True
            self.server.request_stop()
            self.view.setEnabled(False)
            self.statusBar().showMessage("Venter på pågående arbeid og avslutter …")

    def closeEvent(self, event):
        if self.closing:
            if self.server.stopped():
                self.timer.stop()
                # Slett siden før off-record-profilen (Qt krever denne rekkefølgen).
                self.page.deleteLater()
                event.accept()
            else:
                event.ignore()
            return
        if runner.is_running():
            dialog = QMessageBox(self)
            dialog.setWindowTitle("Arbeid pågår")
            dialog.setText("FotoMaxxer behandler fortsatt bilder. Vil du vente til jobben er ferdig og deretter lukke?")
            wait = dialog.addButton("Vent og lukk", QMessageBox.ButtonRole.AcceptRole)
            dialog.addButton("Fortsett å bruke appen", QMessageBox.ButtonRole.RejectRole)
            dialog.exec()
            if dialog.clickedButton() is not wait:
                event.ignore()
                return
        self.begin_close()
        event.ignore()
