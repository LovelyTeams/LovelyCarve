# LovelyCarve integration

Vendored core from https://github.com/SecurityRonin/luks-forensic at `284f06487d7a66f13ed27c61be5e26f0863c0e4f`. Apache-2.0; original license and tests retained.

Local changes: standalone manifest; reject out-of-range/truncated encrypted data instead of synthesizing ciphertext. Application integration adds bounded evidence access and metadata validation.

The libluksde comparison adds AES CBC/ECB, Blowfish CBC/ECB, Serpent CBC/ECB,
ARC4 ECB, null/plain/plain64/benbi/ESSIV IVs, SHA224/SHA384 PBKDF2 and AF,
verified LUKS1/2 master-key unlock, UTF16 password conversion and unlocked UUID.
LUKS2 area keys may have a different size/cipher from the payload master key.
Independent Python-produced fixtures run unconditionally; external cryptsetup
oracle tests remain environment-gated. Details, API mappings and deliberate
differences from libluksde are in `docs/formats/LUKS.md` at the repository root.

Header allocation is limited to 8 KiB–4 MiB and the available source length. The application additionally verifies the LUKS2 primary checksum and rejects unsupported segment/keyslot layouts before derivation. Fixed-size/multi-segment data and backup-header recovery are not supported.
