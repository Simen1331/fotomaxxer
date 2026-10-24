#!/usr/bin/env python3
"""Bygg en intern PyInstaller-utprøving med lokale modell- og ExifTool-ressurser."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))
from fotomaxxer.resources import BUFFALO_FILES


def sha(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stage_linux_exiftool(destination: Path, executable: Path, perl: Path) -> dict:
    config = json.loads(subprocess.check_output([
        str(perl), "-MConfig", "-MJSON::PP", "-e",
        'print encode_json({map {$_ => $Config{$_}} qw(version archname privlib archlib vendorlib)});',
    ], text=True))
    destination.mkdir(parents=True)
    shutil.copy2(perl, destination / "perl")
    shutil.copytree(config["privlib"], destination / "core")
    shutil.copytree(config["archlib"], destination / "arch")
    library = Path(config["vendorlib"]) / "Image"
    if not (library / "ExifTool.pm").is_file():
        raise FileNotFoundError(f"Image/ExifTool.pm mangler under {library}")
    (destination / "lib" / "Image").mkdir(parents=True)
    shutil.copy2(library / "ExifTool.pm", destination / "lib" / "Image" / "ExifTool.pm")
    shutil.copytree(library / "ExifTool", destination / "lib" / "Image" / "ExifTool")
    # ExifTool distribueres også med File::RandomAccess utenfor Image/.
    random_access = library.parent / "File" / "RandomAccess.pm"
    (destination / "lib" / "File").mkdir()
    shutil.copy2(random_access, destination / "lib" / "File" / "RandomAccess.pm")
    # Ikke la Perls innebygde @INC hente moduler fra byggemaskina ved kjøring.
    script = executable.read_text(encoding="utf-8")
    header, body = script.split("\n", 1)
    isolation = 'BEGIN { @INC = map { "$ENV{FOTOMAXXER_PERL_ROOT}/$_" } qw(lib core arch); }\n'
    (destination / "exiftool.pl").write_text(header + "\n" + isolation + body, encoding="utf-8")
    launcher = destination / "exiftool"
    launcher.write_text(
        '#!/bin/sh\n'
        'base=$(CDPATH= cd -- "${0%/*}" && pwd) || exit 1\n'
        'FOTOMAXXER_PERL_ROOT="$base"\n'
        'PERL5LIB= PERL5OPT=\n'
        'export FOTOMAXXER_PERL_ROOT PERL5LIB PERL5OPT\n'
        'exec "$base/perl" "$base/exiftool.pl" "$@"\n',
        encoding="utf-8",
    )
    launcher.chmod(0o755)
    version = subprocess.check_output([str(launcher), "-ver"], text=True).strip()
    return {"version": version, "perl": config}


def stage_windows_exiftool(destination: Path, source: Path) -> dict:
    executable = source / "exiftool.exe"
    # Nye Windows-utgaver trenger også exiftool_files, ikke bare exe-fila.
    if not executable.is_file() or not (source / "exiftool_files").is_dir():
        raise ValueError("Angi komplett ExifTool-mappe med exiftool.exe og exiftool_files/.")
    shutil.copytree(source, destination)
    version = subprocess.check_output([str(destination / "exiftool.exe"), "-ver"], text=True).strip()
    return {"version": version}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--models", type=Path, required=True, help="Lokal buffalo_l-mappe med fem ONNX-filer.")
    parser.add_argument("--exiftool", type=Path, required=True, help="Linux: ExifTool-script. Windows: komplett mappe.")
    parser.add_argument("--perl", type=Path, help="Linux: Perl-binær (standard: fra byggemaskinas PATH).")
    parser.add_argument("--skip-frontend", action="store_true", help="Bruk eksisterende web/dist i denne utprøvingen.")
    args = parser.parse_args()
    if sys.platform not in ("linux", "win32") or platform.machine().lower() not in ("x86_64", "amd64"):
        parser.error("Utprøvingen støtter foreløpig Linux x86-64 og Windows x64.")
    if importlib.metadata.version("pyinstaller") != "6.22.0":
        parser.error("Installer packaging/requirements-build.txt først.")
    model_dir = args.models.expanduser().resolve()
    missing = [name for name in BUFFALO_FILES if not (model_dir / name).is_file()]
    if missing:
        parser.error(f"Modellfiler mangler: {', '.join(missing)}")
    source = args.exiftool.expanduser().resolve()
    if not source.exists():
        parser.error(f"ExifTool finnes ikke: {source}")
    if not args.skip_frontend:
        npm = shutil.which("npm")
        if npm is None:
            parser.error("npm mangler på byggemaskina.")
        subprocess.run([npm, "ci"], cwd=REPO / "web", check=True)
        subprocess.run([npm, "run", "build"], cwd=REPO / "web", check=True)
    if not (REPO / "web" / "dist" / "index.html").is_file():
        parser.error("Bygg web/dist først, eller fjern --skip-frontend.")

    stage = REPO / "build" / "packaging-resources"
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    target_models = stage / "insightface" / "models" / "buffalo_l"
    target_models.mkdir(parents=True)
    for name in BUFFALO_FILES:
        shutil.copy2(model_dir / name, target_models / name)
    # InsightFace get_object() søker direkte i _MEIPASS/objects når frosset.
    insightface = importlib.util.find_spec("insightface")
    objects = Path(insightface.origin).parent / "data" / "objects"
    if objects.is_dir():
        shutil.copytree(objects, stage / "objects")
    shutil.copytree(REPO / "web" / "dist", stage / "web" / "dist")
    tools = stage / "tools" / "exiftool"
    if sys.platform == "win32":
        exif = stage_windows_exiftool(tools, source)
    else:
        perl = args.perl or shutil.which("perl")
        if not perl:
            parser.error("Perl mangler på byggemaskina.")
        exif = stage_linux_exiftool(tools, source, Path(perl).resolve())

    manifest = {
        "format": 1, "purpose": "internal-packaging-experiment",
        "platform": platform.platform(), "machine": platform.machine(),
        "python": platform.python_version(), "exiftool": exif,
        "model": {"name": "buffalo_l", "files": list(BUFFALO_FILES)},
        "dependencies": dict(sorted(
            (d.metadata["Name"], d.version) for d in importlib.metadata.distributions()
            if d.metadata["Name"])),
        "frontend_lock_sha256": sha(REPO / "web" / "package-lock.json"),
        "files": {str(p.relative_to(stage)).replace(os.sep, "/"): {
            "sha256": sha(p), "bytes": p.stat().st_size,
        } for p in sorted(stage.rglob("*")) if p.is_file()},
    }
    (stage / "resource-manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8",
    )
    env = os.environ.copy()
    env["FOTOMAXXER_BUILD_RESOURCES"] = str(stage)
    subprocess.run([
        sys.executable, "-m", "PyInstaller", "--noconfirm", "--clean",
        "--distpath", str(REPO / "dist"), "--workpath", str(REPO / "build" / "pyinstaller"),
        str(REPO / "packaging" / "fotomaxxer.spec"),
    ], cwd=REPO, env=env, check=True)
    executable = REPO / "dist" / "FotoMaxxer" / ("FotoMaxxer.exe" if sys.platform == "win32" else "FotoMaxxer")
    print(f"Ferdig utprøvingspakke: {executable}")
    print("Kjør scripts/verify-desktop-probe.py med --desktop før pakka vurderes som fungerende.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
