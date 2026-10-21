# Third-Party Notices

## Scope and dependency records

LovelyCarve's own code is licensed under AGPL-3.0-only; third-party code,
fonts, models, libraries and test data retain their original notices.
The application dependency versions are recorded in `bun.lock` and
`src-tauri/Cargo.lock`; the separate plugin workspace has its own manifests
and lockfile. Vendored components retain their license and provenance files.
This document preserves notices for incorporated components and derived code;
it is not an automatically generated inventory of all transitive packages.
Release verification is documented in [docs/RELEASING.md](docs/RELEASING.md).

The 2026-10-12 vendored-source audit, selected upstream revisions and retained
local patches are recorded in [src-tauri/vendor/UPSTREAM.md](src-tauri/vendor/UPSTREAM.md).
The updated Apple readers use APFS 0.4.1, HFS+ 0.4.0, UDIF 0.4.2 and the new
shared `cmpfs` 0.1.0 decoder from Dil4rd/dpp (MIT). Their license texts and
integration notes remain in each crate directory. ext4-view 1.0.0 and backhand
0.25.5 retain their MIT / Apache-2.0 texts; rsqlite-vfs 0.2.0 retains its MIT
license. Original data sources and hashes for the ext4 unit-test fixtures are
recorded in `src-tauri/vendor/ext4-view/test_data/README.md`.

The APFS name lookup uses the unmodified crates.io packages
[`unicode-casefold` 0.2.0](https://crates.io/crates/unicode-casefold/0.2.0)
by Chris Wong and [`unicode-normalization` 0.1.25](https://crates.io/crates/unicode-normalization/0.1.25)
by the Rust Project Developers. Both declare MIT OR Apache-2.0;
versions and dependency sources are pinned in the Rust lockfiles.
The libfsapfs APFS comparison uses its documented structures and public API;
no libfsapfs C sources or binaries are incorporated. The new APFS fixtures
are invented, reproducible project test structures (AGPL-3.0-only), documented
in `src-tauri/vendor/apfs/tests/data/README.md`.

## rust_ubidump

The UBI volume reader adapts the UBI header decoders and CRC code from
[aspnmy/rust_ubidump](https://github.com/aspnmy/rust_ubidump), revision
`19782c3eabc25a1baab7faefe12fe6f4bd362dac`, under MIT. Its original license
credits Copyright (c) 2017 Willem Hengeveld <itsme@xs4all.nl>.
The complete license is retained in `src-tauri/vendor/ubidump/LICENSE`;
library extraction, safety fixes and the rewritten bounded volume mapping are
documented in `src-tauri/vendor/ubidump/LOVELYCARVE.md`. The upstream CLI,
UBIFS filesystem parser and C LZO dependency are not incorporated.

## CLI timezone rules

The CLI uses the unmodified `jiff` crate by Andrew Gallant (Unlicense OR MIT)
and its bundled `jiff-tzdb` IANA time zone database. Versions are pinned in
`src-tauri/Cargo.lock`; crate license texts remain in the distributed source
packages. IANA time zone data is in the public domain. Evidence-provided TZif
files are read from the user's evidence and are not incorporated or shipped.

## fast-md5

MD5 hashing uses the unmodified crates.io package
[`fast-md5` 1.0.0](https://github.com/ogital-net/fast-md5), licensed BSD-2-Clause.
Its x86_64 / aarch64 assembly derives from
[`animetosho/md5-optimisation`](https://github.com/animetosho/md5-optimisation),
released into the public domain by its author. The Rust glue and tests retain
the following notice; the exact package is pinned in `src-tauri/Cargo.lock`.

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

`src-tauri/vendor/phdi` is original LovelyCarve Rust code (AGPL-3.0-only),
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

`src-tauri/vendor/smraw` is original LovelyCarve Rust code (AGPL-3.0-only),
referencing the public RAW I/O and naming capabilities of Joachim Metz and
contributors' [libyal/libsmraw](https://github.com/libyal/libsmraw/commit/adfa6b8bc8617226805926180244c774366aad30).
The upstream library is LGPL-3.0-or-later and its tools are GPL-3.0-or-later.
No upstream C implementation, document text, test data, binaries or bindings
are incorporated. This crate has no third-party runtime dependencies.
Its license, provenance and scope remain in its vendor directory; all 72
public APIs are compared in [SMRAW](docs/formats/SMRAW.md).
The project-generated synthetic tests contain no real evidence.

## QCOW reader capability reference

`src-tauri/vendor/qcow2` retains Dave Vasilevsky's MIT-licensed qcow2-rs
feature-name parsing and utility/error infrastructure. Original LovelyCarve
QCOW1/2/3 header/read/chain/crypto implementations and invented new test fixtures
are AGPL-3.0-only; both LICENSE-MIT and LICENSE-AGPL are retained in the crate.
Its manifest records `MIT AND AGPL-3.0-only`. Exact origin and change boundaries
are recorded in [LOVELYCARVE.md](src-tauri/vendor/qcow2/LOVELYCARVE.md).

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

`src-tauri/vendor/gzipf` is original LovelyCarve Rust code (AGPL-3.0-only),
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

## Flyfish File Viewer

The Vue preview component `@file-viewer/vue3-full` and the asset-copy plugin
`@file-viewer/vite-plugin`, version 3.1.2, are published by Yu Wang / Flyfish:
<https://github.com/flyfish-dev/file-viewer>.
Both installed packages declare Apache-2.0 and include a `LICENSE` file;
the complete Apache-2.0 terms are reproduced in the EWF section below.
The deployment copies the viewer's worker, WASM and renderer assets from the
same-version npm packages. These assets may incorporate additional components;
retain their bundled notices and review their licenses when changing packages
or publishing a distribution.

## Models and runtime resources

WeChat QRCode model notices and license terms are retained in
[`src-tauri/resources/qrcode/MODEL_LICENSE`](src-tauri/resources/qrcode/MODEL_LICENSE).
Runtime libraries under `src-tauri/binaries` and OCR models under
`src-tauri/resources/ocr` must be checked against their actual upstream
distribution before release.

`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll` and
`msvcp140_1.dll` under `src-tauri/binaries` are the unmodified,
Microsoft-signed Visual C++ runtime from the Visual Studio redistributable
folder (`VC\Redist\MSVC\14.50.35710\x64\Microsoft.VC145.CRT`, file version
14.50.35719.0). They are deployed next to the executables because the desktop
app and the CLI link the C++ standard library dynamically, and
`lc-plugin-host.exe`, the bundled `linuxinfo` plugin and OpenSSL's
`libcrypto-4-x64.dll` link the Visual C++ runtime dynamically. They are
redistributed as Distributable Code under the Microsoft Visual Studio license
terms (<https://aka.ms/vs/18/redistribution>); the Universal CRT they use ships
with Windows 10 and later. The resources shipped depend on the build
features; a package's Rust or JavaScript license does not by itself document
the provenance of every separately bundled DLL or model.

The Quicksand font notice and OFL 1.1 terms are reproduced at the end of this
file, so portable packages that include this document retain the font license.

## ReFS Rust reader and comparison tools

The production Rust reader at `src-tauri/vendor/refs-reader` ports parsing
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
`src-tauri/Cargo.lock`. These candidates are not used by the desktop/CLI
ReFS adapter. Public disk images remain untracked under `fixtures/`; their
sources and hashes are recorded in `docs/formats/REFS.md`.

## ios-backup-core / ios-backup-forensic (SecurityRonin)

The vendored iOS backup reader and anomaly auditor in `src-tauri/vendor/ios-backup`
are derived from https://github.com/SecurityRonin/ios-backup-forensic, commit
`8d8cef447ae212e7dd22dc0425156bf4ddfcfa05`. Copyright Albert Hui and contributors.
Licensed under Apache-2.0 (full terms are reproduced in the EWF section below
and retained in `src-tauri/vendor/ios-backup/LICENSE`). Local changes are listed
in `src-tauri/vendor/ios-backup/UPSTREAM.md`; synthetic encrypted fixtures retain
their upstream provenance.

## Digler

The raw file-carving scan loop and format-specific carving logic in
`src-tauri/src/core/carving/carver*.rs` and
`src-tauri/src/core/carving/formats/` are Rust ports derived from Digler:
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

The vendored pure Rust EWF reader in `src-tauri/vendor/ewf` is derived from
https://github.com/SecurityRonin/ewf-forensic, core crate 0.4.10, commit
`c456f993f68385f3438b3ff16ba5ab6fd0850fb4`. Copyright Albert Hui and contributors.
Licensed under Apache-2.0; the complete license follows. Local modifications are
documented in `src-tauri/vendor/ewf/LOVELYCARVE.md`.


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

The read-only VMDK reader in `src-tauri/vendor/vmdk` is derived from
[SecurityRonin/vmdk-forensic](https://github.com/SecurityRonin/vmdk-forensic),
version 0.8.4, commit `8a9831125c6db5cff9a24414481037487a92d410`, by Albert Hui.
Licensed under Apache License 2.0 (full text reproduced in the EWF notice above
and in `src-tauri/vendor/vmdk/LICENSE`). Local changes are documented in
`src-tauri/vendor/vmdk/LOVELYCARVE.md`. Test-fixture origins, including
log2timeline/dfvfs and log2timeline/plaso, are retained in `tests/data/README.md`.

## bitlocker-core (bitlocker-forensic)

The BitLocker reader in `src-tauri/vendor/bitlocker` is derived from
[SecurityRonin/bitlocker-forensic](https://github.com/SecurityRonin/bitlocker-forensic),
version 0.3.5, commit `e6674dcda2413b1a02a915f2255af8fda787a154`, by Albert Hui.
Licensed under Apache License 2.0 (full text reproduced in the EWF notice above
and in `src-tauri/vendor/bitlocker/LICENSE`). Local changes and test-fixture
provenance are documented alongside the crate. The minimized `bdetogo.raw`
projection is derived from the Apache-2.0 dfVFS test corpus by Joachim Metz
and the dfVFS project: <https://github.com/log2timeline/dfvfs>.

The local BitLocker format comparison uses documentation and observable read
semantics from [libyal/libbde](https://github.com/libyal/libbde), by Joachim Metz
and contributors (LGPL-3.0-or-later library; GPL tools). No libbde C implementation
is linked or vendored. The new synthetic cipher projections are LovelyCarve
Apache-2.0 contributions; generation and digests accompany the fixtures.

## VHD / VHDX readers

`src-tauri/vendor/vhd` contains `vhd-core` 0.1.4 from
<https://github.com/SecurityRonin/vhd-forensic>, commit
`2835523fe39c16740133dc8d51932cad46bcc4b6`.
`src-tauri/vendor/vhdx` contains `vhdx-core` 0.4.1 from
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
`src-tauri/vendor/veracrypt` and `src-tauri/vendor/luks`. Only the core readers
are integrated, not the separate forensic analyzers. The committed encrypted
integration fixtures are generated independently using Python cryptography;
their generator and public test credentials are included.

## Volume Shadow Copy reader

`src-tauri/vendor/vsc` contains `vsc-core` 0.2.2 from
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

`src-tauri/vendor/ad1` contains `ad1-core` 0.1.2 from
<https://github.com/SecurityRonin/ad1-forensic>, commit
`afc3963659a13acf10082e7d76194d3ab789583e`, by Albert Hui / Security Ronin,
licensed under Apache-2.0 (full license in the vendor directory). Only the core
reader is integrated, not the `ad1-forensic` auditor; local changes are listed in
`LOVELYCARVE.md`. The on-disk layout follows the reverse-engineered
al3ks1s/AD1-tools reference; no AD1-tools code is included or distributed. The
tests build their AD1 fixtures in memory; no AD1 image is committed.

## SQLite record carver

`src-tauri/vendor/sqlite-core` contains `sqlite-core` 0.12.0 from
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

`src/assets/fonts/Quicksand-500.ttf` and `Quicksand-700.ttf` contain
the following copyright notice in their font metadata:

Copyright 2019 The Quicksand Project Authors (https://github.com/andrew-paglinawan/QuicksandFamily.git), with Reserved Font Name "Quicksand"

The font family is licensed under SIL Open Font License 1.1.
The [upstream license](https://github.com/andrew-paglinawan/QuicksandFamily/blob/master/OFL.txt)
is retained below and in `src/assets/fonts/OFL.txt`, together
with the copyright notice from the bundled files. Font binaries are unchanged.

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
`src-tauri/Cargo.lock`. Only std/xpress/lzx are enabled. The package declares
LGPL-2.1-only AND LGPL-2.1-or-later AND Zlib AND Apache-2.0 AND MIT;
selected LZX/XPRESS sources derive from wimlib, and LZX includes CAB/Delta helpers.
The unmodified source and dependency manifests form part of the corresponding
source available for rebuilding LovelyCarve. Generated fixture provenance is
in `src-tauri/src/core/filesystem/volumes/ntfs/testdata/README.md`.


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
remains the BSD-2-Clause xfuse fork under `src-tauri/vendor/xfs-fuse`.
The new Rust format code and the synthetic `minimal-v4.bin` fixture are local
project work. Fixture generator, source statement and hash are recorded in
`src-tauri/vendor/xfs-fuse/test_data/README.md`; full comparison is in `docs/formats/XFS.md`.


## FileVault reader and auditor

`src-tauri/vendor/filevault` contains filevault-core and filevault-forensic 0.1.5,
by Albert Hui / SecurityRonin, pinned at 857e04033e09c84324948b2cbfdd9e043b24d62b.
Source: https://github.com/SecurityRonin/filevault-forensic . License: Apache-2.0,
retained in `src-tauri/vendor/filevault/LICENSE`. Local patches are described in
`src-tauri/vendor/filevault/LOVELYCARVE.md`. libfvde was consulted, not copied or linked.

## HFS Unicode data

`src-tauri/vendor/hfsplus/src/unicode/folding.rs` transforms the fixed case tables
from Apple `hfs/core/UCStringCompareData.h`, commit
d1bac2f062e6e9c0dfcce302d9aacb10173d0eea. Copyright (c) 2000-2002, 2005 Apple Inc.
All rights reserved. This file is covered by APSL-2.0, retained as
`src-tauri/vendor/hfsplus/APPLE_LICENSE`; its original copyright/license header
is retained and LovelyCarve's Rust transformation is marked. The remainder of
hfsplus retains its MIT license.

`unicode/decomposition.rs` is generated from Python's Unicode 3.2 database,
with HFS exclusions and algorithmic Hangul handled separately. Unicode data
license is retained as `src-tauri/vendor/hfsplus/UNICODE_LICENSE`. Source data
version and regeneration steps are described in `hfsplus/LOVELYCARVE.md`.
libfshfs was consulted, not copied or linked.
