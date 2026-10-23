# f2fs-reader

Read-only F2FS over the `lamfold::BlockSource` / `FoldFrontend` contracts.
Independent of Tauri and the LovelyCarve host. No filesystem mutation interface.
See [provenance and local changes](ORIGIN.md) and [supported scope](../../../docs/formats/F2FS.md).

```text
just test-vendor f2fs-reader
just clippy-f2fs
```
