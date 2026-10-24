#!/usr/bin/env python3
"""Sammenlign AI-merkingen (``tag``) i Python- og Rust-CLI-en mot en falsk OpenRouter-tjener.

Startdata er en ferdig datamappe (``--seed-dir``, f.eks. ``~/dev/fotomaxxer-testlib/py-golden``)
med derivater. Skriptet velger et utvalg bilder slik at alle svarvariantene i
``fake_openrouter.py`` dekkes, lager profiler og kilder for alle tre promptstiene, sletter
ett 1024-derivat (myk feil) og bygger en forberedt startmappe. Deretter:

1. **Parallell kjøring** (standard 6 tråder, rekkefølgen er ikke-deterministisk i Python
   heller): stegene i ``--scenario-file`` (``{"argv": [...], "stdin": "..."}``) kjøres mot
   begge implementasjonene. Sammenligner (a) forespørslene per bilde fra tjenerloggen
   (kropp byte for byte, headere, antall forsøk), (b) databasen matchet på ``image_id`` og
   innhold i stedet for rad-id-er, og (c) exit-kode/stdout/stderr per steg.
2. **Deterministisk kjøring** (``--exact-scenario``, ``--workers 1``): kaller
   ``compare_cli.py`` med samme startmappe, så hele databasen (``iterdump``) må være lik.

    python3 tools/golden/compare_tagging.py \\
        --python tools/golden/py_fotomaxxer.py --rust target/release/fotomaxxer \\
        --seed-dir ~/dev/fotomaxxer-testlib/py-golden

``FMX_KEEP_TMP=1`` beholder den midlertidige mappa. Exit 0 = identisk.
"""

from __future__ import annotations

import argparse
import collections
import difflib
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from compare_cli import normalize  # noqa: E402
from fake_openrouter import RARE, VARIANTS, variant_for  # noqa: E402

PROFILES = [
    # (id, navn, fokus, categories-JSON) — dekker alle tre promptstiene + rare verdier.
    (1, "Håndverk", "Design og håndverk", '["materiale", "teknikk", "stil"]'),
    (2, "Natur", "Fugler og natur", "[]"),
    (3, "Interiør", "  Interiør og møbler  ", '[" møbler ", "", "farge", 5]'),
    (4, "Blank", "   ", '["ignoreres"]'),
    (5, "Ødelagt", "Arkitektur", "ikke json"),
]
# Kilde-id → profil-id. Kilde 1 finnes i fasit-databasen og har ingen profil.
SOURCES = {2: 1, 3: 2, 4: 4, 5: 5, 6: None}
# image_id % 7 → kilder bildet får en (ekstra) fil i. Laveste kilde med profil vinner.
FILE_RULE = {0: [2], 1: [3], 2: [3, 2], 3: [4], 4: [5], 5: [6, 3], 6: []}
TABLES = ("ai_tags", "tags", "image_tags", "processing_state")


def sha_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def choose(seed: Path, per_variant: int, universe_size: int):
    conn = sqlite3.connect(seed / "fotomaxxer.db")
    rows = conn.execute(
        "SELECT i.id, i.content_hash FROM images i JOIN processing_state p "
        "ON p.image_id = i.id AND p.stage = 'derive' AND p.status = 'done' ORDER BY i.id"
    ).fetchall()
    conn.close()
    by_variant: dict[str, list[int]] = collections.defaultdict(list)
    hashes = {}
    for image_id, content_hash in rows:
        f = seed / "derived" / f"{content_hash}_1024.jpg"
        if not f.exists():
            continue
        hashes[image_id] = content_hash
        by_variant[variant_for(sha_file(f))].append(image_id)
    pending: list[int] = []
    for v in VARIANTS:
        pending += by_variant[v][:per_variant]
    for v in RARE.values():
        pending += by_variant[v][:1]
    missing = next(i for i in by_variant["flat"][per_variant:])  # 1024-derivatet slettes
    pending.append(missing)
    extras = [
        i
        for v in VARIANTS
        for i in by_variant[v][per_variant + (1 if v == "flat" else 0) :]
    ]
    extras.sort()
    universe = sorted(pending + extras[: max(0, universe_size - len(pending))])
    counts = {v: len(by_variant[v]) for v in [*VARIANTS, *RARE.values()]}
    return sorted(pending), universe, missing, hashes, counts


def prep_sql(pending: list[int], universe: list[int]) -> str:
    u = ", ".join(map(str, universe))
    p = ", ".join(map(str, pending))
    lines = [
        f"UPDATE processing_state SET status = 'pending' WHERE stage = 'derive' AND image_id NOT IN ({u});",
        f"UPDATE processing_state SET status = 'done' WHERE stage = 'ai' AND image_id IN ({u}) "
        f"AND image_id NOT IN ({p});",
    ]
    for pid, name, focus, cats in PROFILES:
        q = lambda s: "'" + s.replace("'", "''") + "'"  # noqa: E731
        lines.append(
            f"INSERT INTO tagging_profiles (id, name, focus, categories) VALUES ({pid}, {q(name)}, {q(focus)}, {q(cats)});"
        )
    for sid, pid in SOURCES.items():
        lines.append(
            f"INSERT INTO sources (id, root_path, profile_id) VALUES ({sid}, '/falsk/kilde-{sid}', "
            f"{'NULL' if pid is None else pid});"
        )
    for image_id in universe:
        for sid in FILE_RULE[image_id % 7]:
            lines.append(
                f"INSERT INTO files (image_id, source_id, path) VALUES ({image_id}, {sid}, "
                f"'/falsk/kilde-{sid}/bilde-{image_id}.jpg');"
            )
    return "\n".join(lines) + "\n"


def build_seed(seed: Path, out: Path, sql: str, universe: list[int], missing: int, hashes: dict) -> None:
    out.mkdir()
    shutil.copy2(seed / "fotomaxxer.db", out / "fotomaxxer.db")
    with sqlite3.connect(out / "fotomaxxer.db") as conn:
        conn.executescript(sql)
    (out / "derived").mkdir()
    for image_id in universe:
        for size in ("1024", "300"):
            if image_id == missing and size == "1024":
                continue
            name = f"{hashes[image_id]}_{size}.jpg"
            os.symlink(seed / "derived" / name, out / "derived" / name)


def start_server(log: Path) -> tuple[subprocess.Popen, str]:
    proc = subprocess.Popen(
        [sys.executable, str(HERE / "fake_openrouter.py"), "--port", "0", "--log", str(log)],
        stdout=subprocess.PIPE,
        text=True,
    )
    line = proc.stdout.readline().split()
    assert line[0] == "LYTTER", line
    return proc, f"http://127.0.0.1:{line[1]}/api/v1/chat/completions"


def run_impl(exe: str, home: Path, xdg: Path, argv: list[str], stdin: str | None, env_extra: dict):
    env = {k: v for k, v in os.environ.items() if k != "FOTOMAXXER_HOME"}
    env.update(env_extra, XDG_CONFIG_HOME=str(xdg), PYTHONIOENCODING="utf-8")
    t0 = time.monotonic()
    proc = subprocess.run(
        [exe, "--home", str(home), *argv],
        input=stdin if stdin is not None else "",
        capture_output=True,
        text=True,
        env=env,
        timeout=3600,
    )
    return (proc.returncode, proc.stdout, proc.stderr), time.monotonic() - t0


def db_content(db: Path, tmp: Path, home: Path) -> dict:
    conn = sqlite3.connect(db)
    out: dict = {}
    out["ai_tags"] = {
        r[0]: r[1:]
        for r in conn.execute(
            "SELECT image_id, provider, scene, event_type, season_ai, indoor_outdoor, person_count, "
            "description, raw_json, cost FROM ai_tags"
        )
    }
    n_ai = conn.execute("SELECT COUNT(*) FROM ai_tags").fetchone()[0]
    out["ai_tags_dupes"] = n_ai - len(out["ai_tags"])
    out["state"] = {
        (r[0], r[1]): (r[2], r[3])
        for r in conn.execute("SELECT image_id, stage, status, error FROM processing_state")
    }
    links: dict[int, list] = collections.defaultdict(list)
    for image_id, name, cat in conn.execute(
        "SELECT it.image_id, t.name, t.category FROM image_tags it JOIN tags t ON t.id = it.tag_id"
    ):
        links[image_id].append((name, cat))
    out["image_tags"] = {k: sorted(v, key=repr) for k, v in links.items()}
    out["tags"] = sorted(conn.execute("SELECT name, category FROM tags").fetchall(), key=repr)
    skip = tuple(f'INSERT INTO "{t}"' for t in TABLES)
    out["rest"] = [normalize(line, tmp, home) for line in conn.iterdump() if not line.startswith(skip)]
    conn.close()
    return out


def compare_dicts(label: str, a: dict, b: dict, failures: list[str], show: int = 5) -> int:
    keys = sorted(set(a) | set(b), key=repr)
    diff = [k for k in keys if a.get(k) != b.get(k)]
    status = "LIK" if not diff else "ULIK"
    print(f"  [{status}]  {label}: {len(keys) - len(diff)}/{len(keys)} like")
    for k in diff[:show]:
        print(f"      {k!r}\n        python: {a.get(k)!r}\n        rust:   {b.get(k)!r}")
    if diff:
        failures.append(label)
    return len(keys) - len(diff)


def compare_requests(log: Path, failures: list[str]) -> None:
    per: dict[str, dict[str, list]] = {"python": collections.defaultdict(list), "rust": collections.defaultdict(list)}
    records = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
    bad = [r for r in records if "feil" in r or r["impl"] not in per]
    if bad:
        failures.append("forespørsler (ugyldige)")
        print(f"  [ULIK]  {len(bad)} ugyldige forespørsler, f.eks. {bad[0]}")
    for r in records:
        if r["impl"] in per and "img_sha" in r:
            per[r["impl"]][r["img_sha"]].append(r)
    fields = ("path", "body_sha", "body_ascii", "headers", "model", "temperature", "response_format",
              "usage", "keys", "role", "content_types", "data_url_prefix", "prompt", "status", "variant")

    def sig(rs: list) -> list:
        # Samme bilde kan merkes flere ganger (retag) med ulik prompt; sammenlign multimengden.
        return sorted((json.dumps([r[f] for f in fields], ensure_ascii=False), r["attempt"]) for r in rs)

    a = {k: sig(v) for k, v in per["python"].items()}
    b = {k: sig(v) for k, v in per["rust"].items()}
    n_py = sum(len(v) for v in per["python"].values())
    n_rs = sum(len(v) for v in per["rust"].values())
    print(f"  forespørsler: python={n_py} rust={n_rs}, bilder={len(set(a) | set(b))}")
    compare_dicts("forespørsler per bilde (kropp byte for byte, headere, forsøk)", a, b, failures, show=2)
    prompts = collections.Counter(r["prompt"] for r in records if "prompt" in r and r["impl"] == "rust")
    base = [p for p in prompts if not p.startswith("Fokusområde:")]
    flat = [p for p in prompts if "3–12 frie stikkord" in p]
    cat = [p for p in prompts if "<grunnform>" in p]
    print(f"  promptstier (rust): grunnprompt={len(base)} flat fokus={len(flat)} kategorier={len(cat)} "
          f"(ulike prompter totalt {len(prompts)})")
    if not (base and flat and cat):
        failures.append("promptstier")
    statuses = collections.Counter((r["variant"], r["status"]) for r in records if r.get("impl") == "rust")
    print("  varianter (rust, variant/status → forespørsler): " +
          ", ".join(f"{v}/{s}={n}" for (v, s), n in sorted(statuses.items())))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--seed-dir", type=Path, required=True)
    ap.add_argument("--scenario-file", type=Path, default=HERE / "scenarios" / "tagging_parallel.json")
    ap.add_argument("--exact-scenario", type=Path, default=HERE / "scenarios" / "tagging.json")
    ap.add_argument("--skip-exact", action="store_true")
    ap.add_argument("--per-variant", type=int, default=4)
    ap.add_argument("--universe", type=int, default=205)
    args = ap.parse_args()
    python_exe, rust_exe = str(Path(args.python).resolve()), str(Path(args.rust).resolve())

    tmp = Path(tempfile.mkdtemp(prefix="fmx-tag-")).resolve()
    pending, universe, missing, hashes, counts = choose(args.seed_dir, args.per_variant, args.universe)
    sql = prep_sql(pending, universe)
    (tmp / "prep.sql").write_text(sql, encoding="utf-8")
    seed = tmp / "seed"
    build_seed(args.seed_dir, seed, sql, universe, missing, hashes)
    print(f"Startdata: {len(pending)} bilder pending, {len(universe)} med derivat, "
          f"bilde {missing} mangler 1024-derivat. Varianter i biblioteket: {counts}")

    failures: list[str] = []
    steps = json.loads(args.scenario_file.read_text(encoding="utf-8"))
    log = tmp / "forespørsler.jsonl"
    server, url = start_server(log)
    try:
        impls = {"python": python_exe, "rust": rust_exe}
        homes = {name: tmp / f"pakke-{name}" for name in impls}
        for name, home in homes.items():
            shutil.copytree(seed, home)  # symlenker følges: hver pakke får egne filer
            (tmp / f"xdg-{name}").mkdir()
        env = {"FOTOMAXXER_OPENROUTER_URL": url}
        print("Parallell kjøring:")
        for step in steps:
            argv, stdin = (step, None) if isinstance(step, list) else (step["argv"], step.get("stdin"))
            res, secs = {}, {}
            for name, exe in impls.items():
                res[name], secs[name] = run_impl(exe, homes[name], tmp / f"xdg-{name}", argv, stdin, env)
            norm = {n: tuple(normalize(str(x), tmp, homes[n]) for x in r) for n, r in res.items()}
            label = " ".join(argv) + (f"  < {stdin!r}" if stdin is not None else "")
            timing = f"({secs['python']:.1f} s / {secs['rust']:.1f} s)"
            if norm["python"] == norm["rust"]:
                out = norm["rust"][1].strip().splitlines()
                print(f"  [LIK]   {label}  {timing}  → {out[-1] if out else norm['rust'][2].strip()}")
                continue
            failures.append(label)
            print(f"  [ULIK]  {label}  {timing}")
            for part, p, r in zip(("exit", "stdout", "stderr"), norm["python"], norm["rust"]):
                if p != r:
                    print(f"    {part}:")
                    for line in difflib.unified_diff(p.splitlines(), r.splitlines(), "python", "rust", lineterm="", n=1):
                        print(f"      {line}")
    finally:
        server.terminate()
        server.wait()

    print("Forespørsler:")
    compare_requests(log, failures)

    print("Database (matchet på image_id og innhold):")
    dbs = {n: db_content(h / "fotomaxxer.db", tmp, h) for n, h in homes.items()}
    a, b = dbs["python"], dbs["rust"]
    compare_dicts("ai_tags per bilde (alle felter inkl. raw_json og cost)", a["ai_tags"], b["ai_tags"], failures)
    if a["ai_tags_dupes"] or b["ai_tags_dupes"]:
        failures.append("ai_tags-duplikater")
        print(f"  [ULIK]  duplikate ai_tags-rader: python={a['ai_tags_dupes']} rust={b['ai_tags_dupes']}")
    compare_dicts("processing_state (status + feiltekst)", a["state"], b["state"], failures)
    compare_dicts("tagger med kategori per bilde", a["image_tags"], b["image_tags"], failures)
    compare_dicts("tags (navn, kategori)", dict(enumerate(a["tags"])), dict(enumerate(b["tags"])), failures)
    if a["rest"] == b["rest"]:
        print(f"  [LIK]   resten av databasen ({len(a['rest'])} linjer)")
    else:
        failures.append("resten av databasen")
        print("  [ULIK]  resten av databasen")
        for line in difflib.unified_diff(a["rest"], b["rest"], "python", "rust", lineterm="", n=0):
            print(f"      {line}")
    errors = collections.Counter((e or "").split(":")[0] for (s, e) in b["state"].values() if s == "error")
    print(f"  ai-feil (rust) per type: {dict(errors)}; done={sum(1 for (i, st), (s, e) in b['state'].items() if st == 'ai' and s == 'done')}")

    if not args.skip_exact:
        print("Deterministisk kjøring (--workers 1, hele databasen via compare_cli.py):")
        log2 = tmp / "forespørsler-eksakt.jsonl"
        server, url = start_server(log2)
        try:
            proc = subprocess.run(
                [sys.executable, str(HERE / "compare_cli.py"), "--python", python_exe, "--rust", rust_exe,
                 "--seed-dir", str(seed), "--scenario-file", str(args.exact_scenario),
                 "--env", f"FOTOMAXXER_OPENROUTER_URL={url}"],
                capture_output=True, text=True, timeout=3600, stdin=subprocess.DEVNULL,
            )
        finally:
            server.terminate()
            server.wait()
        print("\n".join("  " + line for line in proc.stdout.rstrip().splitlines()))
        if proc.returncode != 0:
            failures.append("deterministisk kjøring")
            print(proc.stderr)
        compare_requests(log2, failures)

    if os.environ.get("FMX_KEEP_TMP"):
        print(f"  (beholdt {tmp})")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    print("IDENTISK" if not failures else f"{len(failures)} avvik: {failures}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
