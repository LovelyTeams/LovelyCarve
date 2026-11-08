"""Generate invented USTAR input for the external ABE fixture oracle."""

import io
import sys
import tarfile
from pathlib import Path


def main():
    destination = Path(sys.argv[1])
    with destination.open("xb") as output:
        with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            entry = tarfile.TarInfo("apps/com.example/db/main.db")
            entry.size = 7
            entry.mode = 0o600
            entry.mtime = 1_700_000_000
            archive.addfile(entry, io.BytesIO(b"payload"))


if __name__ == "__main__":
    main()
