"""Desktop-inngang: arbeidere skal aldri åpne et nytt Qt-vindu."""

import multiprocessing

if __name__ == "__main__":
    multiprocessing.freeze_support()
    multiprocessing.set_start_method("spawn")
    from fotomaxxer.desktop import main

    raise SystemExit(main())
