"""Kontrollert loopback-tjener, med én socket fra bind til avslutning."""

from __future__ import annotations

import json
import logging
import secrets
import socket
import threading
from urllib.request import ProxyHandler, Request, build_opener

import uvicorn


class LocalServer:
    def __init__(self):
        from api.app import create_app

        self.token = secrets.token_urlsafe(32)
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            self.socket.bind(("127.0.0.1", 0))
            self.socket.listen(128)
            self.port = self.socket.getsockname()[1]
            self.origin = f"http://127.0.0.1:{self.port}"
            self.app = create_app(session_token=self.token, origin=self.origin)
            self.server = uvicorn.Server(uvicorn.Config(
                self.app, host="127.0.0.1", port=self.port, loop="asyncio", http="h11",
                ws="none", log_config=None, access_log=False, timeout_keep_alive=1,
            ))
        except BaseException:
            self.socket.close()
            raise
        self.error: BaseException | None = None
        self.thread = threading.Thread(target=self._run, name="desktop-api", daemon=False)
        self.opener = build_opener(ProxyHandler({}))

    def start(self) -> None:
        self.thread.start()

    def _run(self) -> None:
        try:
            self.server.run(sockets=[self.socket])
        except BaseException as error:
            self.error = error
            logging.getLogger(__name__).exception("Desktop-API feilet")
        finally:
            self.socket.close()

    def ready(self) -> bool:
        if not self.server.started or self.error is not None:
            return False
        request = Request(self.origin + "/api/health", headers={"X-FotoMaxxer-Session": self.token})
        try:
            with self.opener.open(request, timeout=0.2) as response:
                return json.load(response) == {"status": "ok"}
        except OSError:
            return False

    def request_stop(self) -> None:
        from api.jobs import runner

        runner.begin_shutdown()
        self.server.should_exit = True

    def stopped(self) -> bool:
        return not self.thread.is_alive()

    def wait(self, timeout: float | None = None) -> bool:
        if self.thread.ident is not None:
            self.thread.join(timeout)
        else:
            self.socket.close()
        return self.stopped()
