# Third-Party Notices

## Scope and dependency records

LovelyCarve's own code is licensed under AGPL-3.0-only; third-party code,
fonts, models, libraries and test data retain their original notices.
The application dependency versions are recorded in the workspace
`Cargo.lock`; the separate plugin workspace has its own manifests
and lockfile. Vendored components retain their license and provenance files.
The read-only Python extension records its Rust dependencies in
`python/Cargo.lock` and Python environment in `python/uv.lock`. It uses PyO3
(MIT OR Apache-2.0) for bindings and maturin (MIT OR Apache-2.0) as its build
backend; their original notices remain in the upstream package distributions.
This document preserves notices for incorporated components and derived code;
it is not an automatically generated inventory of all transitive packages.
Release verification is documented in [docs/RELEASING.md](docs/RELEASING.md).

The 2026-10-12 vendored-source audit, selected upstream revisions and retained
local patches are recorded in [vendor/UPSTREAM.md](vendor/UPSTREAM.md).
The updated Apple readers use APFS 0.4.1, HFS+ 0.4.0, UDIF 0.4.2 and the new
shared `cmpfs` 0.1.0 decoder from Dil4rd/dpp (MIT). Their license texts and
integration notes remain in each crate directory. ext4-view 1.0.0 and backhand
0.25.5 retain their MIT / Apache-2.0 texts; rsqlite-vfs 0.2.0 retains its MIT
license. Original data sources and hashes for the ext4 unit-test fixtures are
recorded in `vendor/ext4-view/test_data/README.md`.

The Buffalo ENC / TRX and DD-WRT SquashFS compatibility work uses Firmware Mod
Kit commit `c72f45d7a062156125ae00cb867d8a614b50b963` as a format reference
(Gabor Juhos's GPL-2.0-only Buffalo tools and the bundled SquashFS/LZMA variant).
No FMK C/C++ source, shell scripts or executables are incorporated. Original Rust
readers use the existing backhand (MIT OR Apache-2.0) library and lzma-rust2
(Apache-2.0). Original synthetic fixtures and their generators are CC0-1.0;
see [firmware provenance and limits](docs/formats/FIRMWARE.md).

SquashFS XZ and Zstandard decoding also reuse lzma-rust2 and the existing
MIT-licensed ms-compress-ruzstd 0.9.1. The firmware read path does not call
liblzma or libzstd; independent fixtures use Python encoders as test oracles.

SafeLoader, legacy uImage and Netgear CHK format behavior is checked against
OpenWrt firmware-utils commit `b2a93c6f77bac3a6525e93b383c987b073b4633b`:
Matthias Schiffer's `tplink-safeloader.c` (BSD-2-Clause), Dave C. Reeve's
`mkchkimg.c` (GPL-2.0-or-later), and `uimage_padhdr.c` (GPL-2.0-only).
These C sources and executables are not incorporated. The Rust envelope readers
reuse the existing MD5/CRC implementations; independent synthetic encoders and
fixtures are CC0-1.0. See the firmware format guide for exact compatibility limits.

The APFS name lookup uses the unmodified crates.io packages
[`unicode-casefold` 0.2.0](https://crates.io/crates/unicode-casefold/0.2.0)
by Chris Wong and [`unicode-normalization` 0.1.25](https://crates.io/crates/unicode-normalization/0.1.25)
by the Rust Project Developers. Both declare MIT OR Apache-2.0;
versions and dependency sources are pinned in the Rust lockfiles.

The libfsapfs APFS comparison uses its documented structures and public API;
no libfsapfs C sources or binaries are incorporated. The new APFS fixtures
are invented, reproducible project test structures (AGPL-3.0-only), documented
in `vendor/apfs/tests/data/README.md`.

## Native YAFFS reader

The original AGPL-3.0-only `yaffs2-reader` adapts object/file prefix structures
and validation from binwalk-ng commit `845806c163747eb524d66b5fe3de2ed86ea3aaad`
(MIT, Copyright 2024 devttys0 and 2026 binwalk-ng contributors). Its full notice
is retained in [LICENSE-BINWALK-MIT](vendor/yaffs2-reader/LICENSE-BINWALK-MIT).
Aleph One YAFFS format definitions are references; no C implementation or
external extractor is incorporated. Independent synthetic fixtures are CC0-1.0.
The original YAFFS1 dispersed-tags, serial scanning and page-ECC modules reuse
the same Rust namespace interfaces; they add no C implementation or dependency.

## lamfold CramFS / Linux ROMFS

The CramFS / Linux ROMFS readers use Lamco Development LLC's `lamfold 0.1.5`,
`lamfold-cramfs 0.1.0` and `lamfold-romfs 0.1.1` (MIT OR Apache-2.0).
The original LICENSE-MIT, LICENSE-APACHE and NOTICE files remain in each vendor
directory; provenance and local integrity/range-read patches are recorded in
each `LOVELYCARVE.md` and [ROM image support](docs/formats/ROM_IMAGES.md).
The original synthetic ROM filesystem images and encoder are CC0-1.0.

## rust_ubidump

The UBI/UBIFS readers adapt the UBI headers/CRC and UBIFS node, key, inode
and index structures and field decoders from
[aspnmy/rust_ubidump](https://github.com/aspnmy/rust_ubidump), revision
`19782c3eabc25a1baab7faefe12fe6f4bd362dac`, under MIT. Its original license
credits Copyright (c) 2017 Willem Hengeveld <itsme@xs4all.nl>.
The complete license is retained in `vendor/ubidump/LICENSE`;
library extraction, safety fixes and the rewritten bounded volume mapping are
documented in `vendor/ubidump/LOVELYCARVE.md`. The upstream CLI and
C LZO dependency are not incorporated. UBIFS uses the existing flate2/zlib-rs
packages, lzokay 2.0.1 (MIT), and ms-compress-ruzstd 0.9.1 (MIT). Full codec
notices are retained below and in the vendor directory. Synthetic fixtures and
their independent generators are original CC0-1.0 data.

### lzokay-2.0.1

Source: https://github.com/encounter/lzokay-rs

MIT License

Copyright 2021 Luke Street.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


### ms-compress-ruzstd-0.9.1

Source: https://github.com/CaddyGlow/zstd-rs

MIT License

Copyright (c) 2019 Moritz Borcherding

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


## CLI timezone rules

The CLI uses the unmodified `jiff` crate by Andrew Gallant (Unlicense OR MIT)
and its bundled `jiff-tzdb` IANA time zone database. Versions are pinned in
`Cargo.lock`; crate license texts remain in the distributed source
packages. IANA time zone data is in the public domain. Evidence-provided TZif
files are read from the user's evidence and are not incorporated or shipped.

## fast-md5

MD5 hashing uses the unmodified crates.io package
[`fast-md5` 1.0.0](https://github.com/ogital-net/fast-md5), licensed BSD-2-Clause.
Its x86_64 / aarch64 assembly derives from
[`animetosho/md5-optimisation`](https://github.com/animetosho/md5-optimisation),
released into the public domain by its author. The Rust glue and tests retain
the following notice; the exact package is pinned in `Cargo.lock`.

```text
BSD 2-Clause License

Copyright (c) 2026, Latigo LLC

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Parallels image capability reference

`vendor/phdi` is original LovelyCarve Rust code (AGPL-3.0-only),
referencing the public format and API of Joachim Metz and contributors'
[libyal/libphdi fb132dceb2a1](https://github.com/libyal/libphdi/commit/fb132dceb2a1a43c2597b10bbf9e65757d08cf7c).
The upstream library is LGPL-3.0-or-later, its tools GPL-3.0-or-later,
and its format document GFDL-1.3-or-later. No upstream C implementation,
document text, test data or binaries are incorporated. The runtime uses
the unmodified [quick-xml 0.42.0](https://github.com/tafia/quick-xml) crate
under MIT; dependency versions remain pinned in Cargo.lock.
Original synthetic tests and an independent Python oracle accompany the reader.
The public dfVFS test image was verified outside the repository and is not
redistributed. Provenance and all 56 API comparisons are recorded in
[PHDI](docs/formats/PHDI.md) and the vendor ORIGIN.md / LICENSE.

## RAW image capability reference

`vendor/smraw` is original LovelyCarve Rust code (AGPL-3.0-only),
referencing the public RAW I/O and naming capabilities of Joachim Metz and
contributors' [libyal/libsmraw](https://github.com/libyal/libsmraw/commit/adfa6b8bc8617226805926180244c774366aad30).
The upstream library is LGPL-3.0-or-later and its tools are GPL-3.0-or-later.
No upstream C implementation, document text, test data, binaries or bindings
are incorporated. This crate has no third-party runtime dependencies.
Its license, provenance and scope remain in its vendor directory; all 72
public APIs are compared in [SMRAW](docs/formats/SMRAW.md).
The project-generated synthetic tests contain no real evidence.

## QCOW reader capability reference

`vendor/qcow2` retains Dave Vasilevsky's MIT-licensed qcow2-rs
feature-name parsing and utility/error infrastructure. Original LovelyCarve
QCOW1/2/3 header/read/chain/crypto implementations and invented new test fixtures
are AGPL-3.0-only; both LICENSE-MIT and LICENSE-AGPL are retained in the crate.
Its manifest records `MIT AND AGPL-3.0-only`. Exact origin and change boundaries
are recorded in [LOVELYCARVE.md](vendor/qcow2/LOVELYCARVE.md).

Joachim Metz and contributors' [libyal/libqcow](https://github.com/libyal/libqcow/commit/3e390fbb465f37f3da92c1362c5df3da04ff9efe)
is a public capability/format reference, not incorporated code. It is
LGPL-3.0-or-later; its format document is GFDL-1.3-or-later. No C sources,
documentation text, upstream test data, binaries or bindings are copied.
All 42 public declarations are compared in [QCOW](docs/formats/QCOW.md).
New dependencies use unmodified RustCrypto aes 0.9.3 / zeroize 1.9.1
(MIT OR Apache-2.0) and flate2 1.1.10 (MIT OR Apache-2.0), using pure Rust
miniz_oxide rather than native zlib. Versions are pinned in the Rust lockfiles.
Independent Python zlib/OpenSSL fixture generation and SHA-256 records remain
in the test directory; those packages are not runtime dependencies.

## GZIP reader capability reference

`vendor/gzipf` is original LovelyCarve Rust code (AGPL-3.0-only),
using RFC 1952 and the public capabilities of Joachim Metz's
[libyal/libgzipf](https://github.com/libyal/libgzipf/commit/d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68)
as references. The upstream implementation is LGPL-3.0-or-later and its
format documentation is GFDL-1.3-or-later. No upstream C code, document text,
test data, binaries or bindings are incorporated. The new crate's license,
provenance, API comparison and invented test-data records are retained in its
directory and [docs/formats/GZIP.md](docs/formats/GZIP.md).

The crate uses `flate2` 1.1.10 and `crc32fast` 1.5.2, both declaring
MIT OR Apache-2.0. DEFLATE uses the pure Rust miniz_oxide backend; it does not
link native zlib. Exact transitive versions are pinned in the crate's own
Cargo.lock; the host Cargo.lock also records the path dependency. Host tar.gz,
Docker/OCI GZIP layers and Xiaomi GZIP backups use this reader. The independent
test corpus is invented project data encoded by
Python's C zlib, licensed AGPL-3.0-only, with a reproducible generator and hashes.

## GPUI Kit and Lucide icons

The desktop app is built with GPUI Kit 0.9.0 (`gpui-kit`, `gpui-base`,
`gpui-component` and `gpui-kit-assets`, Apache-2.0,
<https://github.com/longbridge/gpui-kit>) and `gpui-pre` 0.3.11, a snapshot of
Zed's GPUI framework (Apache-2.0, <https://github.com/zed-industries/zed>). The
complete Apache-2.0 terms are reproduced in the EWF section below.

The executable embeds icons from `gpui-kit-assets`, which bundles the Lucide
icon set. Its license, as retained in that crate's `LICENSE-LUCIDE` (which also
lists the icons derived from Feather):

ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

The MIT License (MIT) (for the icons derived from Feather)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Zed file type icons

`crates/desktop/assets/file_icons/` holds unmodified SVG files from Zed
(<https://github.com/zed-industries/zed>, commit
`b5cd310b3b54dfc659fcc06453e6358843dfe216`, by Zed Industries, Inc. and
contributors): the file type, folder and chevron icons under
`assets/icons/file_icons/`, plus `assets/icons/binary.svg` and
`assets/icons/envelope.svg`. The desktop app embeds them to draw the file list
and directory tree. Zed's `assets` crate is licensed GPL-3.0-or-later; the
license text is kept beside the icons in `LICENSE-GPL` and reproduced at the
end of this file. Zed records that its icons derived from Lucide are under
the ISC license reproduced in the previous section. Language, framework and
tool logos remain trademarks of their respective owners.

## Models and runtime resources

WeChat QRCode model notices and license terms are retained in
[`resources/qrcode/MODEL_LICENSE`](resources/qrcode/MODEL_LICENSE).
Runtime libraries under `binaries` and OCR models under
`resources/ocr` must be checked against their actual upstream
distribution before release.

`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll` and
`msvcp140_1.dll` under `binaries` are the unmodified,
Microsoft-signed Visual C++ runtime from the Visual Studio redistributable
folder (`VC\Redist\MSVC\14.50.35710\x64\Microsoft.VC145.CRT`, file version
14.50.35719.0). They are deployed next to the executables because the desktop
app and the CLI link the C++ standard library dynamically, and
`lc-plugin-host.exe`, the bundled `linuxinfo` plugin and OpenSSL's
`libcrypto-4-x64.dll` link the Visual C++ runtime dynamically. They are
redistributed as Distributable Code under the Microsoft Visual Studio license
terms (<https://aka.ms/vs/18/redistribution>); the Universal CRT they use ships
with Windows 10 and later. The resources shipped depend on the build
features; a package's Rust license does not by itself document
the provenance of every separately bundled DLL or model.

The Quicksand font notice and OFL 1.1 terms are reproduced at the end of this
file, so portable packages that include this document retain the font license.

## ReFS Rust reader and comparison tools

The production Rust reader at `vendor/refs-reader` ports parsing
algorithms from Erik Larsson's refsprogs, with b3nguang's stream additions,
revision `391c1bd56986142fc359288b7d2cb41cb3eb3680` (GPL-2.0-or-later).
The upstream GPL text is retained in that crate's `LICENSE`; its `ORIGIN.md`
records source provenance and validated scope. Production ReFS reads use this
crate; the legacy DLL is retained only for explicit comparison builds.

`probe_refs_rust` uses the unmodified crates.io releases `refs-core` 0.1.0
(SecurityRonin/refs-forensic, Apache-2.0, Albert Hui and contributors) and
`librefs` 0.1.0 (MIT, Copyright (c) 2026 eWloYW8). The MIT terms reproduced
in the Digler section below also apply to `librefs`; the Apache-2.0 terms
are reproduced in the EWF section. Exact package checksums are recorded in
`Cargo.lock`. These candidates are not used by the desktop/CLI
ReFS adapter. Public disk images remain untracked under `fixtures/`; their
sources and hashes are recorded in `docs/formats/REFS.md`.

## ios-backup-core / ios-backup-forensic (SecurityRonin)

The vendored iOS backup reader and anomaly auditor in `vendor/ios-backup`
are derived from https://github.com/SecurityRonin/ios-backup-forensic, commit
`8d8cef447ae212e7dd22dc0425156bf4ddfcfa05`. Copyright Albert Hui and contributors.
Licensed under Apache-2.0 (full terms are reproduced in the EWF section below
and retained in `vendor/ios-backup/LICENSE`). Local changes are listed
in `vendor/ios-backup/UPSTREAM.md`; synthetic encrypted fixtures retain
their upstream provenance.

## Digler

The raw file-carving scan loop and format-specific carving logic in
`crates/core/src/carving/carver*.rs` and
`crates/core/src/carving/formats/` are Rust ports derived from Digler:
<https://github.com/ostafen/digler>.

Copyright (c) 2025 Stefano Scafiti

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Go standard library-derived scanners

The PNG, JPEG, and GIF scanners port logic that Digler derived from Go's
standard library.

Copyright (c) 2009 The Go Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of Google Inc. nor the names of its contributors may be
   used to endorse or promote products derived from this software without
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## ewf (SecurityRonin/ewf-forensic)

The vendored pure Rust EWF reader in `vendor/ewf` is derived from
https://github.com/SecurityRonin/ewf-forensic, core crate 0.4.10, commit
`c456f993f68385f3438b3ff16ba5ab6fd0850fb4`. Copyright Albert Hui and contributors.
Licensed under Apache-2.0; the complete license follows. Local modifications are
documented in `vendor/ewf/LOVELYCARVE.md`.


                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.

## vmdk-core (vmdk-forensic)

The read-only VMDK reader in `vendor/vmdk` is derived from
[SecurityRonin/vmdk-forensic](https://github.com/SecurityRonin/vmdk-forensic),
version 0.8.4, commit `8a9831125c6db5cff9a24414481037487a92d410`, by Albert Hui.
Licensed under Apache License 2.0 (full text reproduced in the EWF notice above
and in `vendor/vmdk/LICENSE`). Local changes are documented in
`vendor/vmdk/LOVELYCARVE.md`. Test-fixture origins, including
log2timeline/dfvfs and log2timeline/plaso, are retained in `tests/data/README.md`.

## bitlocker-core (bitlocker-forensic)

The BitLocker reader in `vendor/bitlocker` is derived from
[SecurityRonin/bitlocker-forensic](https://github.com/SecurityRonin/bitlocker-forensic),
version 0.3.5, commit `e6674dcda2413b1a02a915f2255af8fda787a154`, by Albert Hui.
Licensed under Apache License 2.0 (full text reproduced in the EWF notice above
and in `vendor/bitlocker/LICENSE`). Local changes and test-fixture
provenance are documented alongside the crate. The minimized `bdetogo.raw`
projection is derived from the Apache-2.0 dfVFS test corpus by Joachim Metz
and the dfVFS project: <https://github.com/log2timeline/dfvfs>.

The local BitLocker format comparison uses documentation and observable read
semantics from [libyal/libbde](https://github.com/libyal/libbde), by Joachim Metz
and contributors (LGPL-3.0-or-later library; GPL tools). No libbde C implementation
is linked or vendored. The new synthetic cipher projections are LovelyCarve
Apache-2.0 contributions; generation and digests accompany the fixtures.

## VHD / VHDX readers

`vendor/vhd` contains `vhd-core` 0.1.4 from
<https://github.com/SecurityRonin/vhd-forensic>, commit
`2835523fe39c16740133dc8d51932cad46bcc4b6`.
`vendor/vhdx` contains `vhdx-core` 0.4.1 from
<https://github.com/SecurityRonin/vhdx-forensic>, commit
`b03f1f5c1be960d79af73f895017169171c00996`.
Both are by Albert Hui / Security Ronin, licensed under Apache-2.0 (full license
in each vendor directory and above). Local changes are listed in `LOVELYCARVE.md`.
The reader crates are included; the separate forensic analyzers are not.

Committed regression fixtures include Apache-2.0 log2timeline/dfvfs images by
Joachim Metz and contributors, plus QEMU-generated samples. Their source URLs,
checksums and generation details are retained in the respective `tests/data`
README and SOURCES files. Independent decoded hashes are recorded separately in
`LOVELYCARVE_ORACLE.json`; the reference DLL is not distributed.

## VeraCrypt / LUKS readers

`veracrypt-core` 0.2.4 from <https://github.com/SecurityRonin/veracrypt-forensic>,
commit `e3883a5b347f50b51de70ca08655b2fd4b0885b2`, and `luks-core` 0.1.8 from
<https://github.com/SecurityRonin/luks-forensic>, commit
`284f06487d7a66f13ed27c61be5e26f0863c0e4f`, are by Albert Hui / Security Ronin.
Both are Apache-2.0; licenses and integration notes are retained in
`vendor/veracrypt` and `vendor/luks`. Only the core readers
are integrated, not the separate forensic analyzers. The committed encrypted
integration fixtures are generated independently using Python cryptography;
their generator and public test credentials are included.

## Volume Shadow Copy reader

`vendor/vsc` contains `vsc-core` 0.2.2 from
<https://github.com/SecurityRonin/vsc-forensic>, commit
`0af70bed6910fef542224ecd94928b4bc0ae920b`, by Albert Hui / Security Ronin,
licensed under Apache-2.0 (full license in the vendor directory). Only the core
reader is integrated, not the forensic analyzer; local changes are listed in
`LOVELYCARVE.md`. The store-chain reconstruction follows the rules published in
the libvshadow "Volume Shadow Snapshot (VSS) format" documentation by Joachim
Metz and is differentially tested against libvshadow-python; the complete public
API/source comparison is recorded in `docs/formats/VSS.md`. No libvshadow code
is included or distributed. The dfVFS `vss.raw` image used by the optional
oracle test is not committed.

## AD1 logical-image reader

`vendor/ad1` contains `ad1-core` 0.1.2 from
<https://github.com/SecurityRonin/ad1-forensic>, commit
`afc3963659a13acf10082e7d76194d3ab789583e`, by Albert Hui / Security Ronin,
licensed under Apache-2.0 (full license in the vendor directory). Only the core
reader is integrated, not the `ad1-forensic` auditor; local changes are listed in
`LOVELYCARVE.md`. The on-disk layout follows the reverse-engineered
al3ks1s/AD1-tools reference; no AD1-tools code is included or distributed. The
tests build their AD1 fixtures in memory; no AD1 image is committed.

## SQLite record carver

`vendor/sqlite-core` contains `sqlite-core` 0.12.0 from
<https://github.com/SecurityRonin/sqlite-forensic>, commit
`df511309e086825fb194ea8a721e94bbd0033fe0`, by Security Ronin, licensed under
Apache-2.0 (full license in the vendor directory). Its `src/recovery.rs` is
ported from the `forensic` crate of the same repository and commit; the rest of
that crate is not integrated. Local changes are listed in `LOVELYCARVE.md`.

Committed test fixtures, with sources and hashes in `tests/data/README.md` and
`tests/data/cfreds/README.md`: synthetic databases written with SQLite and the
SQLCipher CLI; three databases from the Nemetz SQLite Forensic Corpus v2.0
(CC0); the NIST CFReDS / CFTT SQLite test sets SFT-01 and SFT-03 (U.S.
Government work, public domain, attributed to NIST); and the
`tests/data/lovelycarve` scenarios generated by `make_corpus.py`.

## ShimCache parser

The Windows compatibility-cache plugin depends on `shimcache-core` 0.1.0 by
Security Ronin, from <https://github.com/SecurityRonin/shimcache-forensic>, under
Apache-2.0. The crates.io archive SHA256 is
`b5221566b7ca734a3e90f783276b251213474df5ff9a487a41b762dd0b489910`.
Only the core parser is integrated, with no upstream code changes. LovelyCarve
adds strict framing/Unicode validation, source-cell mapping and bounded projection;
real evidence and independent reference scripts are not distributed.

## Quicksand fonts

`crates/desktop/assets/fonts/Quicksand-700.ttf`, embedded in the desktop app, contains
the following copyright notice in its font metadata:

Copyright 2019 The Quicksand Project Authors (https://github.com/andrew-paglinawan/QuicksandFamily.git), with Reserved Font Name "Quicksand"

The font family is licensed under SIL Open Font License 1.1.
The [upstream license](https://github.com/andrew-paglinawan/QuicksandFamily/blob/master/OFL.txt)
is retained below and in `crates/desktop/assets/fonts/OFL.txt`, together
with the copyright notice from the bundled file. The font binary is unchanged.

Copyright 2011 The Quicksand Project Authors (https://github.com/andrew-paglinawan/QuicksandFamily), with Reserved Font Name “Quicksand”.

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.

## NTFS WOF codecs (ms-compress 0.2.0)

Source: https://github.com/CaddyGlow/ms-compress. Registry checksum is pinned in
`Cargo.lock`. Only std/xpress/lzx are enabled. The package declares
LGPL-2.1-only AND LGPL-2.1-or-later AND Zlib AND Apache-2.0 AND MIT;
selected LZX/XPRESS sources derive from wimlib, and LZX includes CAB/Delta helpers.
The unmodified source and dependency manifests form part of the corresponding
source available for rebuilding LovelyCarve. Generated fixture provenance is
in `crates/core/src/filesystem/volumes/ntfs/testdata/README.md`.


### ms-compress NOTICE.md

# Codec provenance

This crate is LGPL-2.1-or-later. See LICENSE-LGPL-2.1. The implementation is
native safe Rust and does not link or invoke the upstream C decoder.

Behavioral reference: wimlib cd5e231c348c255ae5088873b5a66ee0eb96fa07:
- src/xpress_decompress.c: Copyright (C) 2012-2016 Eric Biggers,
  LGPL-2.1-or-later.
- include/wimlib/xpress_constants.h: format constants.
- include/wimlib/decompress_common.h and src/decompress_common.c:
  Copyright 2022 Eric Biggers, MIT. Their notice is retained below because
  the bitstream and canonical-code semantics informed the Rust port.

Copyright 2022 Eric Biggers

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.



### ms-compress LZX-NOTICE.md

# LZX decoder provenance

The native safe Rust implementation in lzx.rs is a behavioral port of wimlib
revision cd5e231c348c255ae5088873b5a66ee0eb96fa07, src/lzx_decompress.c,
src/lzx_common.c, include/wimlib/lzx_constants.h and include/wimlib/lzx_common.h.
The decoder and common preprocessing code carry Copyright (C) 2012-2016
Eric Biggers and LGPL-2.1-or-later. This Rust implementation retains that
license. See LICENSE-LGPL-2.1 in this directory.

The canonical-code and input-bitstream semantics also follow
include/wimlib/decompress_common.h and src/decompress_common.c, Copyright
2022 Eric Biggers, MIT. The complete MIT notice is preserved in NOTICE.md
in this directory. No upstream C code is compiled, loaded or executed by
this module or its Rust tests.

The libFuzzer corpus copied into tests/fixtures/lzx-libfuzzer is test material
from the upstream project, distributed under its GPL-3.0-or-later option;
see tests/fixtures/LICENSE-GPL-3.0. Generated original-C snapshots and their
standalone standard-library Python generator retain their explicit provenance.


### ms-compress LICENSE-LGPL-2.1

                  GNU LESSER GENERAL PUBLIC LICENSE
                       Version 2.1, February 1999

 Copyright (C) 1991, 1999 Free Software Foundation, Inc.
 <https://fsf.org/>
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

[This is the first released version of the Lesser GPL.  It also counts
 as the successor of the GNU Library Public License, version 2, hence
 the version number 2.1.]

                            Preamble

  The licenses for most software are designed to take away your
freedom to share and change it.  By contrast, the GNU General Public
Licenses are intended to guarantee your freedom to share and change
free software--to make sure the software is free for all its users.

  This license, the Lesser General Public License, applies to some
specially designated software packages--typically libraries--of the
Free Software Foundation and other authors who decide to use it.  You
can use it too, but we suggest you first think carefully about whether
this license or the ordinary General Public License is the better
strategy to use in any particular case, based on the explanations below.

  When we speak of free software, we are referring to freedom of use,
not price.  Our General Public Licenses are designed to make sure that
you have the freedom to distribute copies of free software (and charge
for this service if you wish); that you receive source code or can get
it if you want it; that you can change the software and use pieces of
it in new free programs; and that you are informed that you can do
these things.

  To protect your rights, we need to make restrictions that forbid
distributors to deny you these rights or to ask you to surrender these
rights.  These restrictions translate to certain responsibilities for
you if you distribute copies of the library or if you modify it.

  For example, if you distribute copies of the library, whether gratis
or for a fee, you must give the recipients all the rights that we gave
you.  You must make sure that they, too, receive or can get the source
code.  If you link other code with the library, you must provide
complete object files to the recipients, so that they can relink them
with the library after making changes to the library and recompiling
it.  And you must show them these terms so they know their rights.

  We protect your rights with a two-step method: (1) we copyright the
library, and (2) we offer you this license, which gives you legal
permission to copy, distribute and/or modify the library.

  To protect each distributor, we want to make it very clear that
there is no warranty for the free library.  Also, if the library is
modified by someone else and passed on, the recipients should know
that what they have is not the original version, so that the original
author's reputation will not be affected by problems that might be
introduced by others.

  Finally, software patents pose a constant threat to the existence of
any free program.  We wish to make sure that a company cannot
effectively restrict the users of a free program by obtaining a
restrictive license from a patent holder.  Therefore, we insist that
any patent license obtained for a version of the library must be
consistent with the full freedom of use specified in this license.

  Most GNU software, including some libraries, is covered by the
ordinary GNU General Public License.  This license, the GNU Lesser
General Public License, applies to certain designated libraries, and
is quite different from the ordinary General Public License.  We use
this license for certain libraries in order to permit linking those
libraries into non-free programs.

  When a program is linked with a library, whether statically or using
a shared library, the combination of the two is legally speaking a
combined work, a derivative of the original library.  The ordinary
General Public License therefore permits such linking only if the
entire combination fits its criteria of freedom.  The Lesser General
Public License permits more lax criteria for linking other code with
the library.

  We call this license the "Lesser" General Public License because it
does Less to protect the user's freedom than the ordinary General
Public License.  It also provides other free software developers Less
of an advantage over competing non-free programs.  These disadvantages
are the reason we use the ordinary General Public License for many
libraries.  However, the Lesser license provides advantages in certain
special circumstances.

  For example, on rare occasions, there may be a special need to
encourage the widest possible use of a certain library, so that it becomes
a de-facto standard.  To achieve this, non-free programs must be
allowed to use the library.  A more frequent case is that a free
library does the same job as widely used non-free libraries.  In this
case, there is little to gain by limiting the free library to free
software only, so we use the Lesser General Public License.

  In other cases, permission to use a particular library in non-free
programs enables a greater number of people to use a large body of
free software.  For example, permission to use the GNU C Library in
non-free programs enables many more people to use the whole GNU
operating system, as well as its variant, the GNU/Linux operating
system.

  Although the Lesser General Public License is Less protective of the
users' freedom, it does ensure that the user of a program that is
linked with the Library has the freedom and the wherewithal to run
that program using a modified version of the Library.

  The precise terms and conditions for copying, distribution and
modification follow.  Pay close attention to the difference between a
"work based on the library" and a "work that uses the library".  The
former contains code derived from the library, whereas the latter must
be combined with the library in order to run.

                  GNU LESSER GENERAL PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. This License Agreement applies to any software library or other
program which contains a notice placed by the copyright holder or
other authorized party saying it may be distributed under the terms of
this Lesser General Public License (also called "this License").
Each licensee is addressed as "you".

  A "library" means a collection of software functions and/or data
prepared so as to be conveniently linked with application programs
(which use some of those functions and data) to form executables.

  The "Library", below, refers to any such software library or work
which has been distributed under these terms.  A "work based on the
Library" means either the Library or any derivative work under
copyright law: that is to say, a work containing the Library or a
portion of it, either verbatim or with modifications and/or translated
straightforwardly into another language.  (Hereinafter, translation is
included without limitation in the term "modification".)

  "Source code" for a work means the preferred form of the work for
making modifications to it.  For a library, complete source code means
all the source code for all modules it contains, plus any associated
interface definition files, plus the scripts used to control compilation
and installation of the library.

  Activities other than copying, distribution and modification are not
covered by this License; they are outside its scope.  The act of
running a program using the Library is not restricted, and output from
such a program is covered only if its contents constitute a work based
on the Library (independent of the use of the Library in a tool for
writing it).  Whether that is true depends on what the Library does
and what the program that uses the Library does.

  1. You may copy and distribute verbatim copies of the Library's
complete source code as you receive it, in any medium, provided that
you conspicuously and appropriately publish on each copy an
appropriate copyright notice and disclaimer of warranty; keep intact
all the notices that refer to this License and to the absence of any
warranty; and distribute a copy of this License along with the
Library.

  You may charge a fee for the physical act of transferring a copy,
and you may at your option offer warranty protection in exchange for a
fee.

  2. You may modify your copy or copies of the Library or any portion
of it, thus forming a work based on the Library, and copy and
distribute such modifications or work under the terms of Section 1
above, provided that you also meet all of these conditions:

    a) The modified work must itself be a software library.

    b) You must cause the files modified to carry prominent notices
    stating that you changed the files and the date of any change.

    c) You must cause the whole of the work to be licensed at no
    charge to all third parties under the terms of this License.

    d) If a facility in the modified Library refers to a function or a
    table of data to be supplied by an application program that uses
    the facility, other than as an argument passed when the facility
    is invoked, then you must make a good faith effort to ensure that,
    in the event an application does not supply such function or
    table, the facility still operates, and performs whatever part of
    its purpose remains meaningful.

    (For example, a function in a library to compute square roots has
    a purpose that is entirely well-defined independent of the
    application.  Therefore, Subsection 2d requires that any
    application-supplied function or table used by this function must
    be optional: if the application does not supply it, the square
    root function must still compute square roots.)

These requirements apply to the modified work as a whole.  If
identifiable sections of that work are not derived from the Library,
and can be reasonably considered independent and separate works in
themselves, then this License, and its terms, do not apply to those
sections when you distribute them as separate works.  But when you
distribute the same sections as part of a whole which is a work based
on the Library, the distribution of the whole must be on the terms of
this License, whose permissions for other licensees extend to the
entire whole, and thus to each and every part regardless of who wrote
it.

Thus, it is not the intent of this section to claim rights or contest
your rights to work written entirely by you; rather, the intent is to
exercise the right to control the distribution of derivative or
collective works based on the Library.

In addition, mere aggregation of another work not based on the Library
with the Library (or with a work based on the Library) on a volume of
a storage or distribution medium does not bring the other work under
the scope of this License.

  3. You may opt to apply the terms of the ordinary GNU General Public
License instead of this License to a given copy of the Library.  To do
this, you must alter all the notices that refer to this License, so
that they refer to the ordinary GNU General Public License, version 2,
instead of to this License.  (If a newer version than version 2 of the
ordinary GNU General Public License has appeared, then you can specify
that version instead if you wish.)  Do not make any other change in
these notices.

  Once this change is made in a given copy, it is irreversible for
that copy, so the ordinary GNU General Public License applies to all
subsequent copies and derivative works made from that copy.

  This option is useful when you wish to copy part of the code of
the Library into a program that is not a library.

  4. You may copy and distribute the Library (or a portion or
derivative of it, under Section 2) in object code or executable form
under the terms of Sections 1 and 2 above provided that you accompany
it with the complete corresponding machine-readable source code, which
must be distributed under the terms of Sections 1 and 2 above on a
medium customarily used for software interchange.

  If distribution of object code is made by offering access to copy
from a designated place, then offering equivalent access to copy the
source code from the same place satisfies the requirement to
distribute the source code, even though third parties are not
compelled to copy the source along with the object code.

  5. A program that contains no derivative of any portion of the
Library, but is designed to work with the Library by being compiled or
linked with it, is called a "work that uses the Library".  Such a
work, in isolation, is not a derivative work of the Library, and
therefore falls outside the scope of this License.

  However, linking a "work that uses the Library" with the Library
creates an executable that is a derivative of the Library (because it
contains portions of the Library), rather than a "work that uses the
library".  The executable is therefore covered by this License.
Section 6 states terms for distribution of such executables.

  When a "work that uses the Library" uses material from a header file
that is part of the Library, the object code for the work may be a
derivative work of the Library even though the source code is not.
Whether this is true is especially significant if the work can be
linked without the Library, or if the work is itself a library.  The
threshold for this to be true is not precisely defined by law.

  If such an object file uses only numerical parameters, data
structure layouts and accessors, and small macros and small inline
functions (ten lines or less in length), then the use of the object
file is unrestricted, regardless of whether it is legally a derivative
work.  (Executables containing this object code plus portions of the
Library will still fall under Section 6.)

  Otherwise, if the work is a derivative of the Library, you may
distribute the object code for the work under the terms of Section 6.
Any executables containing that work also fall under Section 6,
whether or not they are linked directly with the Library itself.

  6. As an exception to the Sections above, you may also combine or
link a "work that uses the Library" with the Library to produce a
work containing portions of the Library, and distribute that work
under terms of your choice, provided that the terms permit
modification of the work for the customer's own use and reverse
engineering for debugging such modifications.

  You must give prominent notice with each copy of the work that the
Library is used in it and that the Library and its use are covered by
this License.  You must supply a copy of this License.  If the work
during execution displays copyright notices, you must include the
copyright notice for the Library among them, as well as a reference
directing the user to the copy of this License.  Also, you must do one
of these things:

    a) Accompany the work with the complete corresponding
    machine-readable source code for the Library including whatever
    changes were used in the work (which must be distributed under
    Sections 1 and 2 above); and, if the work is an executable linked
    with the Library, with the complete machine-readable "work that
    uses the Library", as object code and/or source code, so that the
    user can modify the Library and then relink to produce a modified
    executable containing the modified Library.  (It is understood
    that the user who changes the contents of definitions files in the
    Library will not necessarily be able to recompile the application
    to use the modified definitions.)

    b) Use a suitable shared library mechanism for linking with the
    Library.  A suitable mechanism is one that (1) uses at run time a
    copy of the library already present on the user's computer system,
    rather than copying library functions into the executable, and (2)
    will operate properly with a modified version of the library, if
    the user installs one, as long as the modified version is
    interface-compatible with the version that the work was made with.

    c) Accompany the work with a written offer, valid for at
    least three years, to give the same user the materials
    specified in Subsection 6a, above, for a charge no more
    than the cost of performing this distribution.

    d) If distribution of the work is made by offering access to copy
    from a designated place, offer equivalent access to copy the above
    specified materials from the same place.

    e) Verify that the user has already received a copy of these
    materials or that you have already sent this user a copy.

  For an executable, the required form of the "work that uses the
Library" must include any data and utility programs needed for
reproducing the executable from it.  However, as a special exception,
the materials to be distributed need not include anything that is
normally distributed (in either source or binary form) with the major
components (compiler, kernel, and so on) of the operating system on
which the executable runs, unless that component itself accompanies
the executable.

  It may happen that this requirement contradicts the license
restrictions of other proprietary libraries that do not normally
accompany the operating system.  Such a contradiction means you cannot
use both them and the Library together in an executable that you
distribute.

  7. You may place library facilities that are a work based on the
Library side-by-side in a single library together with other library
facilities not covered by this License, and distribute such a combined
library, provided that the separate distribution of the work based on
the Library and of the other library facilities is otherwise
permitted, and provided that you do these two things:

    a) Accompany the combined library with a copy of the same work
    based on the Library, uncombined with any other library
    facilities.  This must be distributed under the terms of the
    Sections above.

    b) Give prominent notice with the combined library of the fact
    that part of it is a work based on the Library, and explaining
    where to find the accompanying uncombined form of the same work.

  8. You may not copy, modify, sublicense, link with, or distribute
the Library except as expressly provided under this License.  Any
attempt otherwise to copy, modify, sublicense, link with, or
distribute the Library is void, and will automatically terminate your
rights under this License.  However, parties who have received copies,
or rights, from you under this License will not have their licenses
terminated so long as such parties remain in full compliance.

  9. You are not required to accept this License, since you have not
signed it.  However, nothing else grants you permission to modify or
distribute the Library or its derivative works.  These actions are
prohibited by law if you do not accept this License.  Therefore, by
modifying or distributing the Library (or any work based on the
Library), you indicate your acceptance of this License to do so, and
all its terms and conditions for copying, distributing or modifying
the Library or works based on it.

  10. Each time you redistribute the Library (or any work based on the
Library), the recipient automatically receives a license from the
original licensor to copy, distribute, link with or modify the Library
subject to these terms and conditions.  You may not impose any further
restrictions on the recipients' exercise of the rights granted herein.
You are not responsible for enforcing compliance by third parties with
this License.

  11. If, as a consequence of a court judgment or allegation of patent
infringement or for any other reason (not limited to patent issues),
conditions are imposed on you (whether by court order, agreement or
otherwise) that contradict the conditions of this License, they do not
excuse you from the conditions of this License.  If you cannot
distribute so as to satisfy simultaneously your obligations under this
License and any other pertinent obligations, then as a consequence you
may not distribute the Library at all.  For example, if a patent
license would not permit royalty-free redistribution of the Library by
all those who receive copies directly or indirectly through you, then
the only way you could satisfy both it and this License would be to
refrain entirely from distribution of the Library.

If any portion of this section is held invalid or unenforceable under any
particular circumstance, the balance of the section is intended to apply,
and the section as a whole is intended to apply in other circumstances.

It is not the purpose of this section to induce you to infringe any
patents or other property right claims or to contest validity of any
such claims; this section has the sole purpose of protecting the
integrity of the free software distribution system which is
implemented by public license practices.  Many people have made
generous contributions to the wide range of software distributed
through that system in reliance on consistent application of that
system; it is up to the author/donor to decide if he or she is willing
to distribute software through any other system and a licensee cannot
impose that choice.

This section is intended to make thoroughly clear what is believed to
be a consequence of the rest of this License.

  12. If the distribution and/or use of the Library is restricted in
certain countries either by patents or by copyrighted interfaces, the
original copyright holder who places the Library under this License may add
an explicit geographical distribution limitation excluding those countries,
so that distribution is permitted only in or among countries not thus
excluded.  In such case, this License incorporates the limitation as if
written in the body of this License.

  13. The Free Software Foundation may publish revised and/or new
versions of the Lesser General Public License from time to time.
Such new versions will be similar in spirit to the present version,
but may differ in detail to address new problems or concerns.

Each version is given a distinguishing version number.  If the Library
specifies a version number of this License which applies to it and
"any later version", you have the option of following the terms and
conditions either of that version or of any later version published by
the Free Software Foundation.  If the Library does not specify a
license version number, you may choose any version ever published by
the Free Software Foundation.

  14. If you wish to incorporate parts of the Library into other free
programs whose distribution conditions are incompatible with these,
write to the author to ask for permission.  For software which is
copyrighted by the Free Software Foundation, write to the Free
Software Foundation; we sometimes make exceptions for this.  Our
decision will be guided by the two goals of preserving the free status
of all derivatives of our free software and of promoting the sharing
and reuse of software generally.

                            NO WARRANTY

  15. BECAUSE THE LIBRARY IS LICENSED FREE OF CHARGE, THERE IS NO
WARRANTY FOR THE LIBRARY, TO THE EXTENT PERMITTED BY APPLICABLE LAW.
EXCEPT WHEN OTHERWISE STATED IN WRITING THE COPYRIGHT HOLDERS AND/OR
OTHER PARTIES PROVIDE THE LIBRARY "AS IS" WITHOUT WARRANTY OF ANY
KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE.  THE ENTIRE RISK AS TO THE QUALITY AND PERFORMANCE OF THE
LIBRARY IS WITH YOU.  SHOULD THE LIBRARY PROVE DEFECTIVE, YOU ASSUME
THE COST OF ALL NECESSARY SERVICING, REPAIR OR CORRECTION.

  16. IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN
WRITING WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MAY MODIFY
AND/OR REDISTRIBUTE THE LIBRARY AS PERMITTED ABOVE, BE LIABLE TO YOU
FOR DAMAGES, INCLUDING ANY GENERAL, SPECIAL, INCIDENTAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF THE USE OR INABILITY TO USE THE
LIBRARY (INCLUDING BUT NOT LIMITED TO LOSS OF DATA OR DATA BEING
RENDERED INACCURATE OR LOSSES SUSTAINED BY YOU OR THIRD PARTIES OR A
FAILURE OF THE LIBRARY TO OPERATE WITH ANY OTHER SOFTWARE), EVEN IF
SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE POSSIBILITY OF SUCH
DAMAGES.

                     END OF TERMS AND CONDITIONS

           How to Apply These Terms to Your New Libraries

  If you develop a new library, and you want it to be of the greatest
possible use to the public, we recommend making it free software that
everyone can redistribute and change.  You can do so by permitting
redistribution under these terms (or, alternatively, under the terms of the
ordinary General Public License).

  To apply these terms, attach the following notices to the library.  It is
safest to attach them to the start of each source file to most effectively
convey the exclusion of warranty; and each file should have at least the
"copyright" line and a pointer to where the full notice is found.

    <one line to give the library's name and a brief idea of what it does.>
    Copyright (C) <year>  <name of author>

    This library is free software; you can redistribute it and/or
    modify it under the terms of the GNU Lesser General Public
    License as published by the Free Software Foundation; either
    version 2.1 of the License, or (at your option) any later version.

    This library is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
    Lesser General Public License for more details.

    You should have received a copy of the GNU Lesser General Public
    License along with this library; if not, see <https://www.gnu.org/licenses/>.

Also add information on how to contact you by electronic and paper mail.

You should also get your employer (if you work as a programmer) or your
school, if any, to sign a "copyright disclaimer" for the library, if
necessary.  Here is a sample; alter the names:

  Yoyodyne, Inc., hereby disclaims all copyright interest in the
  library `Frob' (a library for tweaking knobs) written by James Random Hacker.

  <signature of Moe Ghoul>, 1 April 1990
  Moe Ghoul, President of Vice

That's all there is to it!


## XFS comparison references and local fixtures

The XFS capability audit references libyal/libfsxfs commit
`cf474697de8a32f7eba4122e469b2ecd818054c6` (LGPL-3.0-or-later), without copying,
linking or distributing its C implementation or test corpus. The production reader
remains the BSD-2-Clause xfuse fork under `vendor/xfs-fuse`.
The new Rust format code and the synthetic `minimal-v4.bin` fixture are local
project work. Fixture generator, source statement and hash are recorded in
`vendor/xfs-fuse/test_data/README.md`; full comparison is in `docs/formats/XFS.md`.


## FileVault reader and auditor

`vendor/filevault` contains filevault-core and filevault-forensic 0.1.5,
by Albert Hui / SecurityRonin, pinned at 857e04033e09c84324948b2cbfdd9e043b24d62b.
Source: https://github.com/SecurityRonin/filevault-forensic . License: Apache-2.0,
retained in `vendor/filevault/LICENSE`. Local patches are described in
`vendor/filevault/LOVELYCARVE.md`. libfvde was consulted, not copied or linked.

## HFS Unicode data

`vendor/hfsplus/src/unicode/folding.rs` transforms the fixed case tables
from Apple `hfs/core/UCStringCompareData.h`, commit
d1bac2f062e6e9c0dfcce302d9aacb10173d0eea. Copyright (c) 2000-2002, 2005 Apple Inc.
All rights reserved. This file is covered by APSL-2.0, retained as
`vendor/hfsplus/APPLE_LICENSE`; its original copyright/license header
is retained and LovelyCarve's Rust transformation is marked. The remainder of
hfsplus retains its MIT license.

`unicode/decomposition.rs` is generated from Python's Unicode 3.2 database,
with HFS exclusions and algorithmic Hangul handled separately. Unicode data
license is retained as `vendor/hfsplus/UNICODE_LICENSE`. Source data
version and regeneration steps are described in `hfsplus/LOVELYCARVE.md`.
libfshfs was consulted, not copied or linked.

## jffs2-rs

The read-only JFFS2 reader adapts Rust node field layouts and RTIME decoding from
[xmoezzz/jffs2-rs](https://github.com/xmoezzz/jffs2-rs), version 0.2.1, commit
`52e1234ce66b0e099b35afabff3fc3e3078a2c0e`. It does not incorporate upstream
C LZO / Rubin, FFI, build scripts or host extraction code. Local changes and
limits are documented in `vendor/jffs2/LOVELYCARVE.md`.

```text
MIT License

Copyright (c) 2023 xmoe

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## lzma-rs

The legacy SquashFS reader uses the pure Rust `lzma-rs 0.3.0` decoder by
Guillaume Endignoux, published source commit `da82bd1182993a07db035e285a8f8a87d32a30c2`.
The local fork adds an explicit strict completion API to validate known-size
markerless LZMA streams. Original APIs remain available; no C oracle dependency
is included. See `vendor/lzma-rs/LOVELYCARVE.md`.

```text
MIT License

Copyright (c) 2017 - 2018  Guillaume Endignoux

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## zip (ZIP container metadata)

The Rust `zip` crate is used for ZIP evidence and Sercomm firmware metadata.
Source: https://github.com/zip-rs/zip2 — version recorded in Cargo.lock.

```text
The MIT License (MIT)

Copyright (c) 2014 Mathijs van de Nes

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## MINIFS format references and test data

The `minifs` reader is original Rust code under this repository's AGPL-3.0-only license.
It uses the existing Apache-2.0 `lzma-rust2` dependency; no code from the unlicensed
H4lo/MINIFS-Decompression repository or from the MINIFS paper is incorporated.
Format references and provenance: [MINIFS](docs/formats/MINIFS.md).
The original synthetic MINIFS fixture and its Python generator are CC0-1.0.
Official TP-Link firmware retains its vendor copyright and is downloaded only
for local optional tests; it is not included in this repository.

## F2FS and EROFS readers and fixtures

F2FS metadata modules are adapted from KarpelesLab/fstool 0.5.0 (MIT);
see vendor/f2fs-reader/LICENSE-MIT and ORIGIN.md. New frontend code
is AGPL-3.0-only. The syzkaller mkfs.f2fs fixture is Apache-2.0; its license,
source and hashes are retained under that reader's tests/fixtures directory.
EROFS uses lamfold-erofs 0.1.3 (MIT OR Apache-2.0), including its independently
mkfs-generated test corpus, with original LICENSE-MIT, LICENSE-APACHE and NOTICE.
No external formatter, extractor or kernel C implementation is shipped or run
by the evidence read path. Original F2FS synthetic data/generator is CC0-1.0.

## QNX6 reference fixtures (NetherlandsForensicInstitute/qnxmount)

Apache-2.0, revision `11c8a7f9ee9b945d584263743f6ea8524e8776d2`. The QNX-produced test image, matching TAR and original generation script are retained under `vendor/qnx-reader/tests/fixtures/`; `LICENSE-qnxmount` preserves the license. The image is losslessly zlib-compressed and otherwise unchanged. Runtime QNX4 / QNX6 parsing is original LovelyCarve AGPL-3.0-only Rust code. Optional positive-fixture oracle uses the separately obtained MIT qnxprobe project; it is not a runtime dependency or bundled source.

## UDF reader and reference fixtures

`lamfold-udf` 0.1.1 is adapted from Lamco Development LLC / Greg Lamberson,
[lamco-admin/lamfold](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-udf),
revision `fabaad6af0c28b80535307b339b84110f3a3c12e`, MIT OR Apache-2.0.
Original LICENSE-MIT, LICENSE-APACHE and NOTICE are preserved in
`vendor/lamfold-udf/`; local UDF reader changes use the same licenses.
The mkudffs/Linux-generated UDF 1.02 and 2.01 data fixtures are retained as
lossless zlib copies, with provenance and hashes in their manifest. Invented
fixtures and `scripts/udf-fixtures.py` are CC0-1.0. No upstream C filesystem
implementation, native library or writer is included in the runtime path.

## Minix filesystem reader and fixtures

`minix-reader` metadata layouts are adapted from Dani's
[danifunker/rusty-backup](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/minix.rs),
revision `6e044a314f6523045999058f85a882c1eeb64b50`, manifest license AGPL-3.0.
The original AGPL v3 text and author attribution are retained; this read-only
adaptation is distributed as AGPL-3.0-only. No writer/repair implementation is
included. Official util-linux mkfs.minix/fsck.minix test data and the original
generation script come from `fbfd2ef405d048e8fc01b309b051d2b403b09853`,
GPL-2.0-or-later, with license and original hexdumps preserved in
`vendor/minix-reader/tests/fixtures/util-linux/`.
Invented populated fixtures and `scripts/minix-fixtures.py` are CC0-1.0.
No C filesystem implementation or native runtime dependency is added.

## Classic HFS reader and fixtures

`classic-hfs` adapts HFS layouts and MacRoman decoding from KarpelesLab/fstool
0.5.0, revision `b30823f25b6eef5ea9b6de45876f31c2b8f2d70a` (MIT), and the
MacRoman collation table from Dani's danifunker/rusty-backup 0.1.0, revision
`6e044a314f6523045999058f85a882c1eeb64b50` (AGPL-3.0). The latter attributes
the table to hfsutils, Copyright (C) 1996–1998 Robert Leslie (GPL-2.0-or-later).
Original license texts are retained under `vendor/classic-hfs/`.
The combined adaptation is AGPL-3.0-only. No C filesystem code is compiled.
The retained hfsutils-generated image and generation script are from the same
rusty-backup revision; invented fixtures and `scripts/hfs-fixtures.py` are CC0-1.0.
See [ORIGIN.md](vendor/classic-hfs/ORIGIN.md) for precise source paths,
licenses, changes and fixture hashes.

## ReiserFS v3 reader and fixtures

`reiserfs-reader` adapts layouts/read algorithms from Dani's
[danifunker/rusty-backup](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/reiserfs.rs)
0.1.0 at revision `6e044a314f6523045999058f85a882c1eeb64b50` (AGPL-3.0).
The original license is retained in `vendor/reiserfs-reader/LICENSE`.
The retained mkfs.reiserfs/guestfish image and generation script are from that
same pinned source; invented fixtures and `scripts/reiserfs-fixtures.py` are
CC0-1.0. See [ORIGIN.md](vendor/reiserfs-reader/ORIGIN.md) and the
fixture [manifest](vendor/reiserfs-reader/tests/fixtures/manifest.json).
No C filesystem implementation is linked. The MIT `unearth` detector was
examined during research and is not copied or linked.

## Linux JFS reader and fixtures

`jfs-reader` adapts JFS layouts/read algorithms from Dani's
[danifunker/rusty-backup](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/jfs.rs)
0.1.0 (AGPL-3.0), with bounded tree validation adapted from
[CaddyGlow/partmgr](https://github.com/CaddyGlow/partmgr/blob/d074f660e81dfa6ade66cd07400b7421ad9eb9b9/src/fs_jfs.rs)
0.1.2 (MIT). Original license texts are retained in `vendor/jfs-reader/`.
The combined adaptation is AGPL-3.0-only. No C filesystem implementation is linked.
The retained mkfs.jfs/guestfish fixture and its generation script are from the
same rusty-backup revision; invented fixtures and `scripts/jfs-fixtures.py` are
CC0-1.0. See [ORIGIN.md](vendor/jfs-reader/ORIGIN.md) and the fixture
manifest for precise revisions, changes, licenses and SHA-256 values.

## Amiga OFS / FFS and RDB readers

`amiga-ffs` 0.4.0 and `amiga-rdb` 0.4.1 by Simon Dick, Copyright 2026,
are pure Rust crates licensed MIT OR Apache-2.0. Original LICENSE-MIT and
LICENSE-APACHE files are retained in each vendor directory. Sources:
[amiga-ffs-rs](https://github.com/sidick/amiga-ffs-rs),
[amiga-rdb-rs](https://github.com/sidick/amiga-rdb-rs).
Pinned source hashes and maintained patches are recorded in the corresponding
ORIGIN.md files and [adapter provenance](vendor/amiga-reader/ORIGIN.md).

Invented fixture content and `scripts/amiga-fixtures.py` are CC0-1.0.
[amitools](https://github.com/cnvogelg/amitools) 0.8.1 by Christian Vogelgsang
and contributors (GPL-2.0-or-later) is used only as an external test
formatter/reader oracle, not copied into or linked by the product. The fixture
manifest records decoded/compressed hashes; LNFS and link field-edit boundaries
are documented in the adapter provenance. The LovelyCarve adapter is
AGPL-3.0-only, consistent with this repository.

## BeFS / BeOS BFS reader

`befs-reader` adapts the read-side layouts and algorithms from Dani's
[rusty-backup bfs.rs](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/bfs.rs)
(AGPL-3.0, original license retained as `vendor/befs-reader/LICENSE`).
Primary Haiku BFS definitions and timestamp semantics are MIT-licensed;
Axel Dörfler's notice and the Marcus Overhagen attribution are retained in
`LICENSE-HAIKU`. No C filesystem code is compiled or linked.

The independent invented fixtures and their Python encoder are CC0-1.0.
The opt-in native audit downloads official Haiku public artifacts and an
independent publisher package; these external OS/package files retain their
own licenses and are not distributed in this repository. Fixed sources,
archive checksums, exact verification boundaries and maintained changes are
recorded in [ORIGIN.md](vendor/befs-reader/ORIGIN.md).

## GFS2 detector and original Rust reader

`gfs2-reader` adapts the MIT superblock prefix interpretation from
Marcel Roozekrans’s `unearth` revision `bc891261221f5f5134269780d6c206725b21d52e`,
`src/gfs2.rs`. Its full notice is retained in
[vendor/gfs2-reader/LICENSE-UNEARTH](vendor/gfs2-reader/LICENSE-UNEARTH).
New Rust reading code is AGPL-3.0-only. Linux UAPI record definitions are used
as format references; no kernel code or native filesystem library is linked.
The optional native test uses separately installed Linux tooling and invented
CC0 inputs. See [ORIGIN.md](vendor/gfs2-reader/ORIGIN.md).

## OCFS2 reader

`vendor/ocfs2-reader` contains an original AGPL-3.0-only immutable
Rust reader. Superblock prefix positions/layout adapt Marcel Roozekrans's MIT
`unearth` detector at `bc891261221f5f5134269780d6c206725b21d52e`; its notice is
retained as `LICENSE-UNEARTH`. Linux OCFS2 definitions at
`7a78f7ba9f24962c8033e0152bf34a7903ff61ba` serve as format references;
no kernel C source is compiled, linked or shipped. Original invented fixtures
and Python generators are CC0-1.0; native mkfs/kernel/fsck inputs remain in an
external cache. Provenance, hashes and verification limits are recorded in
[ORIGIN.md](vendor/ocfs2-reader/ORIGIN.md).

## NILFS2 reader

`vendor/nilfs2-reader` contains an original immutable AGPL-3.0-only
Rust reader. Superblock prefix positions adapt Marcel Roozekrans's MIT
`unearth` detector at `bc891261221f5f5134269780d6c206725b21d52e`; its notice
remains in `LICENSE-UNEARTH`. Linux definitions and interpretation at
`7a78f7ba9f24962c8033e0152bf34a7903ff61ba` serve as format references; no
kernel C source or C filesystem library is compiled or linked for this reader.
Independent invented fixtures and generators are CC0-1.0; native mkfs/kernel/GC
inputs stay in an external cache. Sources, hashes and verification limits are
recorded in [ORIGIN.md](vendor/nilfs2-reader/ORIGIN.md).

## bcachefs reader and block framework

`vendor/rust-fs-bcachefs` is the MIT-licensed 0.4.2 reader from
antimatter-studios at `53eb12044f3f6dab498c11c2044748f4591cb4e5`.
Its pinned MIT `rust-fs-core` 0.3.14 sibling is retained at
`19449ad4926d09447d19440c6312ba544b50bf4b`. Original licenses, codec attribution
and the reader's clean-room provenance are retained in their vendor folders.
`bcachefs-reader` is an original AGPL-3.0-only evidence adapter; invented
fixtures/generators are CC0-1.0. Production reads link no native filesystem
library and enable neither the upstream C ABI nor writing. Native oracle
images and native tools are not redistributed. Local changes and evidence:
[reader origin](vendor/rust-fs-bcachefs/ORIGIN.md),
[framework origin](vendor/rust-fs-core/ORIGIN.md),
[adapter and fixtures](vendor/bcachefs-reader/ORIGIN.md).

## GNU General Public License, version 3

The complete GPL-3.0 text, which applies to the Zed file type icons described
above (also kept in `crates/desktop/assets/file_icons/LICENSE-GPL`):

```text
GNU GENERAL PUBLIC LICENSE
Version 3, 29 June 2007

Copyright © 2007 Free Software Foundation, Inc. <https://fsf.org/>

Everyone is permitted to copy and distribute verbatim copies of this license document, but changing it is not allowed.

Preamble

The GNU General Public License is a free, copyleft license for software and other kinds of works.

The licenses for most software and other practical works are designed to take away your freedom to share and change the works. By contrast, the GNU General Public License is intended to guarantee your freedom to share and change all versions of a program--to make sure it remains free software for all its users. We, the Free Software Foundation, use the GNU General Public License for most of our software; it applies also to any other work released this way by its authors. You can apply it to your programs, too.

When we speak of free software, we are referring to freedom, not price. Our General Public Licenses are designed to make sure that you have the freedom to distribute copies of free software (and charge for them if you wish), that you receive source code or can get it if you want it, that you can change the software or use pieces of it in new free programs, and that you know you can do these things.

To protect your rights, we need to prevent others from denying you these rights or asking you to surrender the rights. Therefore, you have certain responsibilities if you distribute copies of the software, or if you modify it: responsibilities to respect the freedom of others.

For example, if you distribute copies of such a program, whether gratis or for a fee, you must pass on to the recipients the same freedoms that you received. You must make sure that they, too, receive or can get the source code. And you must show them these terms so they know their rights.

Developers that use the GNU GPL protect your rights with two steps: (1) assert copyright on the software, and (2) offer you this License giving you legal permission to copy, distribute and/or modify it.

For the developers' and authors' protection, the GPL clearly explains that there is no warranty for this free software. For both users' and authors' sake, the GPL requires that modified versions be marked as changed, so that their problems will not be attributed erroneously to authors of previous versions.

Some devices are designed to deny users access to install or run modified versions of the software inside them, although the manufacturer can do so. This is fundamentally incompatible with the aim of protecting users' freedom to change the software. The systematic pattern of such abuse occurs in the area of products for individuals to use, which is precisely where it is most unacceptable. Therefore, we have designed this version of the GPL to prohibit the practice for those products. If such problems arise substantially in other domains, we stand ready to extend this provision to those domains in future versions of the GPL, as needed to protect the freedom of users.

Finally, every program is threatened constantly by software patents. States should not allow patents to restrict development and use of software on general-purpose computers, but in those that do, we wish to avoid the special danger that patents applied to a free program could make it effectively proprietary. To prevent this, the GPL assures that patents cannot be used to render the program non-free.

The precise terms and conditions for copying, distribution and modification follow.

TERMS AND CONDITIONS

0. Definitions.
"This License" refers to version 3 of the GNU General Public License.

"Copyright" also means copyright-like laws that apply to other kinds of works, such as semiconductor masks.

"The Program" refers to any copyrightable work licensed under this License. Each licensee is addressed as "you". "Licensees" and "recipients" may be individuals or organizations.

To "modify" a work means to copy from or adapt all or part of the work in a fashion requiring copyright permission, other than the making of an exact copy. The resulting work is called a "modified version" of the earlier work or a work "based on" the earlier work.

A "covered work" means either the unmodified Program or a work based on the Program.

To "propagate" a work means to do anything with it that, without permission, would make you directly or secondarily liable for infringement under applicable copyright law, except executing it on a computer or modifying a private copy. Propagation includes copying, distribution (with or without modification), making available to the public, and in some countries other activities as well.

To "convey" a work means any kind of propagation that enables other parties to make or receive copies. Mere interaction with a user through a computer network, with no transfer of a copy, is not conveying.

An interactive user interface displays "Appropriate Legal Notices" to the extent that it includes a convenient and prominently visible feature that (1) displays an appropriate copyright notice, and (2) tells the user that there is no warranty for the work (except to the extent that warranties are provided), that licensees may convey the work under this License, and how to view a copy of this License. If the interface presents a list of user commands or options, such as a menu, a prominent item in the list meets this criterion.

1. Source Code.
The "source code" for a work means the preferred form of the work for making modifications to it. "Object code" means any non-source form of a work.
A "Standard Interface" means an interface that either is an official standard defined by a recognized standards body, or, in the case of interfaces specified for a particular programming language, one that is widely used among developers working in that language.

The "System Libraries" of an executable work include anything, other than the work as a whole, that (a) is included in the normal form of packaging a Major Component, but which is not part of that Major Component, and (b) serves only to enable use of the work with that Major Component, or to implement a Standard Interface for which an implementation is available to the public in source code form. A "Major Component", in this context, means a major essential component (kernel, window system, and so on) of the specific operating system (if any) on which the executable work runs, or a compiler used to produce the work, or an object code interpreter used to run it.

The "Corresponding Source" for a work in object code form means all the source code needed to generate, install, and (for an executable work) run the object code and to modify the work, including scripts to control those activities. However, it does not include the work's System Libraries, or general-purpose tools or generally available free programs which are used unmodified in performing those activities but which are not part of the work. For example, Corresponding Source includes interface definition files associated with source files for the work, and the source code for shared libraries and dynamically linked subprograms that the work is specifically designed to require, such as by intimate data communication or control flow between those subprograms and other parts of the work.

The Corresponding Source need not include anything that users can regenerate automatically from other parts of the Corresponding Source.

The Corresponding Source for a work in source code form is that same work.

2. Basic Permissions.
All rights granted under this License are granted for the term of copyright on the Program, and are irrevocable provided the stated conditions are met. This License explicitly affirms your unlimited permission to run the unmodified Program. The output from running a covered work is covered by this License only if the output, given its content, constitutes a covered work. This License acknowledges your rights of fair use or other equivalent, as provided by copyright law.
You may make, run and propagate covered works that you do not convey, without conditions so long as your license otherwise remains in force. You may convey covered works to others for the sole purpose of having them make modifications exclusively for you, or provide you with facilities for running those works, provided that you comply with the terms of this License in conveying all material for which you do not control copyright. Those thus making or running the covered works for you must do so exclusively on your behalf, under your direction and control, on terms that prohibit them from making any copies of your copyrighted material outside their relationship with you.

Conveying under any other circumstances is permitted solely under the conditions stated below. Sublicensing is not allowed; section 10 makes it unnecessary.

3. Protecting Users' Legal Rights From Anti-Circumvention Law.
No covered work shall be deemed part of an effective technological measure under any applicable law fulfilling obligations under article 11 of the WIPO copyright treaty adopted on 20 December 1996, or similar laws prohibiting or restricting circumvention of such measures.
When you convey a covered work, you waive any legal power to forbid circumvention of technological measures to the extent such circumvention is effected by exercising rights under this License with respect to the covered work, and you disclaim any intention to limit operation or modification of the work as a means of enforcing, against the work's users, your or third parties' legal rights to forbid circumvention of technological measures.

4. Conveying Verbatim Copies.
You may convey verbatim copies of the Program's source code as you receive it, in any medium, provided that you conspicuously and appropriately publish on each copy an appropriate copyright notice; keep intact all notices stating that this License and any non-permissive terms added in accord with section 7 apply to the code; keep intact all notices of the absence of any warranty; and give all recipients a copy of this License along with the Program.
You may charge any price or no price for each copy that you convey, and you may offer support or warranty protection for a fee.

5. Conveying Modified Source Versions.
You may convey a work based on the Program, or the modifications to produce it from the Program, in the form of source code under the terms of section 4, provided that you also meet all of these conditions:
a) The work must carry prominent notices stating that you modified it, and giving a relevant date.
b) The work must carry prominent notices stating that it is released under this License and any conditions added under section 7. This requirement modifies the requirement in section 4 to "keep intact all notices".
c) You must license the entire work, as a whole, under this License to anyone who comes into possession of a copy. This License will therefore apply, along with any applicable section 7 additional terms, to the whole of the work, and all its parts, regardless of how they are packaged. This License gives no permission to license the work in any other way, but it does not invalidate such permission if you have separately received it.
d) If the work has interactive user interfaces, each must display Appropriate Legal Notices; however, if the Program has interactive interfaces that do not display Appropriate Legal Notices, your work need not make them do so.
A compilation of a covered work with other separate and independent works, which are not by their nature extensions of the covered work, and which are not combined with it such as to form a larger program, in or on a volume of a storage or distribution medium, is called an "aggregate" if the compilation and its resulting copyright are not used to limit the access or legal rights of the compilation's users beyond what the individual works permit. Inclusion of a covered work in an aggregate does not cause this License to apply to the other parts of the aggregate.

6. Conveying Non-Source Forms.
You may convey a covered work in object code form under the terms of sections 4 and 5, provided that you also convey the machine-readable Corresponding Source under the terms of this License, in one of these ways:
a) Convey the object code in, or embodied in, a physical product (including a physical distribution medium), accompanied by the Corresponding Source fixed on a durable physical medium customarily used for software interchange.
b) Convey the object code in, or embodied in, a physical product (including a physical distribution medium), accompanied by a written offer, valid for at least three years and valid for as long as you offer spare parts or customer support for that product model, to give anyone who possesses the object code either (1) a copy of the Corresponding Source for all the software in the product that is covered by this License, on a durable physical medium customarily used for software interchange, for a price no more than your reasonable cost of physically performing this conveying of source, or (2) access to copy the Corresponding Source from a network server at no charge.
c) Convey individual copies of the object code with a copy of the written offer to provide the Corresponding Source. This alternative is allowed only occasionally and noncommercially, and only if you received the object code with such an offer, in accord with subsection 6b.
d) Convey the object code by offering access from a designated place (gratis or for a charge), and offer equivalent access to the Corresponding Source in the same way through the same place at no further charge. You need not require recipients to copy the Corresponding Source along with the object code. If the place to copy the object code is a network server, the Corresponding Source may be on a different server (operated by you or a third party) that supports equivalent copying facilities, provided you maintain clear directions next to the object code saying where to find the Corresponding Source. Regardless of what server hosts the Corresponding Source, you remain obligated to ensure that it is available for as long as needed to satisfy these requirements.
e) Convey the object code using peer-to-peer transmission, provided you inform other peers where the object code and Corresponding Source of the work are being offered to the general public at no charge under subsection 6d.
A separable portion of the object code, whose source code is excluded from the Corresponding Source as a System Library, need not be included in conveying the object code work.

A "User Product" is either (1) a "consumer product", which means any tangible personal property which is normally used for personal, family, or household purposes, or (2) anything designed or sold for incorporation into a dwelling. In determining whether a product is a consumer product, doubtful cases shall be resolved in favor of coverage. For a particular product received by a particular user, "normally used" refers to a typical or common use of that class of product, regardless of the status of the particular user or of the way in which the particular user actually uses, or expects or is expected to use, the product. A product is a consumer product regardless of whether the product has substantial commercial, industrial or non-consumer uses, unless such uses represent the only significant mode of use of the product.

"Installation Information" for a User Product means any methods, procedures, authorization keys, or other information required to install and execute modified versions of a covered work in that User Product from a modified version of its Corresponding Source. The information must suffice to ensure that the continued functioning of the modified object code is in no case prevented or interfered with solely because modification has been made.

If you convey an object code work under this section in, or with, or specifically for use in, a User Product, and the conveying occurs as part of a transaction in which the right of possession and use of the User Product is transferred to the recipient in perpetuity or for a fixed term (regardless of how the transaction is characterized), the Corresponding Source conveyed under this section must be accompanied by the Installation Information. But this requirement does not apply if neither you nor any third party retains the ability to install modified object code on the User Product (for example, the work has been installed in ROM).

The requirement to provide Installation Information does not include a requirement to continue to provide support service, warranty, or updates for a work that has been modified or installed by the recipient, or for the User Product in which it has been modified or installed. Access to a network may be denied when the modification itself materially and adversely affects the operation of the network or violates the rules and protocols for communication across the network.

Corresponding Source conveyed, and Installation Information provided, in accord with this section must be in a format that is publicly documented (and with an implementation available to the public in source code form), and must require no special password or key for unpacking, reading or copying.

7. Additional Terms.
"Additional permissions" are terms that supplement the terms of this License by making exceptions from one or more of its conditions. Additional permissions that are applicable to the entire Program shall be treated as though they were included in this License, to the extent that they are valid under applicable law. If additional permissions apply only to part of the Program, that part may be used separately under those permissions, but the entire Program remains governed by this License without regard to the additional permissions.
When you convey a copy of a covered work, you may at your option remove any additional permissions from that copy, or from any part of it. (Additional permissions may be written to require their own removal in certain cases when you modify the work.) You may place additional permissions on material, added by you to a covered work, for which you have or can give appropriate copyright permission.

Notwithstanding any other provision of this License, for material you add to a covered work, you may (if authorized by the copyright holders of that material) supplement the terms of this License with terms:

a) Disclaiming warranty or limiting liability differently from the terms of sections 15 and 16 of this License; or
b) Requiring preservation of specified reasonable legal notices or author attributions in that material or in the Appropriate Legal Notices displayed by works containing it; or
c) Prohibiting misrepresentation of the origin of that material, or requiring that modified versions of such material be marked in reasonable ways as different from the original version; or
d) Limiting the use for publicity purposes of names of licensors or authors of the material; or
e) Declining to grant rights under trademark law for use of some trade names, trademarks, or service marks; or
f) Requiring indemnification of licensors and authors of that material by anyone who conveys the material (or modified versions of it) with contractual assumptions of liability to the recipient, for any liability that these contractual assumptions directly impose on those licensors and authors.
All other non-permissive additional terms are considered "further restrictions" within the meaning of section 10. If the Program as you received it, or any part of it, contains a notice stating that it is governed by this License along with a term that is a further restriction, you may remove that term. If a license document contains a further restriction but permits relicensing or conveying under this License, you may add to a covered work material governed by the terms of that license document, provided that the further restriction does not survive such relicensing or conveying.

If you add terms to a covered work in accord with this section, you must place, in the relevant source files, a statement of the additional terms that apply to those files, or a notice indicating where to find the applicable terms.

Additional terms, permissive or non-permissive, may be stated in the form of a separately written license, or stated as exceptions; the above requirements apply either way.

8. Termination.
You may not propagate or modify a covered work except as expressly provided under this License. Any attempt otherwise to propagate or modify it is void, and will automatically terminate your rights under this License (including any patent licenses granted under the third paragraph of section 11).
However, if you cease all violation of this License, then your license from a particular copyright holder is reinstated (a) provisionally, unless and until the copyright holder explicitly and finally terminates your license, and (b) permanently, if the copyright holder fails to notify you of the violation by some reasonable means prior to 60 days after the cessation.

Moreover, your license from a particular copyright holder is reinstated permanently if the copyright holder notifies you of the violation by some reasonable means, this is the first time you have received notice of violation of this License (for any work) from that copyright holder, and you cure the violation prior to 30 days after your receipt of the notice.

Termination of your rights under this section does not terminate the licenses of parties who have received copies or rights from you under this License. If your rights have been terminated and not permanently reinstated, you do not qualify to receive new licenses for the same material under section 10.

9. Acceptance Not Required for Having Copies.
You are not required to accept this License in order to receive or run a copy of the Program. Ancillary propagation of a covered work occurring solely as a consequence of using peer-to-peer transmission to receive a copy likewise does not require acceptance. However, nothing other than this License grants you permission to propagate or modify any covered work. These actions infringe copyright if you do not accept this License. Therefore, by modifying or propagating a covered work, you indicate your acceptance of this License to do so.
10. Automatic Licensing of Downstream Recipients.
Each time you convey a covered work, the recipient automatically receives a license from the original licensors, to run, modify and propagate that work, subject to this License. You are not responsible for enforcing compliance by third parties with this License.
An "entity transaction" is a transaction transferring control of an organization, or substantially all assets of one, or subdividing an organization, or merging organizations. If propagation of a covered work results from an entity transaction, each party to that transaction who receives a copy of the work also receives whatever licenses to the work the party's predecessor in interest had or could give under the previous paragraph, plus a right to possession of the Corresponding Source of the work from the predecessor in interest, if the predecessor has it or can get it with reasonable efforts.

You may not impose any further restrictions on the exercise of the rights granted or affirmed under this License. For example, you may not impose a license fee, royalty, or other charge for exercise of rights granted under this License, and you may not initiate litigation (including a cross-claim or counterclaim in a lawsuit) alleging that any patent claim is infringed by making, using, selling, offering for sale, or importing the Program or any portion of it.

11. Patents.
A "contributor" is a copyright holder who authorizes use under this License of the Program or a work on which the Program is based. The work thus licensed is called the contributor's "contributor version".
A contributor's "essential patent claims" are all patent claims owned or controlled by the contributor, whether already acquired or hereafter acquired, that would be infringed by some manner, permitted by this License, of making, using, or selling its contributor version, but do not include claims that would be infringed only as a consequence of further modification of the contributor version. For purposes of this definition, "control" includes the right to grant patent sublicenses in a manner consistent with the requirements of this License.

Each contributor grants you a non-exclusive, worldwide, royalty-free patent license under the contributor's essential patent claims, to make, use, sell, offer for sale, import and otherwise run, modify and propagate the contents of its contributor version.

In the following three paragraphs, a "patent license" is any express agreement or commitment, however denominated, not to enforce a patent (such as an express permission to practice a patent or covenant not to sue for patent infringement). To "grant" such a patent license to a party means to make such an agreement or commitment not to enforce a patent against the party.

If you convey a covered work, knowingly relying on a patent license, and the Corresponding Source of the work is not available for anyone to copy, free of charge and under the terms of this License, through a publicly available network server or other readily accessible means, then you must either (1) cause the Corresponding Source to be so available, or (2) arrange to deprive yourself of the benefit of the patent license for this particular work, or (3) arrange, in a manner consistent with the requirements of this License, to extend the patent license to downstream recipients. "Knowingly relying" means you have actual knowledge that, but for the patent license, your conveying the covered work in a country, or your recipient's use of the covered work in a country, would infringe one or more identifiable patents in that country that you have reason to believe are valid.

If, pursuant to or in connection with a single transaction or arrangement, you convey, or propagate by procuring conveyance of, a covered work, and grant a patent license to some of the parties receiving the covered work authorizing them to use, propagate, modify or convey a specific copy of the covered work, then the patent license you grant is automatically extended to all recipients of the covered work and works based on it.

A patent license is "discriminatory" if it does not include within the scope of its coverage, prohibits the exercise of, or is conditioned on the non-exercise of one or more of the rights that are specifically granted under this License. You may not convey a covered work if you are a party to an arrangement with a third party that is in the business of distributing software, under which you make payment to the third party based on the extent of your activity of conveying the work, and under which the third party grants, to any of the parties who would receive the covered work from you, a discriminatory patent license (a) in connection with copies of the covered work conveyed by you (or copies made from those copies), or (b) primarily for and in connection with specific products or compilations that contain the covered work, unless you entered into that arrangement, or that patent license was granted, prior to 28 March 2007.

Nothing in this License shall be construed as excluding or limiting any implied license or other defenses to infringement that may otherwise be available to you under applicable patent law.

12. No Surrender of Others' Freedom.
If conditions are imposed on you (whether by court order, agreement or otherwise) that contradict the conditions of this License, they do not excuse you from the conditions of this License. If you cannot convey a covered work so as to satisfy simultaneously your obligations under this License and any other pertinent obligations, then as a consequence you may not convey it at all. For example, if you agree to terms that obligate you to collect a royalty for further conveying from those to whom you convey the Program, the only way you could satisfy both those terms and this License would be to refrain entirely from conveying the Program.
13. Use with the GNU Affero General Public License.
Notwithstanding any other provision of this License, you have permission to link or combine any covered work with a work licensed under version 3 of the GNU Affero General Public License into a single combined work, and to convey the resulting work. The terms of this License will continue to apply to the part which is the covered work, but the special requirements of the GNU Affero General Public License, section 13, concerning interaction through a network will apply to the combination as such.
14. Revised Versions of this License.
The Free Software Foundation may publish revised and/or new versions of the GNU General Public License from time to time. Such new versions will be similar in spirit to the present version, but may differ in detail to address new problems or concerns.
Each version is given a distinguishing version number. If the Program specifies that a certain numbered version of the GNU General Public License "or any later version" applies to it, you have the option of following the terms and conditions either of that numbered version or of any later version published by the Free Software Foundation. If the Program does not specify a version number of the GNU General Public License, you may choose any version ever published by the Free Software Foundation.

If the Program specifies that a proxy can decide which future versions of the GNU General Public License can be used, that proxy's public statement of acceptance of a version permanently authorizes you to choose that version for the Program.

Later license versions may give you additional or different permissions. However, no additional obligations are imposed on any author or copyright holder as a result of your choosing to follow a later version.

15. Disclaimer of Warranty.
THERE IS NO WARRANTY FOR THE PROGRAM, TO THE EXTENT PERMITTED BY APPLICABLE LAW. EXCEPT WHEN OTHERWISE STATED IN WRITING THE COPYRIGHT HOLDERS AND/OR OTHER PARTIES PROVIDE THE PROGRAM "AS IS" WITHOUT WARRANTY OF ANY KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE. THE ENTIRE RISK AS TO THE QUALITY AND PERFORMANCE OF THE PROGRAM IS WITH YOU. SHOULD THE PROGRAM PROVE DEFECTIVE, YOU ASSUME THE COST OF ALL NECESSARY SERVICING, REPAIR OR CORRECTION.
16. Limitation of Liability.
IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN WRITING WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MODIFIES AND/OR CONVEYS THE PROGRAM AS PERMITTED ABOVE, BE LIABLE TO YOU FOR DAMAGES, INCLUDING ANY GENERAL, SPECIAL, INCIDENTAL OR CONSEQUENTIAL DAMAGES ARISING OUT OF THE USE OR INABILITY TO USE THE PROGRAM (INCLUDING BUT NOT LIMITED TO LOSS OF DATA OR DATA BEING RENDERED INACCURATE OR LOSSES SUSTAINED BY YOU OR THIRD PARTIES OR A FAILURE OF THE PROGRAM TO OPERATE WITH ANY OTHER PROGRAMS), EVEN IF SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.
17. Interpretation of Sections 15 and 16.
If the disclaimer of warranty and limitation of liability provided above cannot be given local legal effect according to their terms, reviewing courts shall apply local law that most closely approximates an absolute waiver of all civil liability in connection with the Program, unless a warranty or assumption of liability accompanies a copy of the Program in return for a fee.

END OF TERMS AND CONDITIONS

How to Apply These Terms to Your New Programs

If you develop a new program, and you want it to be of the greatest possible use to the public, the best way to achieve this is to make it free software which everyone can redistribute and change under these terms.

To do so, attach the following notices to the program. It is safest to attach them to the start of each source file to most effectively state the exclusion of warranty; and each file should have at least the "copyright" line and a pointer to where the full notice is found.

<one line to give the program's name and a brief idea of what it does.>
Copyright (C) <year> <name of author>

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.

Also add information on how to contact you by electronic and paper mail.

If the program does terminal interaction, make it output a short notice like this when it starts in an interactive mode:

<program> Copyright (C) <year> <name of author>
This program comes with ABSOLUTELY NO WARRANTY; for details type `show w'.
This is free software, and you are welcome to redistribute it under certain conditions; type `show c' for details.

The hypothetical commands `show w' and `show c' should show the appropriate parts of the General Public License. Of course, your program's commands might be different; for a GUI interface, you would use an "about box".

You should also get your employer (if you work as a programmer) or school, if any, to sign a "copyright disclaimer" for the program, if necessary. For more information on this, and how to apply and follow the GNU GPL, see <https://www.gnu.org/licenses/>.

The GNU General Public License does not permit incorporating your program into proprietary programs. If your program is a subroutine library, you may consider it more useful to permit linking proprietary applications with the library. If this is what you want to do, use the GNU Lesser General Public License instead of this License. But first, please read <https: //www.gnu.org/licenses/why-not-lgpl.html>.
```
