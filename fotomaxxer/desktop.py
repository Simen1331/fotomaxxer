"""Desktop-launcher: varige data, logging og Qt på hovedtråden."""

from __future__ import annotations

import argparse
import logging
import multiprocessing
import os
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

from . import db, storage
from .resources import frontend_dist


def data_root(explicit: str | None = None) -> tuple[Path, bool]:
    """Eksisterende valg vinner. Returflagget betyr at førstegangsvalg må lagres."""
    if explicit is not None or os.environ.get(storage.ENV_HOME) or storage.read_pointer():
        return storage.resolve_root(explicit), False
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local")
        return (base / "FotoMaxxer").expanduser().resolve(), True
    base = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share")
    return (base / "fotomaxxer").expanduser().resolve(), True


def setup_logging(root: Path) -> Path:
    path = root / "logs" / "desktop.log"
    path.parent.mkdir(parents=True, exist_ok=True)
    handler = RotatingFileHandler(path, maxBytes=2_000_000, backupCount=3, encoding="utf-8")
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))
    logging.getLogger().addHandler(handler)
    logging.getLogger().setLevel(logging.INFO)
    return path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Start FotoMaxxer i eget desktop-vindu.")
    parser.add_argument("--home", help="Datamappe; eksisterende miljøvariabel og pekerfil respekteres.")
    parser.add_argument("--smoke-test", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--smoke-portrait", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.smoke_test is not None:
        if args.smoke_test.exists() or args.smoke_portrait is None or not args.smoke_portrait.is_file():
            parser.error("Røykprøven trenger en ny rapportmappe og et lesbart testportrett.")
        args.smoke_test.mkdir(parents=True)
        if args.home is None:
            args.home = str(args.smoke_test / "library")
    multiprocessing.freeze_support()
    if multiprocessing.get_start_method(allow_none=True) is None:
        multiprocessing.set_start_method("spawn")

    app = server = instance = window = None
    log_path = None
    try:
        from PySide6.QtWidgets import QApplication, QMessageBox
        from .desktop_window import DesktopWindow, SingleInstance
        from .desktop_server import LocalServer

        app = QApplication.instance() or QApplication([sys.argv[0]])
        app.setApplicationName("FotoMaxxer")
        app.setOrganizationName("FotoMaxxer")
        root, first_run = data_root(args.home)
        root.mkdir(parents=True, exist_ok=True)
        instance = SingleInstance(root)
        if not instance.acquire():
            if instance.activate_existing():
                return 0
            raise RuntimeError("FotoMaxxer bruker allerede denne datamappa, men vinduet svarer ikke.")
        log_path = setup_logging(root)
        logging.info("Desktop starter; datamappe: %s", root)
        if not (frontend_dist() / "index.html").is_file():
            raise RuntimeError("Grensesnittet mangler. Bygg web/dist med npm ci og npm run build.")
        store = storage.Storage(root)
        store.ensure_dirs()
        conn = db.connect(store.db_path)
        try:
            db.init_db(conn)
        finally:
            conn.close()
        if first_run:
            storage.write_pointer(root)
        os.environ[storage.ENV_HOME] = str(root)
        server = LocalServer()
        window = DesktopWindow(server, log_path)
        instance.listen(window)
        window.show()
        server.start()
        if args.smoke_test is not None:
            from .desktop_smoke import DesktopSmoke

            window.smoke = DesktopSmoke(window, instance, args.smoke_test, args.smoke_portrait)
        result = app.exec()
        server.request_stop()
        server.wait()
        if args.smoke_test is not None:
            window.smoke.finish()
        return window.exit_code if window.exit_code else result
    except Exception as error:
        logging.exception("Desktop-oppstart feilet")
        message = f"FotoMaxxer kunne ikke starte.\n\n{error}"
        if log_path is not None:
            message += f"\n\nLogg: {log_path}"
        if app is not None:
            from PySide6.QtWidgets import QMessageBox

            QMessageBox.critical(None, "FotoMaxxer", message)
        elif sys.stderr is not None:
            print(message, file=sys.stderr)
        return 1
    finally:
        if server is not None:
            server.request_stop()
            server.wait()
        if instance is not None:
            instance.release()
        if window is not None:
            from shiboken6 import delete, isValid

            window.closing = True
            for item in (window.page, window.view, window.profile, window):
                if isValid(item):
                    delete(item)
        logging.info("Desktop avsluttet")


if __name__ == "__main__":
    raise SystemExit(main())
