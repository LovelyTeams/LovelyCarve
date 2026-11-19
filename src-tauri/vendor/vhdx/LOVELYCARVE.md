# LovelyCarve integration

Reader `vhdx-core` 0.4.1 from <https://github.com/SecurityRonin/vhdx-forensic>,
commit `b03f1f5c1be960d79af73f895017169171c00996`. Apache-2.0; see LICENSE.
The reader and its tests/fixtures are vendored; no analyzer, CLI or hosted CI.
Workspace manifest fields are expanded for standalone builds.

The application uses `core/evidence/readers/vhd_image.rs` for seek/read serialization, EOF
clamping and read-only resolution of relative VHDX parent locators. No native DLL
or whole-image temporary extraction is used.

Test with `just test-vhdx` and `just test vhd_image --lib`.
Committed fixtures run without external tooling; optional QEMU/corpus tests
return early when absent, so their harness status is not external validation.
Application and CLI full-stream hashes in `tests/data/LOVELYCARVE_ORACLE.json`
were captured using the previous independent libvhdi reader over these files.

Local changes include crate-relative fixture paths, bounded locator parsing, and
parent validation against the active header's DataWriteGuid and disk geometry.
File-backed opens retain upstream's bounded read path and dirty-log overlay.
