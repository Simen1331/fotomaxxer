"""FastAPI-instans: router-inkludering, statiske mounts, SPA-servering.

Kjøres via ``fotomaxxer serve`` (uvicorn på ``127.0.0.1:8000``). Importerer
pipeline-funksjonene direkte gjennom routerne. ``/derived`` serverer avledede
thumbnails; ``/`` serverer den bygde SPA-en i prod (lagt til når ``web/dist``
finnes — frontend-bygget kommer i en senere sesjon).
"""

from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles

from api.deps import make_storage
from api.jobs import runner
from fotomaxxer.resources import frontend_dist
from api.routers import albums, export, fs, images, jobs, persons, profiles, settings, sources

@asynccontextmanager
async def lifespan(app: FastAPI):
    try:
        yield
    finally:
        runner.begin_shutdown()
        # Vent på jobbens connection og prosesspool før tjeneren avslutter.
        await asyncio.to_thread(runner.wait)


def create_app(*, session_token: str | None = None, origin: str | None = None) -> FastAPI:
    app = FastAPI(title="FotoMaxxer", version="0.1.0", lifespan=lifespan)
    for module in (images, persons, albums, export, sources, profiles, fs, jobs, settings):
        app.include_router(module.router, prefix="/api")

    @app.get("/api/health")
    def health() -> dict[str, str]:
        return {"status": "ok"}

    store = make_storage()
    store.derived_dir.mkdir(parents=True, exist_ok=True)
    app.mount("/derived", StaticFiles(directory=str(store.derived_dir)), name="derived")

    dist = frontend_dist()
    if dist.is_dir():
        assets = dist / "assets"
        if assets.is_dir():
            app.mount("/assets", StaticFiles(directory=str(assets)), name="assets")

        @app.get("/{full_path:path}", include_in_schema=False)
        def spa_fallback(full_path: str) -> FileResponse:
            if full_path in ("api", "derived") or full_path.startswith(("api/", "derived/")):
                raise HTTPException(status_code=404, detail={"error": "not_found", "message": "ukjent endepunkt"})
            candidate = (dist / full_path).resolve()
            if full_path and candidate.is_relative_to(dist.resolve()) and candidate.is_file():
                return FileResponse(candidate)
            index = dist / "index.html"
            if not index.is_file():
                raise HTTPException(status_code=404, detail={"error": "not_found", "message": "SPA ikke bygget"})
            return FileResponse(index)

    if session_token is not None:
        if origin is None:
            raise ValueError("Desktop-sesjonen trenger en eksplisitt origin.")
        from api.desktop_access import DesktopAccess

        app.add_middleware(DesktopAccess, token=session_token, origin=origin)
    return app


app = create_app()
