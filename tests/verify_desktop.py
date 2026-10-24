"""Desktop-kontrakter: bootstrap, API-tilgang og avslutning under en jobb."""

import asyncio
import os
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from api.jobs import JobBusy, JobRunner
from fotomaxxer import db, storage
from fotomaxxer.desktop import data_root


class DesktopTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fmx-desktop-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.enterContext(patch.dict(os.environ, {
            "XDG_CONFIG_HOME": str(self.root / "config"), "XDG_DATA_HOME": str(self.root / "data"),
            "LOCALAPPDATA": str(self.root / "windows-data"),
        }))
        os.environ.pop(storage.ENV_HOME, None)

    def test_first_launch_uses_persistent_data_instead_of_cwd(self):
        with patch.object(sys, "platform", "linux"):
            self.assertEqual(data_root(), (self.root / "data" / "fotomaxxer", True))
        with patch.object(sys, "platform", "win32"):
            self.assertEqual(data_root(), (self.root / "windows-data" / "FotoMaxxer", True))

    def test_existing_pointer_env_and_explicit_home_keep_precedence(self):
        existing = self.root / "eksisterende æøå"
        storage.write_pointer(existing)
        self.assertEqual(data_root(), (existing, False))
        os.environ[storage.ENV_HOME] = str(self.root / "env")
        self.assertEqual(data_root(), (self.root / "env", False))
        self.assertEqual(data_root(str(self.root / "explicit")), (self.root / "explicit", False))

    def test_invalid_pointer_is_not_used_as_a_path(self):
        path = storage.config_path()
        path.parent.mkdir(parents=True)
        path.write_text('{"data_dir": ["ikke", "en sti"]}')
        self.assertIsNone(storage.read_pointer())

    def test_closing_prevents_new_jobs_and_waits_for_existing_job(self):
        runner = JobRunner()
        started, release = threading.Event(), threading.Event()

        def job(run):
            started.set()
            release.wait(5)

        runner.start("test", job)
        try:
            self.assertTrue(started.wait(2))
            runner.begin_shutdown()
            with self.assertRaises(JobBusy):
                runner.start("next", lambda run: None)
            self.assertFalse(runner.wait(0.01))
        finally:
            release.set()
            self.assertTrue(runner.wait(2))
        self.assertFalse(runner.is_running())

    def _app(self):
        os.environ[storage.ENV_HOME] = str(self.root / "library")
        store = storage.Storage(storage.resolve_root())
        store.ensure_dirs()
        conn = db.connect(store.db_path)
        db.init_db(conn)
        conn.close()
        from api.app import create_app

        return create_app(session_token="test-token", origin="http://127.0.0.1:40000")

    def test_desktop_api_rejects_token_host_and_origin_before_mutations(self):
        app = self._app()

        async def verify():
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app), base_url="http://127.0.0.1:40000") as client:
                for headers in ({}, {"X-FotoMaxxer-Session": "wrong"},
                                {"X-FotoMaxxer-Session": "test-token", "Host": "evil.test"},
                                {"X-FotoMaxxer-Session": "test-token", "Origin": "https://evil.test"},
                                {"X-FotoMaxxer-Session": "test-token", "Sec-Fetch-Site": "cross-site"}):
                    response = await client.post("/api/sources", json={"path": str(self.root)}, headers=headers)
                    self.assertEqual(response.status_code, 403)
                health = await client.get("/api/health", headers={"X-FotoMaxxer-Session": "test-token"})
                self.assertEqual(health.json(), {"status": "ok"})
                missing = await client.get("/api/health")
                self.assertEqual(missing.status_code, 403)
                index = await client.get("/", headers={"X-FotoMaxxer-Session": "test-token"})
                self.assertEqual(index.status_code, 200)
                move = await client.post("/api/maintenance/data-dir", json={"path": str(self.root / "unsafe-move")},
                                         headers={"X-FotoMaxxer-Session": "test-token"})
                self.assertEqual(move.status_code, 409)
                self.assertFalse((self.root / "unsafe-move").exists())

        asyncio.run(verify())
        conn = db.connect(storage.Storage(storage.resolve_root()).db_path)
        try:
            self.assertEqual(conn.execute("SELECT COUNT(*) FROM sources").fetchone()[0], 0)
        finally:
            conn.close()

    def test_threaded_server_stays_alive_until_job_connection_is_closed(self):
        self._app()
        from fotomaxxer.desktop_server import LocalServer

        runner = JobRunner()
        self.enterContext(patch("api.jobs.runner", runner))
        self.enterContext(patch("api.app.runner", runner))
        self.enterContext(patch("api.desktop_access.runner", runner))
        server = LocalServer()
        release = threading.Event()
        closed = threading.Event()
        started = threading.Event()

        def job(run):
            conn = db.connect(storage.Storage(storage.resolve_root()).db_path)
            try:
                started.set()
                release.wait(5)
                conn.execute("INSERT INTO settings VALUES ('desktop_shutdown_test', 'done')")
                conn.commit()
            finally:
                conn.close()
                closed.set()

        server.start()
        try:
            deadline = time.monotonic() + 5
            while not server.ready() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(server.ready())
            with self.assertRaises(HTTPError) as rejected:
                server.opener.open(server.origin + "/api/health")
            self.assertEqual(rejected.exception.code, 403)
            rejected.exception.close()
            runner.start("test", job)
            self.assertTrue(started.wait(2))
            server.request_stop()
            self.assertFalse(server.wait(0.2))
            with self.assertRaises(JobBusy):
                runner.start("next", lambda run: None)
            self.assertFalse(closed.is_set())
        finally:
            release.set()
            server.request_stop()
            self.assertTrue(server.wait(5))
        self.assertTrue(closed.is_set())
        self.assertIsNone(server.error)
        conn = db.connect(storage.Storage(storage.resolve_root()).db_path)
        try:
            self.assertEqual(conn.execute("SELECT value FROM settings WHERE key='desktop_shutdown_test'").fetchone()[0], "done")
        finally:
            conn.close()


if __name__ == "__main__":
    unittest.main()
