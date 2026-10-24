"""Frosset CLI: håndter arbeidere før CLI og tunge biblioteker importeres."""

import multiprocessing

if __name__ == "__main__":
    multiprocessing.freeze_support()
    # Samme oppstartsmodell på Windows og Linux; ingen fork av lastet ML-kode.
    multiprocessing.set_start_method("spawn")
    from fotomaxxer.cli import main

    raise SystemExit(main())
