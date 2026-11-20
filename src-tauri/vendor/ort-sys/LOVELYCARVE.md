# LovelyCarve integration

`ort-sys` 2.0.0-rc.12 (MIT OR Apache-2.0) from crates.io, packaged from pykeio/ort 079ecb47034ec8188e3a06fc04f49ec28a6499e8. Only the native components workspace (`src-tauri/components`) uses it, through `[patch.crates-io]`; the backend crate does not link ONNX Runtime.

Local change, in `build/dynamic_link.rs` only: `copy_dylibs` (enabled by `oar-ocr-core` through `copy-dylibs`) took the profile directory as `OUT_DIR.ancestors().nth(3)`. Cargo's newer build-dir layout puts `OUT_DIR` at `<profile>/build/<package>/<hash>/out`, so the libraries were linked into `<profile>/build` and the build panicked on the missing `build/examples`, which broke every `ocr` / `ocr-cuda` build of the inference component. The patch takes the parent of the nearest `build` ancestor, which is correct for both layouts, and skips `examples/` and `deps/` when the layout has no such directory. The DLLs (CUDA and TensorRT execution providers, DirectML) again land in `target/<profile>/`, beside `lovelycarve_inference.dll`, where ONNX Runtime looks for its providers.

Everything else, including the pinned download list and hashes in `build/download/dist.txt`, is unchanged. The repository's `.gitattributes` keeps this directory LF on every checkout: the build script splits the embedded `dist.txt` on `\n`, so a CRLF checkout would leave `\r` in each hash and never find the cached binaries. Drop this copy once the components move to an `ort` release whose build script handles the layout.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).
