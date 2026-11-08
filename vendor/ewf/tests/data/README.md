
#### ntfs_sample.E01

- **Source:** minted with `ewfacquire 20231119` from `SampleTinyNtfsVolume/partition.dd`
  (Joakim Schicht's `LogFileParser` sample, MIT), which lives TSK-validated in
  `ntfs-forensic/tests/data/SampleTinyNtfsVolume.zip`.
- **Command:** `ewfacquire -u -t ntfs_sample -f encase6 -c deflate:best -S 4GiB partition.dd`
- **Contents:** a bare 7,340,032-byte NTFS volume (OEM id `NTFS`); MD5 over the
  acquired data `e4e9578a9c8bf6c6b375a80c630196a4`.
- **Use case:** `tests/e2e_ntfs_over_ewf.rs` — the Phase-2 end-to-end VFS proof
  (E01 → ImageSource → SourceCursor → NtfsFs → FileSystem). Oracle: TSK `istat`/`icat`
  (`file1.txt` = record 37, 408 bytes, "Just some bogus…").

## Large third-party E01 corpora — consumed via `$EWF_TEST_CORPUS`

The full-disk validation tests (`tests/validate_maxpowers.rs`,
`tests/validate_pcmus.rs`, and the `ewf_reader_opens_real_e01` smoke test in
`core/src/lib.rs`) run against large real-world E01 images that are **gitignored
and downloaded on demand** — they are never committed here.

Point the `EWF_TEST_CORPUS` environment variable at the directory that holds the
`.E01` files; each test resolves `<$EWF_TEST_CORPUS>/<name>.E01`. When the variable
is unset or a file is absent, the test **skips cleanly** (prints a skip note and
returns) — it never fails on a missing corpus. Example:

```sh
EWF_TEST_CORPUS=/path/to/corpus cargo test -p ewf --test validate_maxpowers -- --nocapture
```

The single fleet-wide index of these corpora is
`issen/docs/corpus-catalog.md` (§A1, §A2, §A3); the entries below are the
per-file detail.

#### MaxPowersCDrive.E01

- **Identity:** DEF CON DFIR CTF 2018 — C: drive of user `mpowers`. EWF case
  "MaxPowers-1", examiner "Professor Frink", acquired 2018-05-05 via f-response.
  ~29 GB compressed; acquired media 53,687,091,200 bytes. NTFS partition at LBA
  1,026,048.
- **Source / writeup:** hecfblog Daily Blog 451 (D. Cowen) —
  <https://www.hecfblog.com/2018/08/daily-blog-451-defcon-dfir-ctf-2018.html>
  (Image 3). Additional writeup: or10nlabs.tech.
- **Original download:** via the hecfblog post above (original
  `https://www.dropbox.com/s/jvaqb4rfi3jojbk/Image3.7z` may be expired).
- **Full-media MD5 (acquired data, per libewf + Sleuth Kit):**
  `10c1fbc9c01d969789ada1c67211b89f` — the oracle `tests/validate_maxpowers.rs`
  asserts. (The `.E01` container-file MD5 is a different value:
  `bed3b3ddece20d136a56aa653f0de608`.)
- **Redistribution:** DEF CON public CTF — non-commercial.
- **Consumed by:** `tests/validate_maxpowers.rs` (env-gated on `EWF_TEST_CORPUS`).

#### PC-MUS-001.E01

- **Identity:** Magnet Virtual Summit 2023 CTF — Windows 11 physical drive, by
  Jessica Hyde + Champlain College DFA for Magnet Forensics. EnCase 6, acquired
  2023-01-07. ~49 GB compressed; acquired media 256,060,514,304 bytes. Contains
  `hiberfil.sys` (MFT #54, ~3.37 GB).
- **Source / writeup:** Magnet / getDataForensics —
  <https://getdataforensics.com/capture-the-flag/> (Magnet Virtual Summit 2023 —
  Win11).
- **Original download:** via the getDataForensics CTF page above.
- **Full-media MD5 (acquired data, per libewf + Sleuth Kit):**
  `522df9db8289f4f8132cf47b14d20fb8` — the oracle `tests/validate_pcmus.rs`
  asserts. (The `.E01` container-file MD5 is a different value:
  `8cf0c007391f4a72ddc12a570a115b46`.)
- **Redistribution:** Magnet / Champlain — verify before redistributing.
- **Consumed by:** `tests/validate_pcmus.rs` and the `ewf_reader_opens_real_e01`
  smoke test in `core/src/lib.rs` (both env-gated on `EWF_TEST_CORPUS`).

#### 20200918_0417_DESKTOP-SDN1RPT.E01 (Szechuan Sauce desktop)

- **Identity:** DFIR Madness "Stolen Szechuan Sauce" Case 001 — Windows 10 desktop
  host `DESKTOP-SDN1RPT`, by James Smith (dfirmadness.com). Acquired media
  16,106,127,360 bytes; GPT-partitioned.
- **Source / writeup:** case page
  <https://dfirmadness.com/the-stolen-szechuan-sauce/>.
- **Original download:** `DESKTOP-E01.zip` from
  <https://dfirmadness.com/case001/DESKTOP-E01.zip> (the `.E01` inside the zip).
  Zip MD5 `71c5c3509331f472abcdf81eb6efff07`.
- **Full-media MD5 (acquired data, per libewf + Sleuth Kit):**
  `bcd3aef20406df00585341f0c743a1ce` — the oracle `tests/validate_szechuan.rs`
  asserts.
- **Redistribution:** dfirmadness.com — public DFIR training material.
- **Consumed by:** `tests/validate_szechuan.rs` (env-gated on `EWF_TEST_CORPUS`).
