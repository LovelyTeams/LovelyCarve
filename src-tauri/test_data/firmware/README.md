# Synthetic firmware fixtures

Original generated data, CC0-1.0. Run `generate.py` with Python 3 after generating
the vendored backhand DD-WRT fixture. Only synthetic content and public format
constants are used; there is no real firmware or user data.

The independent Python generator wraps the small v3 filesystem in a TRX v1
container with CRC, a Buffalo plaintext file-length header and default-key ENC.
The bgn variant adds an encrypted 152-byte synthetic product tag. This verifies
both outer layouts through LovelyCarve's normal image and filesystem dispatch.

| File | SHA-256 |
| --- | --- |
| synthetic-bgn.enc | `709307922f9ccb9b85904a4f11fc37545fb0deff632d96e79d7acd5f0c1ab15b` |
| synthetic.enc | `d789405856318177d1a18490ca53253913cf451387608919720ab1755af563fb` |
