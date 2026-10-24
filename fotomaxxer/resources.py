"""Skrivebeskyttede appressurser, uavhengig av datamappe og arbeidsmappe."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

BUFFALO_FILES = (
    "1k3d68.onnx", "2d106det.onnx", "det_10g.onnx",
    "genderage.onnx", "w600k_r50.onnx",
)


def resource_root() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys._MEIPASS)
    return Path(__file__).resolve().parent.parent


def frontend_dist() -> Path:
    root = resource_root()
    if getattr(sys, "frozen", False):
        return root / "web" / "dist"
    # Bevar den eksisterende embedded-Python-pakka og utviklingsstarten.
    legacy = Path(sys.executable).resolve().parent.parent / "web" / "dist"
    return legacy if legacy.is_dir() else root / "web" / "dist"


def exiftool_executable() -> str:
    name = "exiftool.exe" if sys.platform == "win32" else "exiftool"
    if getattr(sys, "frozen", False):
        executable = resource_root() / "tools" / "exiftool" / name
        if not executable.is_file():
            raise FileNotFoundError(f"Pakket ExifTool mangler: {executable}")
        return str(executable)
    legacy = Path(sys.executable).resolve().parent.parent / name
    return str(legacy) if legacy.is_file() else (shutil.which(name) or name)


def face_model_root() -> Path | None:
    """Frosset app laster kun lokale modeller; utviklingsstart er uendret."""
    if not getattr(sys, "frozen", False):
        return None
    root = resource_root() / "insightface"
    model_dir = root / "models" / "buffalo_l"
    missing = [name for name in BUFFALO_FILES if not (model_dir / name).is_file()]
    if missing:
        raise FileNotFoundError(f"Pakkede buffalo_l-modeller mangler: {', '.join(missing)}")
    return root
