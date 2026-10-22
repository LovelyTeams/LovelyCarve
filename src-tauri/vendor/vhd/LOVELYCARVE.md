# LovelyCarve integration

Reader `vhd-core` 0.1.4 from <https://github.com/SecurityRonin/vhd-forensic>,
commit `2835523fe39c16740133dc8d51932cad46bcc4b6`. Apache-2.0; see LICENSE.
The reader and its tests/fixtures are vendored; no analyzer, CLI or hosted CI.
Workspace manifest fields are expanded for standalone builds.

The application uses `core/evidence/readers/vhd_image.rs` for seek/read serialization and EOF
clamping. Its existing command surface has no parent binding, so differencing
disks are rejected. No native DLL or whole-image temporary extraction is used.

Test with `just test-vhd` and `just test vhd_image --lib`.
Committed fixtures run without external tooling; optional QEMU/corpus tests
return early when absent, so their harness status is not external validation.
Application and CLI full-stream hashes in `tests/data/LOVELYCARVE_ORACLE.json`
were captured using the previous independent libvhdi reader over these files.

Local reader changes:
- Read only the 512-byte footer, 1024-byte dynamic header and bounded BAT.
  BAT allocation is capped at 64 MiB and must cover the virtual capacity.
- Validate fixed payload and allocated dynamic block bounds; reject invalid
  block geometry and truncated reads, rather than returning premature EOF.
- Round sector bitmap storage up even for blocks smaller than 4096 bytes.
- Check seeks with i128 arithmetic; no signed overflow or wraparound.
- Extend the upstream 4 MiB-block test fixture to include its full declared
  payload. Add a 1 TiB virtual-source test which rejects any payload read on open.
