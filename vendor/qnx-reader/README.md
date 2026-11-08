# qnx-reader

Independent read-only Rust QNX4 and QNX6 Power-Safe frontends over
`lamfold::BlockSource`. Standard QNX6 v4.3 and MMI layouts, both byte orders,
512–4096-byte blocks, committed superblock selection, QNX4 extent chains,
bounded random reads and timestamp roles. No mounting or writer APIs.

Original LovelyCarve implementation, AGPL-3.0-only. Format references and
validation limits: [QNX](../../docs/formats/QNX.md).
The NFI QNX-produced reference fixture is separately Apache-2.0; see
`tests/fixtures/README.md`. Invented fixtures and their Python generator are CC0.

```text
just test-qnx
just clippy-qnx
```
