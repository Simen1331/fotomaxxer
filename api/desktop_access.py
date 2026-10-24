"""Tilgangsport for den interne desktop-tjeneren. Utviklingsstart bruker den ikke."""

from __future__ import annotations

from hmac import compare_digest

from starlette.responses import JSONResponse

from api.jobs import runner

SESSION_HEADER = b"x-fotomaxxer-session"


class DesktopAccess:
    def __init__(self, app, *, token: str, origin: str):
        self.app = app
        self.token = token.encode("ascii")
        self.origin = origin.encode("ascii")
        self.host = origin.removeprefix("http://").encode("ascii")

    async def __call__(self, scope, receive, send):
        if scope["type"] not in ("http", "websocket"):
            await self.app(scope, receive, send)
            return
        headers = dict(scope["headers"])
        allowed = (
            compare_digest(headers.get(SESSION_HEADER, b""), self.token)
            and headers.get(b"host") == self.host
            and headers.get(b"origin", self.origin) == self.origin
            and headers.get(b"sec-fetch-site", b"none") in (b"none", b"same-origin")
        )
        if scope["type"] == "websocket":
            await send({"type": "websocket.close", "code": 1008})
            return
        status, message = 403, "Tilgang til appen krever en gyldig desktop-sesjon."
        if allowed:
            if scope["method"] not in ("GET", "HEAD", "OPTIONS") and runner.is_closing():
                status, message = 503, "Appen avslutter. Vent til pågående arbeid er ferdig."
            elif scope["method"] == "POST" and scope["path"] == "/api/maintenance/data-dir":
                # Flytting trenger en egen leveranse med quiescence og bytte av mounts.
                status, message = 409, "Flytting av datamappa er ikke tilgjengelig i denne desktop-utprøvingen."
            else:
                await self.app(scope, receive, send)
                return
        response = JSONResponse({"detail": {"error": "desktop_access", "message": message}}, status_code=status)
        await response(scope, receive, send)
