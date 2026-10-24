"""Bakgrunnsjobb-kjører: én jobb om gangen.

Single-user → kun **én** kjørende jobb. Forsøk på å starte en til mens en kjører
gir ``JobBusy`` (routeren oversetter til HTTP 409). Hver jobb kjøres i en egen
tråd og åpner sin **egen** SQLite-connection — aldri delt med request-håndtering.

Sesjon 3 bruker dette til eksport-jobben. Sesjon 5 legger «Oppdater biblioteket»-
kjeden og enkelt-stegene oppå samme kjører.
"""

from __future__ import annotations

import threading
import uuid
from collections.abc import Callable
from datetime import datetime, timezone
from typing import Any


class JobBusy(Exception):
    """En jobb kjører allerede (→ HTTP 409)."""


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


class JobRunner:
    """Holder maks én kjørende jobb og dens synlige tilstand."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._closing = False
        self._thread: threading.Thread | None = None
        self._state: dict[str, Any] = {
            "running": False,
            "kind": None,
            "step": None,
            "started_at": None,
            "message": None,
            "progress": {},
        }

    def is_running(self) -> bool:
        with self._lock:
            return bool(self._state.get("running"))

    def is_closing(self) -> bool:
        with self._lock:
            return self._closing

    def begin_shutdown(self) -> None:
        """Steng jobbporten før vi venter; en ny jobb kan ikke snike seg inn."""
        with self._lock:
            self._closing = True

    def wait(self, timeout: float | None = None) -> bool:
        with self._lock:
            thread = self._thread
        if thread is not None:
            thread.join(timeout)
        return thread is None or not thread.is_alive()

    def current(self) -> dict[str, Any]:
        with self._lock:
            return {**self._state, "progress": dict(self._state["progress"])}

    def update(self, **kw: Any) -> None:
        """Oppdater synlig jobb-tilstand (step/message/progress). Kalles fra jobben."""
        with self._lock:
            self._state.update(kw)

    def start(self, kind: str, target: Callable[["JobRunner"], None]) -> str:
        """Start ``target`` i en bakgrunnstråd. Reiser ``JobBusy`` om en kjører.

        ``target`` får runneren og kaller ``update(...)`` for fremdrift. Den må
        åpne sin egen DB-connection.
        """
        with self._lock:
            if self._closing or self.is_running():
                raise JobBusy()
            job_id = uuid.uuid4().hex[:12]
            self._state = {
                "running": True,
                "kind": kind,
                "step": None,
                "started_at": _now_iso(),
                "message": None,
                "progress": {},
                "job_id": job_id,
            }

            def run() -> None:
                try:
                    target(self)
                    with self._lock:
                        if self._state.get("message") is None:
                            self._state["message"] = "Ferdig."
                except Exception as e:  # jobben feiler mykt — synlig i message
                    self.update(message=f"Feil: {type(e).__name__}: {e}")
                finally:
                    self.update(running=False)

            self._thread = threading.Thread(target=run, name=f"job-{kind}", daemon=False)
            self._thread.start()
        return job_id


# Modul-singleton: én kjører for hele prosessen.
runner = JobRunner()
