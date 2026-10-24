"""Pakke-relativ sti-resolver.

Hele pakka — appen, ``fotomaxxer.db`` og ``derived/`` — er én flyttbar mappe.
Appens egne stier (db + avledede thumbnails) resolves alltid relativt til
pakke-rota, aldri hardkodet absolutt. Da kan mappa kopieres til en annen maskin
og fortsatt virke. (``files.path`` er det eneste som er absolutt — det peker til
eksterne originaler og er bevisst maskin-spesifikt.)

Pakke-rota er mappa ``fotomaxxer.db`` ligger i. Den bestemmes ved kjøretid i
denne rekkefølgen: eksplisitt arg > ``FOTOMAXXER_HOME`` > **bootstrap-pekerfil**
(``~/.config/fotomaxxer/config.json`` → ``{"data_dir": …}``) > gjeldende
arbeidsmappe. Pekerfila lar GUI flytte datamappa: stien kan ikke ligge i
``settings``-tabellen (sirkulært — vi trenger stien for å åpne DB-en tabellen
ligger i), så den bor utenfor pakka i OS-konfigmappa.
"""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

ENV_HOME = "FOTOMAXXER_HOME"
DB_FILENAME = "fotomaxxer.db"
DERIVED_DIRNAME = "derived"

CONFIG_DIRNAME = "fotomaxxer"
CONFIG_FILENAME = "config.json"
POINTER_KEY = "data_dir"


def config_path() -> Path:
    """Sti til bootstrap-pekerfila (respekterer ``XDG_CONFIG_HOME``)."""
    base = os.environ.get("XDG_CONFIG_HOME")
    base_dir = Path(base).expanduser() if base else Path.home() / ".config"
    return base_dir / CONFIG_DIRNAME / CONFIG_FILENAME


def read_pointer() -> str | None:
    """Les ``data_dir`` fra pekerfila, eller ``None`` om den mangler/er ugyldig."""
    try:
        data = json.loads(config_path().read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None
    value = data.get(POINTER_KEY) if isinstance(data, dict) else None
    return value if isinstance(value, str) and value.strip() else None


def write_pointer(data_dir: str | os.PathLike[str]) -> None:
    """Skriv ``data_dir`` til pekerfila (atomisk, bevarer andre nøkler)."""
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            data = {}
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        data = {}
    data[POINTER_KEY] = str(Path(data_dir).expanduser().resolve())
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
    tmp.replace(path)


def resolve_root(explicit: str | os.PathLike[str] | None = None) -> Path:
    """Finn pakke-rota: eksplisitt arg > ``FOTOMAXXER_HOME`` > pekerfil > cwd."""
    if explicit is not None:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get(ENV_HOME)
    if env:
        return Path(env).expanduser().resolve()
    pointer = read_pointer()
    if pointer:
        return Path(pointer).expanduser().resolve()
    return Path.cwd().resolve()


def relocate_data(
    new_dir: str | os.PathLike[str],
    current_root: str | os.PathLike[str] | None = None,
    delete_source: bool = False,
) -> Path:
    """Flytt datamappa (``fotomaxxer.db`` + ``derived/``) til ``new_dir``.

    Kopi-så-bytt for trygghet: kopiér alt til ``new_dir`` først; bytt pekerfila
    kun når kopien er fullført. Feiler kopien, rulles delvis kopi tilbake og
    pekerfil + gammel mappe står urørt. Returnerer den nye rota.

    Den gamle mappa beholdes som standard — sletting er et bevisst valg.
    Sett ``delete_source=True`` for å fjerne gammel ``fotomaxxer.db`` +
    ``derived/`` (best effort) etter at kopien er trygt på plass.
    """
    src = (
        Path(current_root).expanduser().resolve()
        if current_root is not None
        else resolve_root()
    )
    dst = Path(new_dir).expanduser().resolve()
    if dst == src:
        return dst

    src_store, dst_store = Storage(src), Storage(dst)
    dst.mkdir(parents=True, exist_ok=True)

    copied: list[Path] = []
    try:
        if src_store.db_path.exists():
            shutil.copy2(src_store.db_path, dst_store.db_path)
            copied.append(dst_store.db_path)
        if src_store.derived_dir.exists():
            shutil.copytree(src_store.derived_dir, dst_store.derived_dir, dirs_exist_ok=True)
            copied.append(dst_store.derived_dir)
    except Exception:
        # Rull tilbake delvis kopi; pekerfil + gammel mappe urørt.
        for path in copied:
            shutil.rmtree(path, ignore_errors=True) if path.is_dir() else path.unlink(missing_ok=True)
        raise

    # Kopien er trygg → bytt peker. Gammel mappe beholdes med mindre eksplisitt slettet.
    write_pointer(dst)
    if delete_source:
        src_store.db_path.unlink(missing_ok=True)
        if src_store.derived_dir.exists():
            shutil.rmtree(src_store.derived_dir, ignore_errors=True)
    return dst


class Storage:
    """Resolver for appens egne stier, rotet i pakke-mappa."""

    def __init__(self, root: str | os.PathLike[str]):
        self.root = Path(root).expanduser().resolve()

    @property
    def db_path(self) -> Path:
        return self.root / DB_FILENAME

    @property
    def derived_dir(self) -> Path:
        return self.root / DERIVED_DIRNAME

    def derived_path(self, content_hash: str, size: str) -> Path:
        """Sti til en avledet kopi: ``derived/<hash>_<size>.jpg``.

        Lagre kun ``<hash>_<size>.jpg`` i DB-en (eller regn den ut herfra) —
        aldri en absolutt sti, så pakka forblir flyttbar.
        """
        return self.derived_dir / f"{content_hash}_{size}.jpg"

    def ensure_dirs(self) -> None:
        self.derived_dir.mkdir(parents=True, exist_ok=True)
