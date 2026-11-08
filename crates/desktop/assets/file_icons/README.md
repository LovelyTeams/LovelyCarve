# File type icons

The SVG files in this directory are unmodified copies from Zed
(<https://github.com/zed-industries/zed>) at commit
`b5cd310b3b54dfc659fcc06453e6358843dfe216`: every file the desktop app draws
from `assets/icons/file_icons/`, plus `assets/icons/binary.svg` and
`assets/icons/envelope.svg`.

Zed's `assets` crate is licensed GPL-3.0-or-later; the license text is kept in
`LICENSE-GPL`. Zed notes that its icons derived from Lucide are under the ISC
license, which `THIRD_PARTY_NOTICES.md` reproduces. Language and tool logos
remain trademarks of their respective owners.

The desktop app serves these files as `lovelycarve/file_icons/<name>.svg`
(`crates/desktop/src/assets.rs`), and `crates/desktop/src/files/icons/file_types.rs`
maps file names to them. Tests keep this directory, the served list and the
mapping in step.
