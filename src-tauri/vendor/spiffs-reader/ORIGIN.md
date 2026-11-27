# SPIFFS reader provenance

LovelyCarve implements the reader in Rust (AGPL-3.0-only), with `lamfold` providing the bounded, read-only source. Page flags, index and lookup geometry were checked against [SPIFFS upstream](https://github.com/pellepl/spiffs/tree/3a4cf8ba5ed241867fdaeb6291bef17d2960d689). The upstream archive SHA-256 is `f4d5aa1c04249141ec9544ba2ed001175ddca5782b7da19151cd8289fedb21de`. The C implementation is used independently to verify samples.

Generate the fixtures with the [ESP-IDF v5.4.4 SPIFFS generator](https://github.com/espressif/esp-idf/blob/v5.4.4/components/spiffs/spiffsgen.py) (SHA-256 `20a4aaac4a22ca1b926d2163cf05fe20c5968f8160b35706014ac777ff406604`). Pass `0x40000` as image size, a directory containing `empty` (empty), `hello.txt` (`Embedded evidence.\n`) and `logs/event.bin` (49,281 bytes, each byte `n` is `(n * 23 + n // 61) % 256`), and an output path. Compress the resulting image with zlib.

| Fixture | Extra arguments | SHA-256 of uncompressed image |
| --- | --- | --- |
| `esp32-default` | Default 256-byte page and 4096-byte block | `956cf0a0125dfe1395a4184189776db4f15a5ec3cc1b50ac451c7106003d3300` |
| `esp32-no-magic` | `--no-magic --no-magic-len` | `f75b7d1423645d14c1e2bbbe91f87e6ad14f318d52ec97f1bb85f3e503701a4f` |
| `constant` | `--no-magic-len --meta-len 0` | `43bb9d09201cdd2b0839857993d3fe35d0a3822bf622930703bc954bcb120b39` |
| `be-aligned` | `--big-endian --aligned-obj-ix-tables --page-size 512 --block-size 8192 --obj-name-len 48 --meta-len 8` | `bdb36a8bcd48410b39e4ea34e36673690126ed6452553e67157c16e1c72c9df4` |

The original read-only SPIFFS implementation independently opened the first three images with each matching configuration and compared every byte of `/logs/event.bin` with the generator input. The fourth is a big-endian generator fixture tested through the Rust reader.

The upstream C implementation also wrote `esp32-native` with the default ESP32 geometry and the length-based signature. It creates `/empty`, writes and removes `/trash`, writes `/hello.txt`, and creates `/rename-me` with the first 1,787 bytes of the event body. It appends the rest and renames the object to `/logs/event.bin`. An independent read-only mount with the same C implementation verifies all 49,281 body bytes. SHA-256 before zlib is `a96aff0d7a1fbf6a41191eeb41867aaea10e163c5de139854f8b808306cd51fe`.
