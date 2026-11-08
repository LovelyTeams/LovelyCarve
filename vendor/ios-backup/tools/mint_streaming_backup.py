"""LovelyCarve fixture: independent encryption across four 64 KiB boundaries.

Run with an output directory and optional --plain, as for the upstream minter.
The final byte looks like padding but is actual manifest-recorded content.
"""
import mint_encrypted_backup as mint

mint.CONTENT = [("HomeDomain", "Documents/stream.bin", bytes(range(256)) * 1025 + b"\x01")]

if __name__ == "__main__":
    raise SystemExit(mint.main())
