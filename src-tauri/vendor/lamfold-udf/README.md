# lamfold-udf

This is LovelyCarve's hardened fork. See [LOVELYCARVE.md](LOVELYCARVE.md) and
[UDF support](../../../docs/formats/UDF.md) for its current supported profile.
The following describes the upstream project.


[![Crates.io](https://img.shields.io/crates/v/lamfold-udf.svg)](https://crates.io/crates/lamfold-udf)
[![Docs.rs](https://docs.rs/lamfold-udf/badge.svg)](https://docs.rs/lamfold-udf)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The UDF frontend of the [`lamfold`](../lamfold) read-only media stack:
> Clean-room UDF (ECMA-167 / OSTA UDF 1.02 to 2.01).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through `dispatch_fs_over_source` and co-resides with `lamfold-iso` on
UDF-Bridge optical media. Spec: `the lamfold design spec` §4.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-udf](https://docs.rs/lamfold-udf) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Capability | State |
| ---------- | ----- |
| Descriptor chain (AVDP, VDS, Partition and Logical Volume, File Set, root ICB) | Read |
| File Entries (tag 261) and **Extended File Entries (tag 266, UDF 2.00 and later, mkudffs's default)** | Read (EFE from 0.1.1) |
| File Identifier Descriptors; inline, short_ad and long_ad data; UCS-2 and Latin-1 names | Read |
| Metadata, sparable and VAT partition maps (UDF 2.50 and later), extended_ad, allocation-extent continuation | Refused or not used |

Verified against real `mkudffs` images (UDF 1.02 and 2.01) populated by the
kernel through a loop mount. Builds `no_std`; `#![forbid(unsafe_code)]`.

## Installation

```sh
cargo add lamfold-udf
```

## Clean-room posture

Derived only from the ECMA-167 and OSTA UDF specifications and the permissive
references in `NOTICE`. Linux `fs/udf` and `udftools` (GPL) are fenced off:
Never read or copied; udftools only builds the fixtures.

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with a fixture built by a real tool
(`mkudffs` and a loop mount, as `tests/fixtures/regenerate-udf201.sh` does).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
