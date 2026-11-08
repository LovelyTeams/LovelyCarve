# Comparison with Other Libraries

## vs. apple-dmg

The [apple-dmg](https://crates.io/crates/apple-dmg) crate from the apple-platform-rs project has similar goals but significant limitations:

| Feature | **udif** | apple-dmg |
|---------|:-------:|:---------:|
| Zlib | ✓ | ✓ |
| LZFSE | ✓ | ❌ unimplemented |
| XZ | ✓ | ❌ unimplemented |
| Bzip2 | ✓ | ❌ unimplemented |
| ADC | ❌ | ❌ unimplemented |
| Checksum verification | ✓ | ❌ |
| Documentation | ~80% | 12.2% |
| Edge case tests | 31 tests | — |

**Why this matters:** Most modern macOS DMGs (including Kernel Debug Kits, Xcode downloads, and system updates) use **LZFSE compression**. The apple-dmg crate cannot read these files.

```
$ udif-tool info Kernel_Debug_Kit.dmg
Block types used:
  LZFSE:            988 blocks   ← 100% LZFSE, apple-dmg would fail
```

## vs. apple-xar

The [apple-xar](https://crates.io/crates/apple-xar) crate handles **XAR archives** (`.pkg` installers), which is a completely different format from DMG. Not applicable for disk image handling.

## vs. dmgwiz

The [dmgwiz](https://crates.io/crates/dmgwiz) crate is a well-maintained library with good compression support:

| Feature | **udif** | dmgwiz |
|---------|:-------:|:------:|
| Read | ✓ | ✓ |
| Write | ✓ | ❌ |
| Zlib | ✓ | ✓ |
| Bzip2 | ✓ | ✓ |
| LZFSE | ✓ | ✓ |
| XZ | ✓ | ❌ |
| ADC | ❌ | ✓ |
| Encrypted DMGs | ❌ | ✓ |

**Verdict:** If you only need to **read** DMGs and need encrypted DMG support, dmgwiz is a good choice. If you need to **create** DMGs or handle XZ compression, use udif.

## vs. dmg-oxide

The [dmg-oxide](https://crates.io/crates/dmg-oxide) crate from the xbuild project:

| Feature | **udif** | dmg-oxide |
|---------|:-------:|:---------:|
| Read | ✓ | ✓ |
| Write | ✓ | ✓ |
| Zlib | ✓ | ✓ |
| LZFSE | ✓ | ❌ |
| XZ | ✓ | ❌ |
| Bzip2 | ✓ | ❌ |
| Documentation | ~80% | 12.2% |

**Verdict:** dmg-oxide only supports Zlib compression, which limits it to older DMG files. Most modern macOS DMGs use LZFSE and cannot be read by dmg-oxide.

## vs. dmg (crate)

The [dmg](https://crates.io/crates/dmg) crate is a thin wrapper around macOS `hdiutil` — it only works on macOS and cannot read/write DMG contents directly.

### Summary Table

| Crate | Read | Write | LZFSE | XZ | Bzip2 | Zlib | ADC | Encrypted | Platform |
|-------|:----:|:-----:|:-----:|:--:|:-----:|:----:|:---:|:---------:|----------|
| **udif** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ❌ | ❌ | All |
| dmgwiz | ✓ | ❌ | ✓ | ❌ | ✓ | ✓ | ✓ | ✓ | All |
| apple-dmg | ✓ | ❌ | ❌ | ❌ | ❌ | ✓ | ❌ | ❌ | All |
| dmg-oxide | ✓ | ✓ | ❌ | ❌ | ❌ | ✓ | ❌ | ❌ | All |
| dmg | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | macOS |

**udif is the only cross-platform Rust crate that supports both reading AND writing DMGs with modern compression formats (LZFSE, XZ).**
