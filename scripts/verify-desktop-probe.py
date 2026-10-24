#!/usr/bin/env python3
"""Prøv en flyttet pakke med tom PATH, isolerte data og kontrollert avslutning."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from urllib.request import ProxyHandler, build_opener

REPO = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--face-image", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="Ny testmappe, utenfor repoet.")
    parser.add_argument("--isolate-linux", action="store_true", help="Skjul repo, modeller og system-Perl med bubblewrap; slå av nett for pipeline.")
    parser.add_argument("--desktop", action="store_true", help="Prøv også ekte Qt-vindu, enkeltinstans og lukking under jobb.")
    args = parser.parse_args()
    root = args.output.expanduser().resolve()
    source_bundle = args.bundle.expanduser().resolve()
    if source_bundle == root or source_bundle in root.parents:
        parser.error("Testmappa kan ikke ligge inni pakken som kopieres.")
    if root == REPO or REPO in root.parents:
        parser.error("Testmappa må ligge utenfor repoet.")
    if root.exists():
        parser.error("Testmappa må være ny.")
    bwrap = shutil.which("bwrap") if args.isolate_linux else None
    if args.isolate_linux and (sys.platform != "linux" or not bwrap):
        parser.error("--isolate-linux krever bubblewrap på Linux.")
    root.mkdir(parents=True)
    bundle = root / "FotoMaxxer med æøå"
    shutil.copytree(source_bundle, bundle, symlinks=True)
    portrait = root / "testportrett.png"
    shutil.copy2(args.face_image.resolve(), portrait)
    suffix = ".exe" if sys.platform == "win32" else ""
    executable = bundle / ("FotoMaxxer-cli" + suffix)
    if not executable.is_file():
        executable = bundle / ("FotoMaxxer" + suffix)
    cwd = root / "annen arbeidsmappe"
    cwd.mkdir()
    env = os.environ.copy()
    env["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32") if sys.platform == "win32" else ""
    for name in ("PYTHONPATH", "PYTHONHOME", "FOTOMAXXER_HOME", "PERL5LIB", "PERL5OPT"):
        env.pop(name, None)
    env["XDG_CONFIG_HOME"] = str(root / "config")
    env["XDG_CACHE_HOME"] = str(root / "cache")
    # Qt bruker TMPDIR for enkeltinstans-socketen; /tmp ellers er skrivebeskyttet.
    (root / "tmp").mkdir()
    env["TMPDIR"] = str(root / "tmp")
    env["PYTHONUTF8"] = "1"
    env["PYTHONIOENCODING"] = "utf-8"
    if sys.platform == "win32":
        # Windows bruker disse i stedet for XDG_*/TMPDIR; hold alt inni testmappa.
        for name in ("APPDATA", "LOCALAPPDATA"):
            (root / name).mkdir()
            env[name] = str(root / name)
        env["TEMP"] = env["TMP"] = str(root / "tmp")
    # Den ekte bootstrap-pekerfila skal aldri leses eller skrives.
    prefix = []
    if bwrap:
        prefix = [bwrap, "--die-with-parent", "--ro-bind", "/", "/", "--proc", "/proc", "--dev-bind", "/dev", "/dev", "--bind", str(root), str(root),
                  "--tmpfs", str(REPO), "--chdir", str(cwd)]
        for path in (Path.home() / ".insightface", Path("/usr/share/perl5"), Path("/usr/lib/perl5")):
            if path.exists():
                prefix += ["--tmpfs", str(path)]
        if Path("/usr/bin/perl").exists():
            prefix += ["--ro-bind", "/dev/null", "/usr/bin/perl"]
    process_options = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if sys.platform == "win32" else {"start_new_session": True}
    report = {"isolated_linux": bool(bwrap), "checks": []}

    def check(ok: bool, label: str) -> None:
        report["checks"].append({"ok": bool(ok), "label": label})
        print(f"[{'PASS' if ok else 'FAIL'}] {label}", flush=True)

    def command(*arguments: str, offline: bool = False) -> list[str]:
        isolation = prefix + (["--unshare-net"] if offline and bwrap else [])
        return isolation + [str(executable), *arguments]

    def stop(proc: subprocess.Popen, server_pid: int | None = None) -> None:
        if proc.poll() is None:
            if bwrap and server_pid is not None:
                # Signaler tjeneren, ikke namespace-wrapperen som dreper barna.
                os.kill(server_pid, signal.SIGINT)
            else:
                proc.send_signal(signal.CTRL_BREAK_EVENT if sys.platform == "win32" else signal.SIGINT)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                if sys.platform == "win32":
                    proc.kill()
                else:
                    os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=5)

    try:
        started = time.monotonic()
        version = subprocess.run(command("--version"), cwd=cwd, env=env, capture_output=True, text=True, timeout=30)
        report["version_start_seconds"] = round(time.monotonic() - started, 3)
        check(version.returncode == 0 and "fotomaxxer" in version.stdout, "Executable starter fra flyttet mappe uten utviklings-PATH")
        with (root / "pipeline.log").open("w", encoding="utf-8") as log:
            proc = subprocess.Popen(command("packaging-smoke", "--output", str(root / "pipeline"),
                                            "--face-image", str(portrait), "--workers", "2", offline=True),
                                    cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, **process_options)
            try:
                result = proc.wait(timeout=600)
            finally:
                stop(proc)
        check(result == 0, "Pakket scan/derive/ansikter/klynging avslutter uten feil")
        pipeline_report = root / "pipeline" / "report.json"
        if pipeline_report.is_file():
            report["pipeline"] = json.loads(pipeline_report.read_text(encoding="utf-8"))
            check(report["pipeline"]["ok"] and report["pipeline"]["frozen"], "Røykprøven bekrefter frosset runtime")
        else:
            check(False, "Røykprøvens rapport finnes")

        library = root / "pipeline" / "bibliotek med æøå"
        server_log = root / "server.log"
        started = time.monotonic()
        with server_log.open("w", encoding="utf-8") as log:
            server = subprocess.Popen(command("--home", str(library), "serve", "--port", "0"),
                                      cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, **process_options)
            try:
                deadline = time.monotonic() + 30
                match = None
                while time.monotonic() < deadline and server.poll() is None:
                    match = re.search(r"Uvicorn running on http://127\.0\.0\.1:(\d+)", server_log.read_text(encoding="utf-8", errors="replace"))
                    if match:
                        break
                    time.sleep(0.1)
                if not match:
                    raise RuntimeError("API-tjeneren ble ikke klar; se server.log")
                base = f"http://127.0.0.1:{match[1]}"
                opener = build_opener(ProxyHandler({}))

                def get(path: str) -> bytes:
                    with opener.open(base + path, timeout=10) as response:
                        return response.read()

                check(json.loads(get("/api/health")) == {"status": "ok"}, "Pakket FastAPI helsesjekk")
                report["api_health_seconds"] = round(time.monotonic() - started, 3)
                html = get("/").decode("utf-8")
                asset = re.search(r'src="(/assets/[^\"]+\.js)"', html)
                check(bool(asset) and len(get(asset[1])) > 100, "Pakket React og JavaScript lastes")
                check(get("/albums/1").decode("utf-8") == html, "SPA-ruting fra direkte URL")
                with sqlite3.connect(library / "fotomaxxer.db") as conn:
                    thumb = conn.execute("SELECT path FROM derived LIMIT 1").fetchone()[0]
                check(get("/derived/" + thumb).startswith(b"\xff\xd8"), "Thumbnail-servering fra valgt datamappe")
            finally:
                pid = re.search(r"Started server process \[(\d+)\]", server_log.read_text(encoding="utf-8", errors="replace"))
                stop(server, int(pid[1]) if pid else None)
            # Uvicorn gjenutsender Ctrl-C etter vellykket lifespan-shutdown.
            graceful = "Application shutdown complete." in server_log.read_text(encoding="utf-8", errors="replace")
            exit_codes = {0, -signal.SIGINT, 128 + signal.SIGINT}
            if sys.platform == "win32":
                exit_codes.update({0xC000013A, -1073741510, 3})  # 3: uvicorn re-sender Ctrl+Break etter ren shutdown
            print(f"  serve: graceful={graceful} returncode={server.returncode}", flush=True)
            check(graceful and server.returncode in exit_codes,
                  "API-tjeneren avslutter kontrollert")
        if args.desktop:
            desktop = bundle / ("FotoMaxxer" + suffix)
            desktop_env = env.copy()
            desktop_env["QT_QPA_PLATFORMTHEME"] = ""
            desktop_env["QTWEBENGINE_CHROMIUM_FLAGS"] = "--disable-gpu"
            desktop_command = prefix + (["--unshare-net"] if bwrap else []) + [
                str(desktop), "--home", str(library), "--smoke-test", str(root / "desktop"),
                "--smoke-portrait", str(portrait),
            ]
            with (root / "desktop.log").open("w", encoding="utf-8") as log:
                gui = subprocess.Popen(desktop_command, cwd=cwd, env=desktop_env,
                                       stdout=log, stderr=subprocess.STDOUT, **process_options)
                try:
                    result = gui.wait(timeout=600)
                finally:
                    stop(gui)
            check(result == 0, "Qt-executable med enkeltinstans og lukking under jobb")
            gui_report = root / "desktop" / "desktop-report.json"
            if gui_report.is_file():
                report["desktop"] = json.loads(gui_report.read_text(encoding="utf-8"))
                check(report["desktop"]["ok"] and report["desktop"]["frozen"], "Qt-røykprøvens rapport bekrefter frosset runtime")
            else:
                check(False, "Qt-røykprøvens rapport finnes")
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        print(report["error"], file=sys.stderr)
    report["bundle_bytes"] = sum(p.stat().st_size for p in bundle.rglob("*") if p.is_file() and not p.is_symlink())
    if sys.platform == "linux":
        import resource

        # Største enkeltprosess, ikke summen av samtidige arbeidere.
        report["largest_child_peak_rss_kib"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    report["ok"] = "error" not in report and all(c["ok"] for c in report["checks"])
    (root / "verification.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Rapport og logger: {root}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
