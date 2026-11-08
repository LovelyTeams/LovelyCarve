# CoreStorage / FileVault 2 (FVDE) — format & crypto reference

Authoritative spec: libyal **libfvde** —
`documentation/FileVault Drive Encryption (FVDE).asciidoc`
(<https://github.com/libyal/libfvde/blob/main/documentation>). Every offset and
algorithm below is cross-checked against libfvde source and against a real
encrypted volume decrypted stage-by-stage with a debug build of `fvdeinfo`
(see the ground-truth section — Tier-2 oracle: real engine output).

Scope: **CoreStorage / FileVault 2 (macOS 10.7–10.15), AES-XTS-128, password
protector.** APFS-native encryption (10.13+) is a separate format with no
reference decryptor and is **explicitly deferred** (see `DEFERRED.md`).

## Physical volume header (512 bytes, at partition start; LE fields)

| off | size | field |
|----:|-----:|-------|
| 0   | 4  | checksum (CRC-32 of bytes 8..512) |
| 4   | 4  | initial value (0xffffffff) |
| 8   | 2  | format version (1) |
| 10  | 2  | block type (0x0010) |
| 12  | 4  | serial number |
| 48  | 4  | bytes per sector (512) |
| 64  | 8  | physical volume size (bytes) |
| 88  | 2  | CoreStorage signature `"CS"` |
| 90  | 4  | checksum algorithm |
| 96  | 4  | **block size** (4096) |
| 100 | 4  | metadata size |
| 104 | 32 | 4 × u64 metadata block numbers (×block_size = byte offset) |
| 168 | 4  | key data size (16) |
| 172 | 4  | encryption method (2 = AES-XTS-128) |
| 176 | 128| **key data** (first 16 bytes = metadata XTS key) |
| 304 | 16 | **physical volume identifier** (UUID, on-disk byte order) |
| 320 | 16 | volume group identifier (UUID) |

## Metadata (plaintext) — locate the encrypted metadata

Read the metadata block at `metadata_block_numbers[0] * block_size`. Block type
`0x0011` carries:
- encrypted metadata number of blocks (e.g. 6144)
- encrypted metadata block numbers (primary, secondary; e.g. 2049, 8193)

Metadata block header = 64 bytes: checksum[4]@0, initial_value[4]@4,
version[2]@8, type[2]@10, serial[4]@12, transaction_id[8]@16,
object_id[8]@24, number[8]@32, unknown[8]@40, block_size[4]@48, ...

## Encrypted metadata — AES-XTS-128

- ciphertext at `primary_encrypted_metadata_block * block_size`
- **key1 = key_data[0..16]** (from the volume header)
- **key2 = physical_volume_identifier** (16 bytes, on-disk order)
- unit size **8192 bytes**; tweak = 0-based block index within the region
- decrypts to standard metadata blocks (0x0013, 0x0019, 0x001a, ...)

Extract from the decrypted metadata:
- **`com.apple.corestorage.lvf.encryption.context`** plist (in a block whose
  payload is XML plist text) → base64 `PassphraseWrappedKEKStruct` +
  `KEKWrappedVolumeKeyStruct` (the `BlockAlgorithm == AES-XTS` entry).
- block 0x001a → **`com.apple.corestorage.lv.familyUUID`** (tweak-key input).
- LV descriptor → **first physical block** (LV base) + size + segment map.

### PassphraseWrappedKEKStruct (284 bytes)
| off | size | field |
|----:|-----:|-------|
| 8   | 16 | PBKDF2 salt |
| 32  | 24 | WrappedKEK (RFC 3394: 8-byte A6.. ICV + 16-byte key) |
| 172 | 4  | PBKDF2 iteration count |

### KEKWrappedVolumeKeyStruct (256 bytes)
| off | size | field |
|----:|-----:|-------|
| 8   | 24 | WrappedKEK for the volume master key |

## Key hierarchy (RustCrypto — never hand-roll)

1. `passphrase_key = PBKDF2-HMAC-SHA256(password_utf8, salt, iterations, dkLen=16)`
2. `KEK = AES-KW-unwrap(WrappedKEK@PassphraseWrapped[32..56], key=passphrase_key)` (RFC 3394, 24→16)
3. `VMK = AES-KW-unwrap(WrappedKEK@KEKWrapped[8..32], key=KEK)` (24→16)
4. `tweak_key = SHA256(VMK ‖ familyUUID_bytes)[0..16]`

## Logical-volume sector decryption — AES-XTS-128

- For LV logical offset `L`: physical offset via the segment map; the fvdetest
  LV is a **single contiguous segment** (base = first_block × block_size).
- **key1 = VMK, key2 = tweak_key**, unit = 512 bytes (`bytes_per_sector`).
- **tweak value = logical sector number = L / 512** (LOGICAL, not physical —
  verified empirically against the oracle at 5 offsets).

## Tier-2 ground truth — dfvfs `fvdetest.qcow2`, password `fvde-TEST`

CS partition: GPT LBA 40 → byte offset 20480, length 536829952 (511 MiB).
Carve: `qemu-img convert -O raw fvdetest.qcow2 x.raw && dd if=x.raw of=cs.raw bs=512 skip=40 count=1048496`.

| stage | value (hex) |
|-------|-------------|
| volume header key_data | `18eaeb7da9ab0852ead69e9dabc86f59` |
| physical volume id | `3273a055-3b8b-47e8-b970-df35eecda81b` |
| block size / bytes-per-sector | 4096 / 512 |
| encrypted-metadata primary block | 2049 (× 4096) |
| family UUID | `1F01CA34-5F6C-4123-AC0C-B0A256889DB2` |
| PBKDF2 salt | `9bfcf480e4d9ad0eddd9ac6f47b85955` |
| PBKDF2 iterations | 90506 |
| passphrase_key (PBKDF2 out, 16) | `0ec2849349f914e8bdbc189ac09c8bc7` |
| PassphraseWrapped WrappedKEK (24) | `ebbc1f64b9684eb4b26bfba3f855786 77bab8cfafaf7b2c1` |
| KEK (unwrapped, 16) | `a2543f0b8a6fc5cf2eaf7e76c95ef49c` |
| KEKWrapped WrappedKEK (24) | `9a5b30e99f902ed8e2f03989e5f9c154 3ed60512aa1dc9d1` |
| **VMK** (16) | `d0d9c323197c62401c6e6b48f1c0f9d7` |
| **tweak key** (16) | `53a17ba3213ec213bedcc34fe4e239af` |
| LV base (physical) / size | 0x04000000 / 167772160 (single segment) |

### Decrypted LV sector sha256 (512 B) — final oracle
| LV offset | sha256 |
|----------:|--------|
| 0 | `076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560` |
| 1024 | `ebedb80407fc8bfdd3cce9c68de94efece7ed748df1babf35deeaacf008990af` |
| 163840 | `a863e21577e54cd763729803a621804da4b5030afa35bcf879ea3b3413488a66` |
| 1048576 | `076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560` |
| 10485760 | `076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560` |

Offset 1024 decrypts to the HFS+ volume header (`482b0004…4846534a` = "H+" v4
"HFSJ"), the structural proof of correct decryption.
