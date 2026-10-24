"""FastAPI-laget for FotoMaxxer.

Importerer pipeline-funksjonene i ``fotomaxxer``-pakka **direkte** (ingen
subprocess) og eksponerer dem som JSON-endepunkter + serverer derivater og den
bygde SPA-en. Hver request får sin egen SQLite-connection; bakgrunnsjobber får
en til (se ``api/jobs.py``). Bindes til ``127.0.0.1`` — lokal, ingen auth.
"""
