# LovelyCarve integration

Magika ORT-compatible source audited at google/magika 26b6a9ba7e92f2b0a3745970a9190ec0dde9bf83 (Apache-2.0), version 1.1.1-dev.

Includes the upstream RAR MIME correction. Preserve existing async APIs and ort rc.12/ndarray compatibility. Upstream HEAD 2136599b6943fe32e3aef94e96620ea14cbc8e62 is 2.0.0-dev with an embedded tract runtime and unpublished path crates; migrating inference runtimes is deferred rather than silently changing model/runtime behavior. The latest crates.io release is 1.1.0, so this fork is not downgraded.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).
