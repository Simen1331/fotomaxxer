"""FotoMaxxer CLI — argparse-entrypoint.

Sesjon 1: ``init``. Sesjon 2: ``source add``/``source list`` + ``scan``.
Sesjon 3: ``derive``. Sesjon 4: ``source remove`` + ``prepare-handoff``.
Hele CLI-en er nå wiret; GUI kaller de samme funksjonene senere.
"""

from __future__ import annotations

import argparse
import sys

from . import __version__, db, settings as settings_mod, sources, storage
from .derive import derive as derive_images
from .exif import ExifReader
from .geocode import Geocoder
from .scan import scan_source
from .faces import cluster as cluster_faces
from .faces import detect as detect_faces
from .tagging import TaggingError
from .tagging import tag as tag_images


def _open(args: argparse.Namespace) -> tuple[storage.Storage, "db.sqlite3.Connection"]:
    """Resolv pakke-rota og åpne DB-en med skjemaet på plass."""
    store = storage.Storage(storage.resolve_root(args.home))
    conn = db.connect(store.db_path)
    db.init_db(conn)
    return store, conn


def cmd_init(args: argparse.Namespace) -> int:
    root = storage.resolve_root(args.home)
    store = storage.Storage(root)
    store.ensure_dirs()
    conn = db.connect(store.db_path)
    try:
        db.init_db(conn)
        tables = db.table_names(conn)
    finally:
        conn.close()
    print(f"Initialiserte FotoMaxxer-pakke i {root}")
    print(f"  database: {store.db_path}")
    print(f"  derived/: {store.derived_dir}")
    print(f"  tabeller ({len(tables)}): {', '.join(tables)}")
    return 0


def cmd_source_add(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        try:
            row = sources.add(conn, args.path)
        except sources.SourceError as e:
            print(f"Feil: {e}", file=sys.stderr)
            return 1
    finally:
        conn.close()
    print(f"La til kilde #{row['id']}: {row['root_path']}")
    return 0


def cmd_source_list(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        rows = sources.list_sources(conn)
    finally:
        conn.close()
    if not rows:
        print("Ingen kilder lagt til ennå.")
        return 0
    for r in rows:
        scanned = r["last_scanned_at"] or "aldri"
        print(f"#{r['id']}  {r['root_path']}  (filer={r['file_count']}, scannet={scanned})")
    return 0


def cmd_scan(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        if args.source:
            row = sources.get_by_path(conn, args.source)
            if row is None:
                print(f"Ukjent kilde: {args.source} — legg den til med 'source add' først.",
                      file=sys.stderr)
                return 1
            targets = [row]
        elif args.all:
            targets = sources.list_sources(conn)
        else:
            print("Angi --source PATH eller --all.", file=sys.stderr)
            return 2
        if not targets:
            print("Ingen kilder å scanne.")
            return 0

        geocoder = Geocoder()
        with ExifReader() as exif:
            for row in targets:
                print(f"Scanner #{row['id']}: {row['root_path']} …")
                stats = scan_source(conn, row, exif, geocoder)
                print(f"  {stats.summary()}")
    finally:
        conn.close()
    return 0


def cmd_derive(args: argparse.Namespace) -> int:
    store, conn = _open(args)
    try:
        stats = derive_images(conn, store, workers=args.workers)
    finally:
        conn.close()
    print(stats.summary())
    return 0


def cmd_settings_set(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        settings_mod.set_setting(conn, args.key, args.value)
    finally:
        conn.close()
    print(f"Satt {args.key} = {settings_mod.mask_value(args.key, args.value)}")
    return 0


def cmd_settings_list(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        items = settings_mod.all_settings(conn)
    finally:
        conn.close()
    if not items:
        print("Ingen settings satt.")
        return 0
    for key in sorted(items):
        print(f"  {key} = {settings_mod.mask_value(key, items[key])}")
    return 0


def cmd_tag(args: argparse.Namespace) -> int:
    store, conn = _open(args)
    try:
        try:
            stats = tag_images(
                conn,
                store,
                limit=args.limit,
                workers=args.workers,
                retag=args.retag,
                assume_yes=args.yes,
            )
        except TaggingError as e:
            print(f"Feil: {e}", file=sys.stderr)
            return 1
    finally:
        conn.close()
    print(stats.summary())
    return 0


def cmd_faces_detect(args: argparse.Namespace) -> int:
    store, conn = _open(args)
    try:
        stats = detect_faces(conn, store, workers=args.workers, retry_errors=args.retry_errors)
    finally:
        conn.close()
    print(stats.summary())
    return 0


def cmd_faces_cluster(args: argparse.Namespace) -> int:
    _store, conn = _open(args)
    try:
        stats = cluster_faces(conn, full=args.full, threshold=args.threshold)
    finally:
        conn.close()
    print(stats.summary())
    return 0


def cmd_source_remove(args: argparse.Namespace) -> int:
    store, conn = _open(args)
    try:
        try:
            stats = sources.remove(conn, store, args.target)
        except sources.SourceError as e:
            print(f"Feil: {e}", file=sys.stderr)
            return 1
    finally:
        conn.close()
    print(f"Fjernet kilde. {stats.summary()}")
    print("Kilde-filer på disk er urørt.")
    return 0


def cmd_handoff(args: argparse.Namespace) -> int:
    store, conn = _open(args)
    try:
        stats = sources.prepare_for_handoff(conn, store)
    finally:
        conn.close()
    print(f"Pakka er klar for deling. {stats.summary()}")
    print("Settings tømt (inkl. evt. API-nøkkel). Kilde-filer på disk er urørt.")
    return 0


def cmd_serve(args: argparse.Namespace) -> int:
    """Start det lokale web-GUI-et (FastAPI på uvicorn).

    ``api/`` og ``web/`` er søsken-mapper utenfor Python-pakka. Vi legger repo-
    rota (mappa over ``fotomaxxer/``) på ``sys.path`` + ``PYTHONPATH`` slik at
    ``api.app`` kan importeres — også i uvicorns reload-subprosess.
    """
    import os
    from pathlib import Path

    if getattr(sys, "frozen", False) and args.reload:
        print("Feil: --reload er bare tilgjengelig i utviklingsstarten.", file=sys.stderr)
        return 2

    repo_root = Path(__file__).resolve().parent.parent
    root_str = str(repo_root)
    if root_str not in sys.path:
        sys.path.insert(0, root_str)
    os.environ["PYTHONPATH"] = root_str + os.pathsep + os.environ.get("PYTHONPATH", "")
    # La --home virke for serve: peker FOTOMAXXER_HOME hvis gitt.
    if args.home:
        os.environ["FOTOMAXXER_HOME"] = str(Path(args.home).expanduser().resolve())

    try:
        import uvicorn
    except ImportError:
        print(
            "Feil: uvicorn/fastapi er ikke installert. Kjør 'pip install -e .' på nytt.",
            file=sys.stderr,
        )
        return 1

    print(f"FotoMaxxer GUI på http://{args.host}:{args.port}  (Ctrl-C for å stoppe)")
    uvicorn.run(
        "api.app:app",
        host=args.host,
        port=args.port,
        reload=args.reload,
        reload_dirs=[str(repo_root / "api")] if args.reload else None,
    )
    return 0


def cmd_packaging_smoke(args: argparse.Namespace) -> int:
    from .packaging_probe import run

    return run(args.output, args.face_image, args.workers)


def cmd_desktop(args: argparse.Namespace) -> int:
    from .desktop import main as desktop_main

    return desktop_main(["--home", args.home] if args.home is not None else [])


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="fotomaxxer",
        description="Lokal-først, bærbar foto-kuratering for trykte album.",
    )
    parser.add_argument("--version", action="version", version=f"fotomaxxer {__version__}")
    parser.add_argument(
        "--home",
        default=None,
        metavar="DIR",
        help="Pakke-rota (mappa med fotomaxxer.db). "
        "Standard: $FOTOMAXXER_HOME, ellers gjeldende mappe.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_init = sub.add_parser("init", help="Opprett pakke + database-skjema (idempotent).")
    p_init.set_defaults(func=cmd_init)

    p_desktop = sub.add_parser("desktop", help="Start FotoMaxxer i eget vindu.")
    p_desktop.set_defaults(func=cmd_desktop)

    p_probe = sub.add_parser("packaging-smoke", help="Prøv pakket pipeline i et isolert testbibliotek.")
    p_probe.add_argument("--output", required=True, metavar="DIR", help="Ny, tom testmappe.")
    p_probe.add_argument("--face-image", required=True, metavar="FILE", help="Godkjent testbilde med minst ett ansikt.")
    p_probe.add_argument("--workers", type=int, default=2, metavar="N")
    p_probe.set_defaults(func=cmd_packaging_smoke)

    p_scan = sub.add_parser("scan", help="Scan kilde(r) → images/files.")
    g = p_scan.add_mutually_exclusive_group()
    g.add_argument("--source", metavar="PATH", help="Scan kun denne kilden.")
    g.add_argument("--all", action="store_true", help="Scan alle kilder.")
    p_scan.set_defaults(func=cmd_scan)

    p_derive = sub.add_parser("derive", help="Lag nedskalerte arbeidskopier (1024 + 300).")
    p_derive.add_argument(
        "--workers", type=int, default=None, metavar="N",
        help="Antall parallelle prosesser (standard: CPU-kjerner).",
    )
    p_derive.set_defaults(func=cmd_derive)

    p_settings = sub.add_parser("settings", help="Les/skriv bruker-konfig (nøkkel, modell, terskler).")
    set_sub = p_settings.add_subparsers(dest="settings_command", required=True)
    p_set = set_sub.add_parser("set", help="Skriv en setting.")
    p_set.add_argument("key", help="f.eks. openrouter_api_key, ai_model, face_match_threshold.")
    p_set.add_argument("value", help="Verdien.")
    p_set.set_defaults(func=cmd_settings_set)
    p_slist = set_sub.add_parser("list", help="Vis settings (api-nøkkel maskeres).")
    p_slist.set_defaults(func=cmd_settings_list)

    p_tag = sub.add_parser("tag", help="AI-merk pending bilder via OpenRouter vision.")
    p_tag.add_argument(
        "--limit", type=int, default=None, metavar="N",
        help="Maks antall bilder denne kjøringen (0/utelatt = alle pending).",
    )
    p_tag.add_argument(
        "--workers", type=int, default=None, metavar="N",
        help="Antall parallelle tråder (standard: 6).",
    )
    p_tag.add_argument(
        "--retag", action="store_true",
        help="Nullstill ai-state og merk på nytt (overskriver eksisterende ai_tags).",
    )
    p_tag.add_argument(
        "--yes", action="store_true",
        help="Hopp over kostnads-bekreftelsen for store batcher.",
    )
    p_tag.set_defaults(func=cmd_tag)

    p_faces = sub.add_parser("faces", help="Ansikts-deteksjon og klynging til personer.")
    faces_sub = p_faces.add_subparsers(dest="faces_command", required=True)
    p_detect = faces_sub.add_parser("detect", help="Detektér ansikter + embeddings på pending bilder.")
    p_detect.add_argument(
        "--workers", type=int, default=None, metavar="N",
        help="Antall parallelle prosesser (standard: CPU-kjerner).",
    )
    p_detect.add_argument(
        "--retry-errors", action="store_true",
        help="Ta også med bilder som tidligere feilet.",
    )
    p_detect.set_defaults(func=cmd_faces_detect)
    p_cluster = faces_sub.add_parser("cluster", help="Klyng ansikter til personer (inkrementell | full).")
    p_cluster.add_argument(
        "--full", action="store_true",
        help="Full re-klynging av alt + re-mapping av navn (sjelden).",
    )
    p_cluster.add_argument(
        "--threshold", type=float, default=None, metavar="T",
        help="Cosinus-terskel for tilordning (standard: settings face_match_threshold).",
    )
    p_cluster.set_defaults(func=cmd_faces_cluster)

    p_source = sub.add_parser("source", help="Administrer kilde-mapper.")
    src_sub = p_source.add_subparsers(dest="source_command", required=True)
    p_add = src_sub.add_parser("add", help="Legg til en kilde-mappe.")
    p_add.add_argument("path", help="Absolutt sti til mappa.")
    p_add.set_defaults(func=cmd_source_add)
    p_list = src_sub.add_parser("list", help="List innlagte kilder.")
    p_list.set_defaults(func=cmd_source_list)
    p_remove = src_sub.add_parser(
        "remove", help="Fjern en kilde + dens app-data, aldri kilde-filer."
    )
    p_remove.add_argument("target", help="Sti eller id til kilden.")
    p_remove.set_defaults(func=cmd_source_remove)

    p_handoff = sub.add_parser(
        "prepare-handoff", help="Tøm pakka for deling (fjern alle kilder + settings)."
    )
    p_handoff.set_defaults(func=cmd_handoff)

    p_serve = sub.add_parser("serve", help="Start det lokale web-GUI-et (FastAPI på uvicorn).")
    p_serve.add_argument("--host", default="127.0.0.1", help="Bind-adresse (standard: 127.0.0.1).")
    p_serve.add_argument("--port", type=int, default=8000, help="Port (standard: 8000).")
    p_serve.add_argument(
        "--reload", action="store_true", help="Auto-reload ved kodeendring (utvikling)."
    )
    p_serve.set_defaults(func=cmd_serve)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
