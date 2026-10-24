import multiprocessing
import sys

if __name__ == "__main__":
    multiprocessing.freeze_support()
    from fotomaxxer.cli import main

    sys.exit(main())
