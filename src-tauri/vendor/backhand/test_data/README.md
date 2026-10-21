# Synthetic DD-WRT fixture

`ddwrt-v3.squashfs` is generated entirely by `generate_ddwrt.py`, using Python's
standard `struct` and `lzma` modules. It contains one file `/hello` with the
literal `Independent DD-WRT fixture.\n` repeated 64 times. Data, inode and directory
blocks use different LZMA parameters. There is no real firmware or user data.

Layout reference: Firmware Mod Kit commit
`c72f45d7a062156125ae00cb867d8a614b50b963`, SquashFS 3.0 variant headers.
The generator is an independent implementation, not a backhand writer round trip.
These original synthetic fixture files are dedicated to the public domain under CC0-1.0.

SHA-256: `adf312d8c6d9e7ae5fadcfec851b960a4e729a3495caa5c277dcc71e978db7eb`.
