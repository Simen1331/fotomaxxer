#!/usr/bin/env python3
"""Falsk OpenRouter-tjener for å sammenligne AI-merkingen i Python og Rust uten betalte kall.

Svarer på ``POST …/chat/completions`` deterministisk ut fra forespørselen: SHA-256 av
bildedataene (1024-derivatet i ``data:``-URL-en) velger en svarvariant, og prompten
bestemmer kategoriene i det kategoriserte svaret. Hver implementasjon (skilt på
``User-Agent``) får sin egen forsøksteller per bilde, så «429 før suksess» skjer én
gang per bilde og implementasjon. Alle forespørsler logges som JSON-linjer.

    python3 tools/golden/fake_openrouter.py --port 0 --log forespørsler.jsonl

Skriver ``LYTTER <port>`` på stdout når tjeneren er klar. Ingen avhengigheter
utenfor standardbiblioteket.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Sjeldne varianter som feiler hvert forsøk (5 forsøk med backoff = 15 s per bilde).
PERMANENT_503 = "permanent-503"
PERMANENT_400 = "permanent-400"
BAD_BODY = "ugyldig-kropp"
RARE = {0: PERMANENT_503, 1: PERMANENT_400, 2: BAD_BODY}
RARE_MODULUS = 40

VARIANTS = [
    "flat",
    "kategorisert",
    "gjerde-uten-kost",
    "tekst-rundt-uten-usage",
    "ugyldig-innhold",
    "null-felter",
    "rare-typer",
    "429-forst",
    "liste-innhold",
    "mangler-choices",
    "kategori-spesial",
    "gjerde-uten-merke",
    "unicode-og-tall",
]


def variant_for(img_sha: str) -> str:
    h = int(img_sha[:12], 16)
    if h % RARE_MODULUS in RARE:
        return RARE[h % RARE_MODULUS]
    return VARIANTS[(h // RARE_MODULUS) % len(VARIANTS)]


def impl_of(user_agent: str) -> str:
    if user_agent.startswith("python-requests"):
        return "python"
    if user_agent.startswith("ureq"):
        return "rust"
    return user_agent or "ukjent"


def categories_in(prompt: str) -> list[str]:
    m = re.search(r"kategoriene: (.*)\.\n", prompt)
    return [c for c in m.group(1).split(", ")] if m else []


def completion(content: str, usage: object = ..., extra: dict | None = None) -> dict:
    body: dict = {
        "id": "gen-falsk-1",
        "provider": "Falsk",
        "model": "test/fake-vision",
        "object": "chat.completion",
        "created": 1761230000,
        "choices": [
            {
                "logprobs": None,
                "finish_reason": "stop",
                "index": 0,
                "message": {"role": "assistant", "content": content, "refusal": None},
            }
        ],
    }
    if usage is not ...:
        body["usage"] = usage
    if extra:
        body.update(extra)
    return body


def usage(cost: object = ...) -> dict:
    u: dict = {"prompt_tokens": 1290, "completion_tokens": 87, "total_tokens": 1377}
    if cost is not ...:
        u["cost"] = cost
        u["is_byok"] = False
        u["cost_details"] = {"upstream_inference_cost": None}
    return u


def respond(variant: str, attempt: int, prompt: str, img_sha: str) -> tuple[int, str, str]:
    """(status, content-type, kropp) for en variant og et forsøksnummer."""
    j = "application/json; charset=utf-8"
    n = int(img_sha[12:14], 16)  # litt variasjon innen samme variant
    if variant == PERMANENT_503:
        msg = "Tjenesten er overbelastet – prøv igjen senere. " + "ø" * 220
        return 503, j, json.dumps({"error": {"message": msg, "code": 503}}, ensure_ascii=False)
    if variant == PERMANENT_400:
        return 400, j, json.dumps({"error": {"message": "Ugyldig bilde", "code": 400}})
    if variant == BAD_BODY:
        return 200, "text/html; charset=utf-8", "<html><body>502 Bad Gateway</body></html>"
    if variant == "429-forst" and attempt == 1:
        return 429, j, json.dumps({"error": {"message": "Rate limit exceeded", "code": 429}})

    flat = {
        "scene": ["strand", "skog", "kjøkken", "hage"][n % 4],
        "event_type": ["ferie", "hverdag", "bursdag", "tur"][n % 4],
        "season": ["sommer", "vinter", "vår", "høst"][n % 4],
        "indoor_outdoor": "ute" if n % 2 else "inne",
        "person_count": n % 5,
        "description": f"Et bilde med {n % 5} personer på {['stranda', 'tur', 'kjøkkenet'][n % 3]}.",
        "tags": ["Familie", "sommer", f"motiv{n % 7}", "familie", " Hav "],
    }
    if variant == "flat":
        body = completion(json.dumps(flat, ensure_ascii=False), usage(0.000123 + n * 1e-7))
    elif variant == "kategorisert":
        # Tag-navnet bærer kategorien, så «sist ikke-null vinner» (COALESCE) gir samme
        # kategori uansett rekkefølge. Første kategoris ord gjentas (med store bokstaver)
        # i neste kategori: dedup beholder første forekomst.
        cats = categories_in(prompt) or ["motiv", "stemning"]
        tags = {c: [f"{c}-ord{n % 3}", f" {c.upper()}-felles "] for c in cats}
        if len(cats) > 1:
            tags[cats[1]].append(f"{cats[0].upper()}-ORD{n % 3}")
        tags["Ekstra kategori"] = ["Uventet", "familie"]
        content = json.dumps({**flat, "tags": tags}, ensure_ascii=False)
        body = completion(content, usage(1.2e-05 * (1 + n % 3)))
    elif variant == "gjerde-uten-kost":
        content = "```json\n" + json.dumps(flat, ensure_ascii=False, indent=2) + "\n```"
        body = completion(content, usage())
    elif variant == "tekst-rundt-uten-usage":
        content = "Her er analysen:\n" + json.dumps(flat, ensure_ascii=False) + "\nHåper det hjelper!"
        body = completion(content)
    elif variant == "ugyldig-innhold":
        body = completion("Beklager, jeg kan ikke analysere dette bildet.", usage(0.0001))
    elif variant == "null-felter":
        content = json.dumps(
            {
                "scene": None,
                "event_type": "null",
                "season": "NULL",
                "indoor_outdoor": "  ",
                "person_count": " 3 ",
                "description": "  Tekst med æøå og «sitat»  ",
                "tags": ["Ål", "ål", " ÅL ", 7, True, None, 1.5, "", "  ", "Σ-form"],
            },
            ensure_ascii=False,
        )
        body = completion(content, usage(None))
    elif variant == "rare-typer":
        content = json.dumps(
            {
                "scene": 12,
                "event_type": 1.5,
                "season": False,
                "indoor_outdoor": ["ute", "inne"],
                "person_count": "2.5" if n % 2 else "1_000",
                "description": {"tekst": "vår'dag", "n": None, "x": [1, 2.0, 1e-05]},
                "tags": "bare en streng",
            },
            ensure_ascii=False,
        )
        body = completion(content, usage("0.00042"))
    elif variant == "429-forst":
        body = completion(json.dumps(flat, ensure_ascii=False), usage(9.5e-05))
    elif variant == "liste-innhold":
        body = completion('["strand", "hav"]', usage(0.0002))
    elif variant == "mangler-choices":
        body = {"error": {"message": "Provider returned error", "code": 502}, "user_id": "u-1"}
    elif variant == "kategori-spesial":
        cats = categories_in(prompt) or ["materiale"]
        tags = {f"  {cats[0]}  ": f"{cats[0]}-enkeltord", "": ["uten kategori"], cats[-1] + "!": [], " ": ["blank"]}
        content = "```JSON\n" + json.dumps({**flat, "person_count": True, "tags": tags}, ensure_ascii=False) + "\n```"
        body = completion(content, usage(0))
    elif variant == "gjerde-uten-merke":
        content = "```\n" + json.dumps({**flat, "person_count": 2.7}, ensure_ascii=False) + "\n``` og litt tekst etterpå"
        body = completion(content, usage(1e-7))
    elif variant == "unicode-og-tall":
        content = json.dumps(
            {**flat, "description": "Linje skift, emoji 😀 og \u0007kontroll", "tags": ["😀", "Ærlig"]},
            ensure_ascii=True,
        )
        body = completion(
            content,
            {**usage(0.0001234), "native_tokens": 10**16, "liten": 1e-7, "stor": 1e16, "neg": -0.5},
            extra={"system_fingerprint": None, "tall": [0.1, 2.5e-10, 123456789012345678]},
        )
    else:
        raise ValueError(variant)
    return 200, j, json.dumps(body, ensure_ascii=False)


class State:
    def __init__(self, log_path: str):
        self.lock = threading.Lock()
        self.attempts: dict[tuple[str, str], int] = {}
        self.log = open(log_path, "a", encoding="utf-8")

    def next_attempt(self, key: tuple[str, str]) -> int:
        with self.lock:
            self.attempts[key] = self.attempts.get(key, 0) + 1
            return self.attempts[key]

    def write(self, record: dict) -> None:
        with self.lock:
            self.log.write(json.dumps(record, ensure_ascii=False) + "\n")
            self.log.flush()


class Handler(BaseHTTPRequestHandler):
    state: State
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt: str, *args: object) -> None:  # stille
        pass

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        impl = impl_of(self.headers.get("User-Agent", ""))
        record: dict = {
            "impl": impl,
            "path": self.path,
            "body_sha": hashlib.sha256(raw).hexdigest(),
            "body_ascii": raw.isascii(),
            "headers": {
                k: self.headers.get(k) for k in ("Authorization", "Content-Type", "X-Title")
            },
        }
        try:
            payload = json.loads(raw)
            msg = payload["messages"][0]
            text = next(c["text"] for c in msg["content"] if c["type"] == "text")
            url = next(c["image_url"]["url"] for c in msg["content"] if c["type"] == "image_url")
            prefix, b64 = url.split(",", 1)
            img_sha = hashlib.sha256(base64.b64decode(b64)).hexdigest()
        except Exception as e:  # noqa: BLE001 — logg og svar 400
            record["feil"] = f"{type(e).__name__}: {e}"
            self.state.write(record)
            self._send(400, "application/json", '{"error": "ugyldig forespørsel"}')
            return
        attempt = self.state.next_attempt((impl, img_sha))
        variant = variant_for(img_sha)
        status, ctype, body = respond(variant, attempt, text, img_sha)
        record.update(
            img_sha=img_sha,
            attempt=attempt,
            variant=variant,
            status=status,
            model=payload.get("model"),
            temperature=payload.get("temperature"),
            response_format=payload.get("response_format"),
            usage=payload.get("usage"),
            keys=list(payload),
            role=msg.get("role"),
            content_types=[c["type"] for c in msg["content"]],
            data_url_prefix=prefix,
            prompt=text,
        )
        self.state.write(record)
        self._send(status, ctype, body)

    def _send(self, status: int, ctype: str, body: str) -> None:
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--log", required=True)
    args = ap.parse_args()
    Handler.state = State(args.log)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    print(f"LYTTER {server.server_address[1]}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
