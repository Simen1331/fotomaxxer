#!/usr/bin/env python3
"""Kjør samme HTTP-scenario mot Python- (FastAPI) og Rust-tjeneren og sammenlign svarene.

Begge tjenerne startes med ``serve`` på hver sin ledige port. Hver får sin egen kopi av
den samme datamappa (``--seed-dir``: ``fotomaxxer.db`` + ``derived/``, kopiert med
bevarte tidsstempler), sin egen ``XDG_CONFIG_HOME`` med en pekerfil til kopien, og sin
egen arbeidsmappe. Ingen originaler røres: alt skrives under en midlertidig mappe.
``HOME`` er en felles, falsk hjemmemappe (for ``/api/fs/list``) med en lenke til
``~/.insightface``, så ansiktsmodellene fortsatt finnes.

For hvert steg sammenlignes statuskode, utvalgte headere (``content-type``,
``content-length``, ``allow``, ``accept-ranges``) og den normaliserte kroppen byte for
byte: stier (``<TMP>``, ``<PAKKE>``, ``<WORK>``, ``<XDG>``, ``<HOME>``, ``<LIB>``),
tidsstempler (``<TS>``), porter (``<PORT>``) og jobb-id-er (``<JOB>``) byttes ut først.
Binære kropper sammenlignes som SHA-256. Når scenarioet er ferdig, stoppes tjenerne med
Ctrl-C (SIGINT; en pågående jobb får gjøre seg ferdig), første utskriftslinje fra
``serve`` sammenlignes, og til slutt hele databasen (``iterdump``) med samme
normalisering som ``compare_cli.py``.

    python3 tools/golden/compare_api.py \\
        --python ~/dev/fotomaxxer/.venv/bin/python \\
        --rust target/release/fotomaxxer \\
        --scenario-file tools/golden/scenarios/api-settings.json \\
        [--scenario-file tools/golden/scenarios/api-fs.json …] \\
        [--seed-dir ~/dev/fotomaxxer-testlib/py-golden] \\
        [--library ~/dev/fotomaxxer-testlib/scanlib] [--mask-errors] [--keep] [--verbose]

``--python`` er Python-tolken i master-venv-en; tjeneren startes via
``py_fotomaxxer.py serve`` (samme kode som ``fotomaxxer serve``), som også setter
OpenRouter-URL-en og ``web/dist`` fra miljøet. ``--web-dist`` (standard: ``web/dist`` i
dette repoet) er SPA-en Python skal servere — samme filer som er bakt inn i Rust-binæren.
OpenRouter-URL-en peker alltid på en lokal adresse: på ``fake_openrouter.py`` når
scenarioet ber om det, ellers på en lukket port. Det ekte, betalte API-et nås aldri.

Scenarioformat (JSON-objekt; trinn B legger bare til nye ``scenarios/api-*.json``)::

    {
      "beskrivelse": "fritekst",
      "seed_dir": "~/dev/fotomaxxer-testlib/py-golden",   // valgfri; --seed-dir overstyrer
      "prep_sql": "UPDATE ...;",                          // valgfri, kjøres på begge kopiene
      "fake_openrouter": true,                            // start fake_openrouter.py
      "mask_errors": true,                                // som --mask-errors i compare_cli
      "mask_sql": ["UPDATE faces SET embedding = NULL"],  // kjøres på kopier før iterdump
      "skip_db": false,                                   // hopp over databasesammenligningen
      "steps": [
        {"name": "valgfritt navn",
         "method": "POST", "path": "/api/sources",
         "query": {"source_id": 1},                       // valgfri (eller rett i path)
         "json": {"path": "{lib}/a"},                     // JSON-kropp (Content-Type settes)
         "body": "rå tekst", "headers": {"X": "y"},       // alternativt: rå kropp + headere
         "body_latin1": "\u00e6",                         // rå kropp kodet som latin-1 (ugyldig UTF-8)
         "wait": true,                                    // vent til jobben er ferdig etterpå
         "fields": ["running", "kind"],                   // sammenlign bare disse JSON-feltene
         "compare": "full" | "status" | "none",           // standard "full"
         "headers_compare": ["etag", "last-modified"],    // headere i tillegg til standardsettet
         "ignore_headers": ["content-length"],
         "expect_status": 409},                           // sjekk at begge svarer dette
        {"wait": true},                                   // bare vent på pågående jobb
        {"sleep": 0.5},
        {"check_files": "{pakke}/derived", "hash": true, "mtime": false},
        {"var": "hash1", "sql": "SELECT content_hash FROM images ORDER BY id LIMIT 1"},
        {"exec_sql": "INSERT INTO albums (name) VALUES ('{impl}')", "name": "valgfritt"}
      ]
    }

Plassholdere i ``path``, ``query``, ``json``, ``body``, ``headers`` og ``check_files``:
``{tmp}`` (felles testmappe), ``{pakke}`` (implementasjonens datamappe ved start),
``{work}`` (implementasjonens arbeidsmappe, tom ved start), ``{home}`` (falsk HOME),
``{lib}`` (``--library``), ``{etag}`` (ETag fra forrige svar fra samme implementasjon) og
``{asset_js}``/``{asset_css}`` (filnavnene i ``web/dist/assets``, som har hash i navnet) og
``{impl}`` (``@py@``/``@rs@``, normaliseres til ``<IMPL>``) for stier i den delte HOME-mappa
som ellers ville kollidert. En ``var``-steg leser en verdi fra implementasjonens egen
database (skrivebeskyttet) og gjør den tilgjengelig som ``{navn}`` — slik slipper scenarioene
å inneholde hasher eller id-er fra private testbibliotek. Et ``exec_sql``-steg kjører SQL
(med plassholdere) mot begge databasene mens tjenerne går, for oppsett API-et ikke har
(album før albumruterne finnes, en original som forsvinner). ``{cwd}`` er tjenerens
arbeidskatalog (relative stier).
Exit 0 = identisk.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import http.client
import json
import os
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
IMPLS = ("py", "rs")  # like lange navn → like lange stier → sammenlignbar content-length
DEFAULT_HEADERS = ("content-type", "content-length", "allow", "accept-ranges")
TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}")
JOB_ID = re.compile(r'"job_id":"[0-9a-f]{12}"')
CLOSED_PORT_URL = "http://127.0.0.1:9/ingen-openrouter"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Impl:
    """Én tjener (Python eller Rust) med sine egne mapper."""

    def __init__(self, name: str, tmp: Path):
        self.name = name
        self.tmp = tmp
        self.pakke = tmp / f"pakke-{name}"
        self.work = tmp / f"work-{name}"
        self.xdg = tmp / f"xdg-{name}"
        self.cwd = tmp / f"cwd-{name}"
        self.port = free_port()
        self.proc: subprocess.Popen | None = None
        self.last_etag = ""
        self.stdout = ""
        self.stderr = ""
        self.vars: dict[str, str] = {}

    def subst(self) -> dict[str, str]:
        return {
            "pakke": str(self.pakke),
            "work": str(self.work),
            "cwd": str(self.cwd),
            "etag": self.last_etag,
            "impl": f"@{self.name}@",
            **self.vars,
        }

    def final_root(self) -> Path:
        """Datamappa etter scenarioet (pekerfila kan ha blitt flyttet)."""
        try:
            data = json.loads((self.xdg / "fotomaxxer" / "config.json").read_text("utf-8"))
            return Path(data["data_dir"])
        except (OSError, ValueError, KeyError):
            return self.pakke


def fill(value, mapping: dict[str, str]):
    """Sett inn plassholdere rekursivt i strenger i en JSON-verdi."""
    if isinstance(value, str):
        for key, repl in mapping.items():
            value = value.replace("{" + key + "}", repl)
        return value
    if isinstance(value, list):
        return [fill(v, mapping) for v in value]
    if isinstance(value, dict):
        return {fill(k, mapping): fill(v, mapping) for k, v in value.items()}
    return value


def make_home(home: Path) -> None:
    """Falsk HOME med et lite mappetre for /api/fs/list."""
    for d in ("Bilder", "bilder 2", "Æbler", "ørret", "a", "B", ".skjult", "Bilder/2011", "Bilder/.cache"):
        (home / d).mkdir(parents=True, exist_ok=True)
    (home / "fil.txt").write_text("ikke en mappe", encoding="utf-8")
    (home / "lenke-til-mappe").symlink_to(home / "a")
    (home / "brutt-lenke").symlink_to(home / "finnes-ikke")
    locked = home / "låst"
    locked.mkdir()
    (locked / "inni").mkdir()
    locked.chmod(0)
    real_models = Path.home() / ".insightface"
    if real_models.exists():
        (home / ".insightface").symlink_to(real_models)


def copy_seed(seed: Path, dst: Path) -> None:
    dst.mkdir(parents=True, exist_ok=True)
    if (seed / "fotomaxxer.db").exists():
        shutil.copy2(seed / "fotomaxxer.db", dst / "fotomaxxer.db")
    if (seed / "derived").is_dir():
        shutil.copytree(seed / "derived", dst / "derived")


def start(impl: Impl, args, env_common: dict[str, str]) -> None:
    impl.work.mkdir()
    impl.cwd.mkdir()
    (impl.xdg / "fotomaxxer").mkdir(parents=True)
    (impl.xdg / "fotomaxxer" / "config.json").write_text(
        json.dumps({"data_dir": str(impl.pakke)}), encoding="utf-8"
    )
    env = {k: v for k, v in os.environ.items() if k not in ("FOTOMAXXER_HOME", "PYTHONPATH")}
    env.update(env_common)
    env["XDG_CONFIG_HOME"] = str(impl.xdg)
    env["PYTHONIOENCODING"] = "utf-8"
    env["PYTHONUNBUFFERED"] = "1"
    if impl.name == "py":
        cmd = [args.python, str(HERE / "py_fotomaxxer.py"), "serve", "--port", str(impl.port)]
        env["FMX_WEB_DIST"] = str(args.web_dist)
    else:
        cmd = [str(args.rust), "serve", "--port", str(impl.port)]
    impl.proc = subprocess.Popen(
        cmd, cwd=impl.cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    deadline = time.time() + 120
    while time.time() < deadline:
        if impl.proc.poll() is not None:
            out, err = impl.proc.communicate()
            raise SystemExit(f"{impl.name}-tjeneren døde ved oppstart:\n{out}\n{err}")
        try:
            status, _, _ = request(impl, "GET", "/api/health")
            if status == 200:
                return
        except OSError:
            pass
        time.sleep(0.2)
    raise SystemExit(f"{impl.name}-tjeneren svarte ikke på /api/health")


def stop(impl: Impl) -> None:
    if impl.proc is None or impl.proc.poll() is not None:
        return
    impl.proc.send_signal(signal.SIGINT)
    try:
        impl.stdout, impl.stderr = impl.proc.communicate(timeout=1800)
    except subprocess.TimeoutExpired:
        impl.proc.kill()
        impl.stdout, impl.stderr = impl.proc.communicate()


def request(impl: Impl, method: str, path: str, body: bytes | None = None, headers=None):
    conn = http.client.HTTPConnection("127.0.0.1", impl.port, timeout=600)
    try:
        conn.request(method, path, body=body, headers=headers or {})
        resp = conn.getresponse()
        data = resp.read()
        hdrs = {k.lower(): v for k, v in resp.getheaders()}
        return resp.status, hdrs, data
    finally:
        conn.close()


def wait_job(impl: Impl, timeout: float) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        _, _, data = request(impl, "GET", "/api/jobs/current")
        if not json.loads(data)["running"]:
            return
        time.sleep(0.2)
    raise SystemExit(f"{impl.name}: jobben ble ikke ferdig innen {timeout} s")


class Normalizer:
    def __init__(self, tmp: Path, impls: dict[str, Impl], home: Path, lib: Path):
        self.tmp, self.impls, self.home, self.lib = tmp, impls, home, lib

    def pairs(self, impl: Impl) -> list[tuple[str, str]]:
        pairs = [
            (str(impl.pakke), "<PAKKE>"),
            (str(impl.work), "<WORK>"),
            (str(impl.xdg), "<XDG>"),
            (str(impl.cwd), "<CWD>"),
            (str(self.home), "<HOME>"),
            (str(self.lib), "<LIB>"),
            (str(self.tmp), "<TMP>"),
        ]
        return sorted(pairs, key=lambda p: -len(p[0]))

    def text(self, impl: Impl, text: str) -> str:
        for path, token in self.pairs(impl):
            text = text.replace(path, token)
        text = text.replace(f"127.0.0.1:{impl.port}", "127.0.0.1:<PORT>").replace(f"@{impl.name}@", "<IMPL>")
        text = JOB_ID.sub('"job_id":"<JOB>"', text)
        return TIMESTAMP.sub("<TS>", text)


def is_text(ctype: str) -> bool:
    return ctype.startswith(("application/json", "text/"))


def pretty(text: str) -> list[str]:
    try:
        return json.dumps(json.loads(text), indent=1, ensure_ascii=False).splitlines()
    except ValueError:
        return text.splitlines()


def run_step(step: dict, impls: dict[str, Impl], norm: Normalizer, common: dict[str, str], verbose: bool):
    """Returnerer (etikett, liste med avvik)."""
    method = step.get("method", "GET").upper()
    results = {}
    label = step.get("name") or f"{method} {step['path']}"
    for name, impl in impls.items():
        mapping = {**common, **impl.subst()}
        path = fill(step["path"], mapping)
        if "query" in step:
            q = fill(step["query"], mapping)
            path += ("&" if "?" in path else "?") + urllib.parse.urlencode(
                {k: ("" if v is None else str(v).lower() if isinstance(v, bool) else v) for k, v in q.items()}
            )
        headers = fill(step.get("headers", {}), mapping)
        body = None
        if "json" in step:
            body = json.dumps(fill(step["json"], mapping), ensure_ascii=False, separators=(",", ":")).encode()
            headers = {"Content-Type": "application/json", **headers}
        elif "body" in step:
            body = fill(step["body"], mapping).encode("utf-8")
        elif "body_latin1" in step:
            body = fill(step["body_latin1"], mapping).encode("latin-1")
        status, hdrs, data = request(impl, method, path, body, headers)
        if "etag" in hdrs:
            impl.last_etag = hdrs["etag"]
        results[name] = (status, hdrs, data)
    if step.get("wait"):
        for impl in impls.values():
            wait_job(impl, step.get("timeout", 3600))

    problems: list[str] = []
    expect = step.get("expect_status")
    if expect is not None:
        for name, (status, _, _) in results.items():
            if status != expect:
                problems.append(f"{name}: forventet status {expect}, fikk {status}")
    mode = step.get("compare", "full")
    if mode == "none":
        return label, problems, results
    (ps, ph, pd), (rs, rh, rd) = results["py"], results["rs"]
    if ps != rs:
        problems.append(f"status: python {ps} ≠ rust {rs}")
    if mode == "status":
        return label, problems, results
    want = [h for h in (*DEFAULT_HEADERS, *step.get("headers_compare", [])) if h not in step.get("ignore_headers", [])]
    for h in want:
        pv, rv = ph.get(h), rh.get(h)
        if h == "content-length" and "fields" in step:
            continue
        if pv != rv:
            problems.append(f"header {h}: python {pv!r} ≠ rust {rv!r}")
    ctype = ph.get("content-type", "")
    if is_text(ctype):
        ptxt = norm.text(impls["py"], pd.decode("utf-8", "replace"))
        rtxt = norm.text(impls["rs"], rd.decode("utf-8", "replace"))
        if "fields" in step:
            pj, rj = json.loads(ptxt), json.loads(rtxt)
            ptxt = json.dumps({k: pj.get(k) for k in step["fields"]}, ensure_ascii=False)
            rtxt = json.dumps({k: rj.get(k) for k in step["fields"]}, ensure_ascii=False)
        if ptxt != rtxt:
            diff = difflib.unified_diff(pretty(ptxt), pretty(rtxt), "python", "rust", lineterm="", n=2)
            problems.append("kropp:\n" + "\n".join("      " + line for line in diff))
            if pretty(ptxt) == pretty(rtxt):
                problems.append(f"  (bare formatering) python: {ptxt[:300]}\n  rust:   {rtxt[:300]}")
        elif verbose:
            print(f"          {ptxt[:200]}")
    elif hashlib.sha256(pd).digest() != hashlib.sha256(rd).digest():
        problems.append(f"binær kropp ulik ({len(pd)} ≠ {len(rd)} byte)")
    return label, problems, results


def list_files(root: Path, with_hash: bool, with_mtime: bool) -> list[str]:
    if not root.exists():
        return ["<finnes ikke>"]
    if root.is_file():
        return ["<fil>"]
    out = []
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root).as_posix()
        if p.is_dir():
            out.append(f"{rel}/")
            continue
        line = f"{rel} {p.stat().st_size}"
        if with_hash:
            line += " " + hashlib.sha256(p.read_bytes()).hexdigest()[:16]
        if with_mtime:
            line += f" {p.stat().st_mtime_ns}"
        out.append(line)
    return out


def dump(db: Path, impl: Impl, norm: Normalizer, mask_errors: bool, mask_sql: list[str]) -> list[str]:
    if not db.exists():
        return ["<ingen database>"]
    masked = db.with_name(db.name + ".sammenlign")
    shutil.copy2(db, masked)
    try:
        with sqlite3.connect(masked) as mconn:
            if mask_errors:
                mconn.execute("UPDATE processing_state SET error = '<feil>' WHERE error IS NOT NULL")
            for sql in mask_sql:
                mconn.execute(sql)
        conn = sqlite3.connect(masked)
        try:
            version = conn.execute("PRAGMA user_version").fetchone()[0]
            return [f"PRAGMA user_version = {version}"] + [norm.text(impl, line) for line in conn.iterdump()]
        finally:
            conn.close()
    finally:
        masked.unlink(missing_ok=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--python", required=True, help="Python-tolken i master-venv-en")
    ap.add_argument("--rust", required=True, type=Path, help="Rust-binæren (fotomaxxer)")
    ap.add_argument(
        "--scenario-file", required=True, type=Path, action="append",
        help="scenario (kan gjentas; hvert kjøres med nye tjenere og ny testmappe)",
    )
    ap.add_argument("--seed-dir", type=Path)
    ap.add_argument("--library", type=Path, default=Path("~/dev/fotomaxxer-testlib/scanlib").expanduser())
    ap.add_argument("--web-dist", type=Path, default=REPO / "web" / "dist")
    ap.add_argument("--mask-errors", action="store_true")
    ap.add_argument("--keep", action="store_true", help="behold testmappa")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()
    args.rust = args.rust.resolve()
    args.web_dist = args.web_dist.resolve()
    results = []
    for path in args.scenario_file:
        failures = run_scenario(args, path)
        results.append((path.name, failures))
    if len(results) > 1:
        print("\nOppsummering:")
        for name, failures in results:
            print(f"  {'IDENTISK' if failures == 0 else f'{failures} avvik':<10} {name}")
    return 0 if all(f == 0 for _, f in results) else 1


def run_scenario(args, scenario_file: Path) -> int:
    scenario = json.loads(scenario_file.read_text(encoding="utf-8"))
    seed = args.seed_dir or (Path(scenario["seed_dir"]).expanduser() if scenario.get("seed_dir") else None)
    mask_errors = args.mask_errors or scenario.get("mask_errors", False)
    print(f"Scenario: {scenario_file.name} — {scenario.get('beskrivelse', '')}")

    tmp = Path(tempfile.mkdtemp(prefix="fmx-api-")).resolve()
    home = tmp / "home"
    make_home(home)
    impls = {name: Impl(name, tmp) for name in IMPLS}
    for impl in impls.values():
        if seed:
            copy_seed(seed, impl.pakke)
        if scenario.get("prep_sql"):
            impl.pakke.mkdir(parents=True, exist_ok=True)
            with sqlite3.connect(impl.pakke / "fotomaxxer.db") as pconn:
                pconn.executescript(scenario["prep_sql"])

    fake = None
    env_common = {"HOME": str(home), "FOTOMAXXER_OPENROUTER_URL": CLOSED_PORT_URL}
    models = Path.home() / ".insightface" / "models" / "buffalo_l"
    if models.is_dir():
        env_common["FOTOMAXXER_MODELS"] = str(models)
    if scenario.get("fake_openrouter"):
        fake = subprocess.Popen(
            [sys.executable, str(HERE / "fake_openrouter.py"), "--port", "0", "--log", str(tmp / "openrouter.jsonl")],
            stdout=subprocess.PIPE, text=True,
        )
        line = fake.stdout.readline().split()
        env_common["FOTOMAXXER_OPENROUTER_URL"] = f"http://127.0.0.1:{line[1]}/api/v1/chat/completions"

    norm = Normalizer(tmp, impls, home, args.library)
    common = {"tmp": str(tmp), "home": str(home), "lib": str(args.library)}
    for ext in ("js", "css"):
        found = sorted((args.web_dist / "assets").glob(f"*.{ext}"))
        common[f"asset_{ext}"] = found[0].name if found else f"mangler.{ext}"
    failures = 0
    try:
        for impl in impls.values():
            start(impl, args, env_common)
        for step in scenario["steps"]:
            if "sleep" in step:
                time.sleep(step["sleep"])
                continue
            if "var" in step:
                for impl in impls.values():
                    db = impl.final_root() / "fotomaxxer.db"
                    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as vconn:
                        row = vconn.execute(step["sql"]).fetchone()
                    impl.vars[step["var"]] = "" if row is None or row[0] is None else str(row[0])
                continue
            if "exec_sql" in step:
                for impl in impls.values():
                    db = impl.final_root() / "fotomaxxer.db"
                    econn = sqlite3.connect(db)
                    try:
                        econn.executescript(fill(step["exec_sql"], {**common, **impl.subst()}))
                    finally:
                        econn.close()
                print(f"  [SQL]   {step.get('name', step['exec_sql'][:70])}")
                continue
            if "check_files" in step:
                listings = {}
                for name, impl in impls.items():
                    root = Path(fill(step["check_files"], {**common, **impl.subst()}))
                    listings[name] = [norm.text(impl, x) for x in list_files(root, step.get("hash", False), step.get("mtime", False))]
                label = f"filer {step['check_files']}"
                if listings["py"] == listings["rs"]:
                    print(f"  [LIK]   {label} ({len(listings['rs'])} oppføringer)")
                else:
                    failures += 1
                    print(f"  [ULIK]  {label}")
                    for line in difflib.unified_diff(listings["py"], listings["rs"], "python", "rust", lineterm="", n=0):
                        print(f"      {line}")
                continue
            if "path" not in step:
                if step.get("wait"):
                    for impl in impls.values():
                        wait_job(impl, step.get("timeout", 3600))
                    print("  [VENT]  jobb ferdig")
                continue
            label, problems, _ = run_step(step, impls, norm, common, args.verbose)
            if problems:
                failures += 1
                print(f"  [ULIK]  {label}")
                for p in problems:
                    print(f"    {p}")
            else:
                print(f"  [LIK]   {label}")
    finally:
        for impl in impls.values():
            stop(impl)
        if fake is not None:
            fake.terminate()
            fake.wait()

    if args.verbose:
        for name, impl in impls.items():
            print(f"  --- stderr fra {name} (siste 40 linjer) ---")
            for line in impl.stderr.splitlines()[-40:]:
                print(f"      {line}")
    first = {name: norm.text(impl, (impl.stdout.splitlines() or [""])[0]) for name, impl in impls.items()}
    if first["py"] == first["rs"]:
        print(f"  [LIK]   serve-utskrift: {first['rs']}")
    else:
        failures += 1
        print(f"  [ULIK]  serve-utskrift\n      python: {first['py']}\n      rust:   {first['rs']}")

    if not scenario.get("skip_db"):
        dumps = {
            name: dump(impl.final_root() / "fotomaxxer.db", impl, norm, mask_errors, scenario.get("mask_sql", []))
            for name, impl in impls.items()
        }
        if dumps["py"] == dumps["rs"]:
            print(f"  [LIK]   database ({len(dumps['rs'])} linjer)")
        else:
            failures += 1
            print("  [ULIK]  database")
            for line in list(difflib.unified_diff(dumps["py"], dumps["rs"], "python", "rust", lineterm="", n=0))[:200]:
                print(f"      {line}")

    (home / "låst").chmod(0o755)
    if args.keep or os.environ.get("FMX_KEEP_TMP"):
        print(f"  (beholdt {tmp})")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    print("IDENTISK" if failures == 0 else f"{failures} avvik")
    return failures


if __name__ == "__main__":
    sys.exit(main())
