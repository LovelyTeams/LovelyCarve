//! Differential tests: volumes built by an *independent* implementation,
//! read back through this crate — and, since milestone 2, volumes built
//! by this crate and handed to that implementation.
//!
//! There are two oracles.
//!
//! **`xdftool`** from amitools is GPL-2, so it is **run, never copied**.
//! Nothing in this file transcribes a line of it; it is invoked as a
//! subprocess, it lays down the bytes, and this crate has to agree with
//! what a completely separate codebase decided those bytes mean. That is
//! the only kind of test that can catch a mistake this crate and its own
//! synthetic builder in `volumes.rs` would make together — and the
//! failure this crate exists to not have (every name empty on a `DOS\7`
//! volume) is exactly that kind of mistake: self-consistent, and wrong.
//!
//! **`fstool`** (KarpelesLab, crates.io) is the second, and it is here
//! for a different reason: it is **MIT**, so when it and this crate
//! disagree the disagreement can be *settled by reading its source*
//! rather than inferred from behaviour. It is still only ever run as a
//! subprocess — no line of it is copied here either, and the citations in
//! the tests below are file-and-function references, not transcriptions.
//! Having a readable second implementation paid for itself immediately:
//! see [`fstool_corrupts_a_dircache_volume_it_was_never_able_to_create`],
//! which names two bugs in it and cites the functions they live in.
//!
//! The two oracles overlap deliberately. Where they agree, the claim is
//! about this crate; where they disagree, three implementations are
//! enough to say which one is odd.
//!
//! # Skipping, not failing
//!
//! Neither tool is a build dependency. When one is absent every test
//! using it prints why and returns green: a contributor without amitools
//! or fstool installed must not see red for a tool they were never asked
//! to have. Set `AMIGA_FFS_XDFTOOL` to point at a specific xdftool, or
//! leave it and the suite looks for `xdftool` on `PATH` and then for
//! `python3 -m amitools.tools.xdftool`; likewise `AMIGA_FFS_FSTOOL`, or
//! `fstool` on `PATH`. CI installs both and asserts they are there, so
//! "skipped" is a local convenience and never a way for the suite to go
//! quiet on a push.
//!
//! Fixtures are generated into a temporary directory at test time and
//! deleted afterwards. Nothing is checked in: a committed ADF is a
//! binary blob nobody can review, and the whole point is that the bytes
//! come from the other implementation *now*, not from whatever version
//! produced them once.
//!
//! # What is covered, and what is not
//!
//! All eight variants `DOS\0`–`DOS\7` are generated and read: both
//! filesystems, both fold tables, dircaches and long names. Long names
//! (>30 bytes) are exercised on `DOS\6`/`DOS\7` only, because xdftool
//! correctly refuses them elsewhere — which is itself asserted, since a
//! writer that accepted one would be writing a volume no AmigaDOS could
//! read.
//!
//! Not covered: **comments**. `xdftool`'s `comment` command fails with a
//! `TypeError` before it writes anything (amitools 0.7.x), so there is no
//! way to make it produce a volume with a comment on it. Comments are
//! covered against synthetic volumes in `volumes.rs` instead, in both
//! layouts including the `T_COMMENT` overflow block.
//!
//! Also not covered: block sizes other than 512. `xdftool`'s ADF and HDF
//! images are 512-byte-blocked, `fstool`'s AFFS backend has `BSIZE` as a
//! compile-time constant, and the RDB-partitioned images where other
//! block sizes live are `rdbtool`'s territory and this crate's sibling's
//! problem.
//!
//! `fstool` covers less than `xdftool` and is asserted to: it reads
//! `DOS\0`–`DOS\5` and writes only `DOS\0`–`DOS\3` (its `AffsFormatOpts`
//! has exactly two knobs, `ffs` and `intl`). It has no long-name support
//! at all — `DOS\6`/`DOS\7` open and produce wrong names rather than an
//! error, which is asserted rather than assumed — and it must not be let
//! near a `DOS\4`/`DOS\5` volume, which is asserted too. *Image geometry*
//! is a documented don't-care for it: its default is a 1 MiB image, and
//! the tests that care pass `--size 880KiB` to get the ADF shape the rest
//! of this file uses.
//!
//! # The write side
//!
//! The last two tests run the other way: this crate formats an ADF and
//! xdftool is asked to mount it, list it, account for its free space,
//! write a file into it and read that file back — and then the two
//! implementations' *own* freshly formatted images are compared longword
//! by longword, which is how the block placement decisions in
//! [`amiga_ffs::format`] stay honest. The don't-cares are named in that
//! comparison rather than assumed: the three root DateStamps (the
//! formatting instant, and a caller-supplied parameter here) and the
//! root's longword −4, which xdftool fills in on every variant and this
//! crate fills in only on the LNFS ones that define it.

use std::path::{Path, PathBuf};
use std::process::Command;

use amiga_ffs::populate::Populator;
use amiga_ffs::*;

// ---------------------------------------------------------------------------
// Finding the oracle
// ---------------------------------------------------------------------------

/// How to invoke xdftool, if it can be invoked at all.
fn xdftool() -> Option<Vec<String>> {
    if let Ok(explicit) = std::env::var("AMIGA_FFS_XDFTOOL") {
        return Some(vec![explicit]);
    }
    let candidates = [
        vec!["xdftool".to_string()],
        vec![
            "python3".to_string(),
            "-m".to_string(),
            "amitools.tools.xdftool".to_string(),
        ],
    ];
    candidates.into_iter().find(|argv| {
        Command::new(&argv[0])
            .args(&argv[1..])
            .arg("--help")
            .output()
            .map_or(false, |o| o.status.success())
    })
}

/// Run xdftool, returning its stdout, or `None` with the reason printed.
/// A failed *command* is a hard failure — the tool is present, so it
/// working is now this test's business.
fn run(argv: &[String], args: &[&str]) -> String {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .args(args)
        .output()
        .expect("xdftool was found a moment ago");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "xdftool {args:?} failed:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // xdftool exits 0 on some filesystem errors and reports them on
    // stdout, so the text has to be checked too.
    assert!(
        !stdout.contains("FSError") && !stdout.contains("Traceback"),
        "xdftool {args:?} reported an error:\n{stdout}"
    );
    stdout
}

/// A scratch directory that removes itself. No `tempfile` dependency for
/// a crate whose whole point is having none.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "amiga-ffs-diff-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("scratch dir");
        Self(p)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// A file-backed BlockSource
// ---------------------------------------------------------------------------

/// The whole image in memory. An ADF is 880 KB; streaming it would buy
/// nothing and cost a `Seek` impl.
struct ImageDisk {
    bs: usize,
    data: Vec<u8>,
}

impl ImageDisk {
    fn open(path: &Path) -> Self {
        Self {
            bs: 512,
            data: std::fs::read(path).expect("image file"),
        }
    }

    /// A blank image of the size an ADF is, for this crate to format.
    fn blank(blocks: u64) -> Self {
        Self {
            bs: 512,
            data: vec![0u8; 512 * blocks as usize],
        }
    }

    fn save(&self, path: &Path) {
        std::fs::write(path, &self.data).expect("write image");
    }

    fn block(&self, lba: u64) -> &[u8] {
        &self.data[lba as usize * self.bs..(lba as usize + 1) * self.bs]
    }
}

impl BlockSink for ImageDisk {
    type Error = ImageError;

    fn block_size(&self) -> usize {
        self.bs
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ImageError> {
        let off = lba as usize * self.bs;
        if off + self.bs > self.data.len() {
            return Err(ImageError(format!("lba {lba} past end of image")));
        }
        self.data[off..off + self.bs].copy_from_slice(buf);
        Ok(())
    }

    fn block_count(&self) -> Option<u64> {
        Some(self.data.len() as u64 / self.bs as u64)
    }
}

#[derive(Debug)]
struct ImageError(String);

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImageError {}

impl BlockSource for ImageDisk {
    type Error = ImageError;

    fn block_size(&self) -> usize {
        self.bs
    }

    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ImageError> {
        let off = lba as usize * self.bs;
        if off + self.bs > self.data.len() {
            return Err(ImageError(format!("lba {lba} past end of image")));
        }
        buf.copy_from_slice(&self.data[off..off + self.bs]);
        Ok(())
    }

    fn block_count(&self) -> Option<u64> {
        Some(self.data.len() as u64 / self.bs as u64)
    }
}

// ---------------------------------------------------------------------------
// The fixture tree
// ---------------------------------------------------------------------------

/// A byte pattern with no short period, so a chain read out of order or
/// off by a block cannot round-trip.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + i / 251) % 251) as u8).collect()
}

/// A file the fixture contains: its Amiga path and its bytes.
fn fixture_files(long_names: bool) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![
        (
            "S/Startup-Sequence".to_string(),
            b"Echo \"Hello\"\n".to_vec(),
        ),
        ("Devs/system-configuration".to_string(), pattern(232)),
        // 40000 bytes is 79 FFS data blocks at 512 -- more than the 72 a
        // header's table holds, so this file *must* cross an extension
        // block. On OFS it is 82 blocks, and crosses one too.
        ("big.dat".to_string(), pattern(40_000)),
        ("Devs/Keymaps/gb".to_string(), pattern(1000)),
        ("empty".to_string(), Vec::new()),
    ];
    if long_names {
        // 50 bytes: past the classic 30-byte limit, well inside LNFS's
        // 107. The name that a classic-offset reader returns as empty.
        files.push((
            "a-file-name-of-fifty-characters-for-the-lnfs-tests".to_string(),
            pattern(777),
        ));
    }
    files
}

const FIXTURE_DIRS: [&str; 3] = ["Devs", "Devs/Keymaps", "S"];

/// Build one variant's image with xdftool and hand back its path.
fn build_image(argv: &[String], scratch: &Scratch, variant: Variant, label: &str) -> PathBuf {
    let name = format!("dos{}.adf", variant.dostype() & 0xFF);
    let image = scratch.path(&name);
    let image_s = image.to_str().unwrap().to_string();
    let dostype = format!("DOS{}", variant.dostype() & 0xFF);

    let mut args: Vec<String> = vec!["-f".into(), image_s, "format".into(), label.into(), dostype];
    for dir in FIXTURE_DIRS {
        args.push("+".into());
        args.push("makedir".into());
        args.push(dir.into());
    }
    for (path, bytes) in fixture_files(variant.has_long_names()) {
        let host = scratch.path(&path.replace('/', "_"));
        std::fs::write(&host, &bytes).expect("write host file");
        args.push("+".into());
        args.push("write".into());
        args.push(host.to_str().unwrap().to_string());
        args.push(path);
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run(argv, &refs);
    image
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Print the reason and skip. Returns the tool when it is there.
macro_rules! oracle {
    () => {
        match xdftool() {
            Some(t) => t,
            None => {
                eprintln!(
                    "SKIP: xdftool not found. Install amitools (`pipx install amitools`) or set \
                     AMIGA_FFS_XDFTOOL to run the differential suite."
                );
                return;
            }
        }
    };
}

/// Every entry the crate finds under `dir`, as `(path, kind, size)`,
/// recursively, sorted — the shape a tree comparison needs.
fn walk(vol: &mut Volume<ImageDisk>, dir: u64, prefix: &str) -> Vec<(String, EntryKind, u32)> {
    let mut out = Vec::new();
    for entry in vol.read_dir(dir).expect("read_dir") {
        let name = String::from_utf8(entry.name.clone()).expect("latin-1 name is ascii here");
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.kind.is_directory() {
            out.extend(walk(vol, entry.lba, &path));
        }
        out.push((path, entry.kind, entry.byte_size));
    }
    out.sort();
    out
}

#[test]
fn every_variant_xdftool_can_write_reads_back_identically() {
    let argv = oracle!();
    let scratch = Scratch::new("tree");

    for byte in 0u32..=7 {
        let variant = Variant::from_dostype(0x444F_5300 | byte).unwrap();
        let label = format!("Vol{byte}");
        let image = build_image(&argv, &scratch, variant, &label);

        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        assert_eq!(vol.variant(), variant, "dostype from the boot block");
        assert_eq!(vol.root().name, label.as_bytes(), "{variant:?}");
        assert_eq!(vol.block_size(), 512);

        let root = vol.root_lba();
        let found = walk(&mut vol, root, "");

        let mut want: Vec<(String, EntryKind, u32)> = FIXTURE_DIRS
            .iter()
            .map(|d| (d.to_string(), EntryKind::Directory, 0))
            .collect();
        for (path, bytes) in fixture_files(variant.has_long_names()) {
            want.push((path, EntryKind::File, bytes.len() as u32));
        }
        want.sort();
        assert_eq!(found, want, "{variant:?} tree");

        // Not one empty name, on any variant. The regression this crate
        // exists to not have, asserted against bytes it did not write.
        for (path, _, _) in &found {
            assert!(!path.is_empty());
            assert!(
                !path.ends_with('/'),
                "{variant:?}: empty component in {path}"
            );
        }

        // Every byte of every file, through the chain the other
        // implementation laid down -- including one that crosses an
        // extension block, on FFS and OFS alike.
        for (path, bytes) in fixture_files(variant.has_long_names()) {
            let entry = vol
                .lookup_path(root, path.as_bytes())
                .unwrap()
                .unwrap_or_else(|| panic!("{variant:?}: {path} not found"));
            assert_eq!(
                vol.read_file(entry.lba).unwrap(),
                bytes,
                "{variant:?}: contents of {path}"
            );
        }

        // Lookup under the volume's own fold table finds what xdftool
        // wrote, whatever case it is asked in.
        assert!(vol.lookup_path(root, b"devs/KEYMAPS/gb").unwrap().is_some());
    }
}

#[test]
fn images_from_another_implementation_validate_clean() {
    let argv = oracle!();
    let scratch = Scratch::new("validate");

    for byte in 0u32..=7 {
        let variant = Variant::from_dostype(0x444F_5300 | byte).unwrap();
        let image = build_image(&argv, &scratch, variant, "Checked");
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();

        // The strongest single statement this suite makes: an
        // independent implementation's idea of a correct volume passes
        // every check this crate's validator applies -- checksums, hash
        // slots, parent pointers, own keys, dircache agreement, and the
        // bitmap in both directions.
        assert!(
            report.is_clean(),
            "{variant:?}: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        let s = report.summary;
        assert_eq!(s.directories, 1 + FIXTURE_DIRS.len() as u64, "{variant:?}");
        assert_eq!(
            s.files,
            fixture_files(variant.has_long_names()).len() as u64,
            "{variant:?}"
        );
        assert_eq!(s.reachable, s.allocated, "{variant:?}");
        assert_eq!(s.orphans, 0);
        assert_eq!(s.reachable_but_free, 0);
        // 40000 bytes needs more than one table's worth of pointers on
        // every variant, so an extension block is always there.
        assert!(s.extension_blocks >= 1, "{variant:?}");
        assert_eq!(
            s.dircache_blocks > 0,
            variant.has_dircache(),
            "{variant:?} dircache blocks"
        );
        assert_eq!(s.bitmap_blocks, 1, "one page covers a 1760-block floppy");
    }
}

#[test]
fn the_bitmap_agrees_with_the_writers_own_accounting() {
    let argv = oracle!();
    let scratch = Scratch::new("bitmap");
    let image = build_image(&argv, &scratch, Variant::FfsIntl, "Counted");

    // xdftool's own `info` prints the block counts it believes.
    let info = run(&argv, &[image.to_str().unwrap(), "info"]);
    let used: u64 = info
        .split_whitespace()
        .skip_while(|w| *w != "used:")
        .nth(1)
        .expect("xdftool info prints a used count")
        .parse()
        .expect("a number");

    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let bm = vol.read_bitmap().unwrap();
    assert!(bm.valid());
    // xdftool counts the two boot blocks as used; the bitmap has no bit
    // for them, which is the whole reason `is_allocated` returns `None`
    // there rather than `Some(true)`.
    assert_eq!(bm.allocated_count() + 2, used, "\n{info}");
    assert_eq!(bm.allocated_count() + bm.free_count(), 1758);
    assert!(bm.covers_whole_volume());
    assert!(!bm.covers(0) && !bm.covers(1));

    // And every block the walk reaches is one the bitmap marks used.
    let report = vol.validate();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.summary.allocated + 2, used);
}

#[test]
fn a_dircache_written_by_another_implementation_agrees_with_its_own_chains() {
    let argv = oracle!();
    let scratch = Scratch::new("dircache");

    for variant in [Variant::OfsIntlDircache, Variant::FfsIntlDircache] {
        let image = build_image(&argv, &scratch, variant, "Cached");
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let root = vol.root_lba();

        let cache = vol.read_dircache(root).unwrap();
        assert!(!cache.blocks.is_empty(), "{variant:?} has a dircache");
        let mut cached: Vec<String> = cache
            .records
            .iter()
            .map(|r| String::from_utf8(r.name.clone()).unwrap())
            .collect();
        cached.sort();

        let mut listed: Vec<String> = vol
            .read_dir(root)
            .unwrap()
            .into_iter()
            .map(|e| String::from_utf8(e.name).unwrap())
            .collect();
        listed.sort();
        assert_eq!(cached, listed, "{variant:?}: cache vs chains");

        // Sizes agree too -- and the record layout's word alignment is
        // what makes the second and later records land where they do, so
        // getting this list right at all is the padding rule confirmed
        // against a writer that is not this crate.
        for record in &cache.records {
            let entry = vol.entry_at(record.entry as u64).unwrap();
            assert_eq!(record.size, entry.byte_size, "{:?}", record.name);
            assert_eq!(record.name, entry.name);
        }

        // The subdirectories have their own caches, chained off their own
        // longword -2.
        let devs = vol.lookup(root, b"Devs").unwrap().unwrap();
        assert!(vol.dircache_head(devs.lba).unwrap() != 0);
        assert_eq!(vol.read_dircache(devs.lba).unwrap().records.len(), 2);
    }
}

#[test]
fn xdftool_refuses_a_long_name_on_a_variant_that_cannot_store_it() {
    // Not a test of this crate: a test that the *oracle* enforces the
    // same 30-byte limit, which is what makes its `DOS\6`/`DOS\7` images
    // evidence about long names rather than evidence about nothing.
    let argv = oracle!();
    let scratch = Scratch::new("toolong");
    let image = scratch.path("classic.adf");
    let host = scratch.path("payload");
    std::fs::write(&host, b"x").unwrap();
    let long = "a-file-name-of-fifty-characters-for-the-lnfs-tests";
    assert!(long.len() > MAX_NAME_CLASSIC);

    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .args([
            "-f",
            image.to_str().unwrap(),
            "format",
            "Classic",
            "DOS3",
            "+",
            "write",
            host.to_str().unwrap(),
            long,
        ])
        .output()
        .expect("run xdftool");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Invalid File Name"),
        "xdftool accepted a {}-byte name on DOS\\3:\n{text}",
        long.len()
    );

    // This crate refuses to even ask for it, one layer earlier.
    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let root = vol.root_lba();
    assert!(matches!(
        vol.lookup(root, long.as_bytes()),
        Err(Error::NameTooLong { max: 30, .. })
    ));
}

// ---------------------------------------------------------------------------
// The other direction: volumes this crate formatted, read by the oracle
// ---------------------------------------------------------------------------

/// The DD floppy geometry every ADF has: 80 cylinders x 2 heads x 11
/// sectors. The only size xdftool will open as an ADF, which is why the
/// write-side differential runs at this size and not another.
const ADF_BLOCKS: u64 = 1760;

/// Format an ADF-sized image with *this* crate and write it out.
fn format_adf(scratch: &Scratch, variant: Variant, label: &str, name: &str) -> PathBuf {
    let mut disk = ImageDisk::blank(ADF_BLOCKS);
    let opts = FormatOptions::new(variant, ADF_BLOCKS, label.as_bytes());
    amiga_ffs::format(&mut disk, &opts).expect("format");
    let path = scratch.path(name);
    disk.save(&path);
    path
}

#[test]
fn xdftool_accepts_a_volume_this_crate_formatted() {
    let argv = oracle!();
    let scratch = Scratch::new("wrote");

    // The claim this test makes is the one that cannot be made by reading
    // our own bytes back: an implementation that shares no code with this
    // one mounts the volume, agrees about its name and its free space,
    // writes a file into it, and reads that file back.
    for (variant, tag) in [(Variant::Ffs, "dos1"), (Variant::FfsIntl, "dos3")] {
        let image = format_adf(&scratch, variant, "Rustfmt", &format!("{tag}.adf"));
        let path = image.to_str().unwrap();

        let listing = run(&argv, &[path, "list"]);
        assert!(
            listing.contains("Rustfmt") && listing.contains("VOLUME"),
            "{variant:?}: xdftool would not list our volume:\n{listing}"
        );
        // An empty volume, so nothing but the volume line itself.
        assert_eq!(
            listing.lines().filter(|l| l.contains("rwed")).count(),
            0,
            "{variant:?}: xdftool sees entries in an empty root:\n{listing}"
        );

        // Its accounting agrees with ours: root plus one bitmap page,
        // plus the two boot blocks xdftool counts and the bitmap has no
        // bits for.
        let info = run(&argv, &[path, "info"]);
        let used: u64 = info
            .split_whitespace()
            .skip_while(|w| *w != "used:")
            .nth(1)
            .expect("a used count")
            .parse()
            .expect("a number");
        assert_eq!(used, 4, "{variant:?}:\n{info}");

        // And it is a *working* filesystem, not merely a parseable one:
        // the oracle allocates from our bitmap, chains into our root's
        // hash table, and gets its bytes back.
        let host = scratch.path("payload");
        let payload = pattern(3000);
        std::fs::write(&host, &payload).unwrap();
        run(
            &argv,
            &[path, "write", host.to_str().unwrap(), "Startup-Sequence"],
        );
        let back = scratch.path("back");
        run(
            &argv,
            &[path, "read", "Startup-Sequence", back.to_str().unwrap()],
        );
        assert_eq!(std::fs::read(&back).unwrap(), payload, "{variant:?}");

        // And this crate still validates what the oracle left behind.
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after xdftool wrote to it: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn our_format_and_the_oracles_agree_block_for_block() {
    let argv = oracle!();
    let scratch = Scratch::new("shape");

    for (variant, tag) in [(Variant::Ffs, "DOS1"), (Variant::FfsIntl, "DOS3")] {
        let ours = ImageDisk::open(&format_adf(
            &scratch,
            variant,
            "Empty",
            &format!("ours-{tag}.adf"),
        ));
        let theirs = scratch.path(&format!("theirs-{tag}.adf"));
        run(
            &argv,
            &["-f", theirs.to_str().unwrap(), "format", "Empty", tag],
        );
        let theirs = ImageDisk::open(&theirs);

        // The boot block: dostype, no checksum, and the root's LBA in
        // longword 2. Identical, including the deliberately-zero
        // checksum -- `Format` leaves that to `Install`, and so do both
        // of these.
        assert_eq!(ours.block(0), theirs.block(0), "{variant:?} boot block");
        assert_eq!(ours.block(1), theirs.block(1), "{variant:?} block 1");

        // The root: every longword but the three DateStamps (which are
        // the formatting instant, and ours is the caller's to supply)
        // and longword -4, where xdftool writes the dostype on every
        // variant and this crate writes it only on LNFS ones, the only
        // place the format defines that field.
        let dont_care: Vec<usize> = (105..=107)
            .chain(117..=123)
            .chain(core::iter::once(124))
            .collect();
        let (a, b) = (ours.block(880), theirs.block(880));
        for lw in 0..128 {
            if dont_care.contains(&lw) || lw == 5 {
                continue;
            }
            assert_eq!(
                be32(a, lw * 4),
                be32(b, lw * 4),
                "{variant:?}: root longword {lw} (tail -{})",
                128 - lw
            );
        }
        // Both checksums are correct over their own block, which is the
        // statement worth making about longword 5.
        assert!(checksum_ok(a) && checksum_ok(b));

        // The bitmap: same page, same placement, and byte for byte the
        // same contents -- checksum included, which only comes out if
        // the allocated set, the bit polarity, the bit order and the
        // checksum longword all agree.
        assert_eq!(
            be32(a, 512 - 49 * 4),
            881,
            "{variant:?}: the bitmap page goes straight after the root"
        );
        assert_eq!(ours.block(881), theirs.block(881), "{variant:?} bitmap");

        // Nothing else in either image was touched.
        for lba in 2..ADF_BLOCKS {
            if lba == 880 || lba == 881 {
                continue;
            }
            assert!(
                ours.block(lba).iter().all(|&x| x == 0),
                "{variant:?}: we wrote block {lba}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The other direction again: a tree this crate *populated*, read by the oracle
// ---------------------------------------------------------------------------

/// Build the fixture tree with this crate's own writer and save it.
///
/// The same tree `build_image` asks xdftool for, so the two directions
/// are comparable: same directories, same files, same 40 000-byte file
/// that must cross an extension block on every variant.
fn populate_adf(scratch: &Scratch, variant: Variant, label: &str, name: &str) -> PathBuf {
    let disk = ImageDisk::blank(ADF_BLOCKS);
    let opts = FormatOptions::new(variant, ADF_BLOCKS, label.as_bytes());
    let mut pop = Populator::new(disk, &opts).expect("populate");
    let root = pop.root_lba();

    let meta = Metadata::new();
    let mut dirs: Vec<(String, u64)> = vec![(String::new(), root)];
    for dir in FIXTURE_DIRS {
        let (parent, leaf) = match dir.rsplit_once('/') {
            Some((p, l)) => (p.to_string(), l),
            None => (String::new(), dir),
        };
        let parent_lba = dirs.iter().find(|(p, _)| *p == parent).expect("parent").1;
        let lba = pop
            .create_dir(parent_lba, leaf.as_bytes(), &meta)
            .expect("create_dir");
        dirs.push((dir.to_string(), lba));
    }
    for (path, bytes) in fixture_files(variant.has_long_names()) {
        let (parent, leaf) = match path.rsplit_once('/') {
            Some((p, l)) => (p.to_string(), l),
            None => (String::new(), path.as_str()),
        };
        let parent_lba = dirs.iter().find(|(p, _)| *p == parent).expect("parent").1;
        pop.create_file(parent_lba, leaf.as_bytes(), &meta, &bytes)
            .expect("create_file");
    }

    let disk = pop.finish().expect("finish");
    let out = scratch.path(name);
    disk.save(&out);
    out
}

#[test]
fn xdftool_reads_back_a_tree_this_crate_populated() {
    let argv = oracle!();
    let scratch = Scratch::new("populated");

    for byte in 0u32..=7 {
        let variant = Variant::from_dostype(0x444F_5300 | byte).unwrap();
        let image = populate_adf(&scratch, variant, "Written", &format!("ours{byte}.adf"));
        let path = image.to_str().unwrap();

        // The listing: an implementation sharing no code with this one
        // walks our hash chains, our extension blocks and (on DOS\4 and
        // DOS\5) our dircaches, and finds every name.
        let listing = run(&argv, &[path, "list"]);
        for dir in FIXTURE_DIRS {
            let leaf = dir.rsplit('/').next().unwrap();
            assert!(
                listing.contains(leaf),
                "{variant:?}: xdftool did not list {dir}:\n{listing}"
            );
        }

        // Every byte of every file, back out through the oracle's reader.
        // This is the claim the round-trip tests structurally cannot make:
        // the bytes are checked by code that did not write them.
        for (file, bytes) in fixture_files(variant.has_long_names()) {
            assert!(
                listing.contains(file.rsplit('/').next().unwrap()),
                "{variant:?}: {file} missing from:\n{listing}"
            );
            let back = scratch.path("back");
            run(&argv, &[path, "read", &file, back.to_str().unwrap()]);
            assert_eq!(
                std::fs::read(&back).unwrap(),
                bytes,
                "{variant:?}: xdftool read {file} differently"
            );
            let _ = std::fs::remove_file(&back);
        }

        // And the oracle can still *write* into it: it allocates from the
        // bitmap `finish()` laid down and chains into a hash table we
        // filled, which is the strongest statement about both.
        let host = scratch.path("payload");
        let payload = pattern(9000);
        std::fs::write(&host, &payload).unwrap();
        run(&argv, &[path, "write", host.to_str().unwrap(), "Added"]);
        let back = scratch.path("added");
        run(&argv, &[path, "read", "Added", back.to_str().unwrap()]);
        assert_eq!(std::fs::read(&back).unwrap(), payload, "{variant:?}");

        // ...and the result still validates through this crate, dircache
        // agreement included.
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after xdftool added a file: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_oracles_free_space_accounting_agrees_with_the_populators() {
    let argv = oracle!();
    let scratch = Scratch::new("accounting");
    let image = populate_adf(&scratch, Variant::FfsIntl, "Counted", "counted.adf");

    let info = run(&argv, &[image.to_str().unwrap(), "info"]);
    let used: u64 = info
        .split_whitespace()
        .skip_while(|w| *w != "used:")
        .nth(1)
        .expect("a used count")
        .parse()
        .expect("a number");

    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let bm = vol.read_bitmap().unwrap();
    assert!(bm.valid(), "finish() restores the bitmap flag");
    // The two boot blocks are used but have no bit, which is why the
    // difference is exactly two and not zero.
    assert_eq!(bm.allocated_count() + 2, used, "\n{info}");
    assert!(vol.validate().is_clean());
}

/// A volume this crate *repaired* is still a volume the other
/// implementation can mount, list, read and write.
///
/// The strongest available statement about `repair()`: the bitmap it
/// rebuilds is not merely one this crate agrees with — an allocator that
/// shares no code with it allocates out of the same bits and the result
/// still reads back here.
#[test]
fn xdftool_accepts_a_volume_this_crate_repaired() {
    let argv = oracle!();
    let scratch = Scratch::new("repaired");
    let image = populate_adf(&scratch, Variant::FfsIntl, "Repaired", "repaired.adf");

    // Damage it in both directions at once: a block a file is using
    // marked free (the dangerous finding) and a free block marked in use
    // (the recoverable one).
    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let root = vol.root_lba();
    let victim = vol.lookup(root, "big.dat".as_bytes()).unwrap().unwrap().lba;
    let bitmap = vol.read_bitmap().unwrap();
    let page = bitmap.pages()[0];
    let stray = bitmap.free().next().unwrap();
    let mut buf = vol.source_mut().block(page).to_vec();
    for (lba, free) in [(victim, true), (stray, false)] {
        let bit = (lba - 2) as usize;
        let off = 4 + (bit / 32) * 4;
        let mut w = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        if free {
            w |= 1 << (bit % 32);
        } else {
            w &= !(1 << (bit % 32));
        }
        buf[off..off + 4].copy_from_slice(&w.to_be_bytes());
    }
    buf[0..4].copy_from_slice(&0u32.to_be_bytes());
    let ck = amiga_ffs::checksum_compute(&buf, amiga_ffs::layout::BITMAP_CHECKSUM_INDEX);
    buf[0..4].copy_from_slice(&ck.to_be_bytes());
    vol.source_mut().write_block(page, &buf).unwrap();

    let mut vol = Volume::open(vol.into_inner(), None).unwrap();
    assert_eq!(vol.validate().summary.reachable_but_free, 1);
    let done = vol.repair(&RepairOptions::new()).expect("repair");
    assert_eq!(done.allocated, 1);
    assert_eq!(done.leaked, 1, "the stale bit stays a leak, by design");
    assert!(vol.validate().summary.reachable_but_free == 0);
    vol.into_inner().save(&image);

    // ...and now the oracle's turn.
    let path = image.to_str().unwrap();
    let listing = run(&argv, &[path, "list"]);
    for (file, bytes) in fixture_files(false) {
        assert!(
            listing.contains(file.rsplit('/').next().unwrap()),
            "{file} missing after repair:\n{listing}"
        );
        let back = scratch.path("back");
        run(&argv, &[path, "read", &file, back.to_str().unwrap()]);
        assert_eq!(std::fs::read(&back).unwrap(), bytes, "{file} after repair");
        let _ = std::fs::remove_file(&back);
    }

    // Writing into it allocates from the bitmap the repair laid down.
    let host = scratch.path("payload");
    let payload = pattern(6000);
    std::fs::write(&host, &payload).unwrap();
    run(
        &argv,
        &[path, "write", host.to_str().unwrap(), "AfterRepair"],
    );
    let back = scratch.path("added");
    run(
        &argv,
        &[path, "read", "AfterRepair", back.to_str().unwrap()],
    );
    assert_eq!(std::fs::read(&back).unwrap(), payload);

    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let report = vol.validate();
    // The one deliberate leftover is the leak the repair kept on purpose;
    // xdftool may or may not have allocated it back.
    for finding in &report.findings {
        assert!(
            matches!(finding, Finding::OrphanBlock { lba } if *lba == stray),
            "after repair and an oracle write: {finding}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mutation, differentially
// ---------------------------------------------------------------------------

/// Populate an ADF-sized image with this crate: a small tree for a
/// mutation test to change.
fn mutable_adf(scratch: &Scratch, variant: Variant, name: &str) -> PathBuf {
    let disk = ImageDisk::blank(ADF_BLOCKS);
    let opts = FormatOptions::new(variant, ADF_BLOCKS, b"Mutated");
    let mut pop = Populator::new(disk, &opts).expect("populate");
    let root = pop.root_lba();
    let meta = Metadata::new();
    let devs = pop.create_dir(root, b"Devs", &meta).expect("makedir");
    pop.create_file(devs, b"system-configuration", &meta, &pattern(232))
        .expect("write");
    pop.create_file(root, b"Doomed", &meta, &pattern(4000))
        .expect("write");
    pop.create_file(root, b"Renameable", &meta, &pattern(1500))
        .expect("write");
    let disk = pop.finish().expect("finish");
    let path = scratch.path(name);
    disk.save(&path);
    path
}

/// The oracle reads what a [`Mutator`] left behind.
///
/// This is the claim no amount of reading our own bytes back can make: an
/// implementation sharing no code with this one mounts a volume whose
/// hash chains have had entries spliced into and out of them, lists the
/// tree we think is there, reads every byte of a file we created *and* of
/// one that was already there when we started, and then writes into the
/// same volume — allocating from a bitmap we edited in place.
#[test]
fn xdftool_reads_a_volume_this_crate_mutated() {
    let argv = oracle!();
    let scratch = Scratch::new("mutate");

    for (variant, tag) in [(Variant::Ffs, "dos1"), (Variant::FfsIntl, "dos3")] {
        let image = mutable_adf(&scratch, variant, &format!("{tag}.adf"));
        let created = pattern(9000);

        // Every operation this wave implements, against a volume that
        // already had a tree on it.
        let mut vol = Volume::open(ImageDisk::open(&image), None).expect("open");
        let root = vol.root_lba();
        let mut m = Mutator::open(vol).expect("mutator");
        let devs = m.volume().lookup(root, b"Devs").unwrap().unwrap().lba;
        m.create_dir(devs, b"Keymaps", &Metadata::new())
            .expect("create_dir");
        let keymaps = m.volume().lookup(devs, b"Keymaps").unwrap().unwrap().lba;
        m.create_file(keymaps, b"usa1", &Metadata::new(), &created)
            .expect("create_file");
        m.rename(root, b"Renameable", devs, b"Renamed")
            .expect("rename");
        m.delete(root, b"Doomed").expect("delete");
        let renamed = m.volume().lookup(devs, b"Renamed").unwrap().unwrap().lba;
        m.set_metadata(
            renamed,
            &MetaUpdate::new().protection(meta::FIBF_WRITE | meta::FIBF_DELETE),
        )
        .expect("set_metadata");
        vol = m.into_volume();
        assert!(vol.validate().is_clean(), "{variant:?}: our own validator");
        vol.into_inner().save(&image);
        let path = image.to_str().unwrap();

        // The oracle's listing agrees with the tree we asked for.
        let listing = run(&argv, &[path, "list"]);
        for present in ["Keymaps", "usa1", "Renamed", "system-configuration"] {
            assert!(
                listing.contains(present),
                "{variant:?}: xdftool cannot see {present}:\n{listing}"
            );
        }
        for gone in ["Doomed", "Renameable"] {
            assert!(
                !listing.contains(gone),
                "{variant:?}: xdftool still sees the deleted {gone}:\n{listing}"
            );
        }

        // ...and every byte of the file we created, and of one that was
        // there before we started, comes back through it.
        for (amiga, want) in [
            ("Devs/Keymaps/usa1", created.clone()),
            ("Devs/system-configuration", pattern(232)),
        ] {
            let back = scratch.path("back");
            run(&argv, &[path, "read", amiga, back.to_str().unwrap()]);
            assert_eq!(std::fs::read(&back).unwrap(), want, "{variant:?}: {amiga}");
        }

        // The bitmap we edited in place is one the oracle can allocate
        // from: it writes a file of its own, and the result still
        // validates here.
        let host = scratch.path("theirs");
        std::fs::write(&host, pattern(2000)).unwrap();
        run(&argv, &[path, "write", host.to_str().unwrap(), "Theirs"]);
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after xdftool wrote into our mutated volume: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        let theirs = vol.lookup(root, b"Theirs").unwrap().unwrap().lba;
        assert_eq!(vol.read_file(theirs).unwrap(), pattern(2000));
    }
}

/// Wave 2's whole claim, from the outside: a volume this crate's own
/// compactor has rewritten -- data blocks relocated (tier 1), a
/// directory header moved (tier 2) -- is not merely readable by this
/// crate's own reader again, it is a volume *neither oracle can tell was
/// ever touched*. Both oracles read the same image; where they agree,
/// the claim is about this crate's compactor specifically, not about
/// whichever oracle happened to be installed.
#[test]
fn both_oracles_read_a_volume_this_crate_compacted() {
    let scratch = Scratch::new("compact");
    let image = mutable_adf(&scratch, Variant::FfsIntl, "compact.adf");

    let created = pattern(9000);
    let vol = Volume::open(ImageDisk::open(&image), None).expect("open");
    let root = vol.root_lba();
    let mut m = Mutator::open(vol).expect("mutator");
    let devs = m.volume().lookup(root, b"Devs").unwrap().unwrap().lba;
    m.create_file(devs, b"Big", &Metadata::new(), &created)
        .expect("create_file");

    // Tier 1: scatter "Big"'s data by rewriting it a few times through
    // ordinary `Mutator` calls (which do not place by policy -- wave 1's
    // own deliberate non-goal), then defragment it back to one run.
    for _ in 0..3 {
        m.truncate(devs, b"Big", 0).expect("truncate");
        m.write_file(devs, b"Big", 0, &created).expect("rewrite");
    }
    let big = m.volume().lookup(devs, b"Big").unwrap().unwrap().lba;
    m.defragment_file(big).expect("tier 1");

    // Tier 2: relocate the directory header itself, reparenting its
    // children in the process.
    let report = m.relocate_header(devs).expect("tier 2");

    let mut vol = m.into_volume();
    assert!(vol.validate().is_clean(), "our own validator, post-compact");
    vol.into_inner().save(&image);
    let path = image.to_str().unwrap();

    // xdftool: the tree and every byte, through a reader sharing no
    // code with the compactor.
    if let Some(argv) = xdftool() {
        let listing = run(&argv, &[path, "list"]);
        for present in ["Devs", "Big", "system-configuration"] {
            assert!(
                listing.contains(present),
                "xdftool cannot see {present}:\n{listing}"
            );
        }
        let back = scratch.path("back");
        run(&argv, &[path, "read", "Devs/Big", back.to_str().unwrap()]);
        assert_eq!(std::fs::read(&back).unwrap(), created, "xdftool: Devs/Big");
    } else {
        eprintln!("xdftool not found -- skipping its half of this differential check");
    }

    // fstool: the same claim, from an MIT-licensed, source-readable
    // second implementation.
    if let Some(bin) = fstool() {
        let cat = fs_run(&bin, &["cat", path, "/Devs/Big"]);
        assert_eq!(cat, created, "fstool: /Devs/Big");
        let info = fs_text(&bin, &["info", path]);
        assert!(
            info.contains("fs kind:           affs"),
            "fstool did not recognise the compacted image as AFFS:\n{info}"
        );
    } else {
        eprintln!("fstool not found -- skipping its half of this differential check");
    }

    // And this crate's own reader, opening the file fresh from disk
    // rather than trusting the in-memory `Mutator` session, agrees with
    // both: the directory really did move, its child really was
    // reparented, and the bytes really are what was written.
    let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
    let root = vol.root_lba();
    let devs = vol.lookup(root, b"Devs").unwrap().unwrap();
    assert_eq!(devs.lba, report.new_lba);
    let big = vol.lookup(devs.lba, b"Big").unwrap().unwrap();
    assert_eq!(big.parent as u64, devs.lba);
    assert_eq!(vol.read_file(big.lba).unwrap(), created);
}

/// The same operation sequence, applied through both implementations,
/// must produce the same *tree*.
///
/// Not the same image: the two allocate differently (this crate scans
/// forward from a hint, xdftool always from the bottom of the bitmap),
/// and dates are the caller's to supply here and the wall clock's there.
/// Those are the documented don't-cares. What must agree is everything
/// the filesystem is *for* — which entries exist, in which directories,
/// of which kind, of which length, holding which bytes — and, because
/// both implementations wrote it, how many blocks the volume has left.
#[test]
fn the_same_operation_sequence_through_both_implementations_agrees() {
    let argv = oracle!();
    let scratch = Scratch::new("both");

    for (variant, tag) in [(Variant::Ffs, "DOS1"), (Variant::FfsIntl, "DOS3")] {
        let payload = pattern(5000);
        let host = scratch.path("payload");
        std::fs::write(&host, &payload).unwrap();
        let small = scratch.path("small");
        std::fs::write(&small, b"hello").unwrap();

        // Theirs: format, makedir, two writes, then delete one of them.
        let theirs = scratch.path(&format!("theirs-{tag}.adf"));
        run(
            &argv,
            &[
                "-f",
                theirs.to_str().unwrap(),
                "format",
                "Both",
                tag,
                "+",
                "makedir",
                "Devs",
                "+",
                "write",
                host.to_str().unwrap(),
                "Devs/Payload",
                "+",
                "write",
                small.to_str().unwrap(),
                "Doomed",
                "+",
                "delete",
                "Doomed",
                "+",
                "write",
                small.to_str().unwrap(),
                "Small",
            ],
        );

        // Ours: the identical sequence through format() and a Mutator --
        // deliberately *not* through the Populator, because a populator
        // cannot delete and the point of the exercise is the delete.
        let ours = scratch.path(&format!("ours-{tag}.adf"));
        {
            let mut disk = ImageDisk::blank(ADF_BLOCKS);
            let opts = FormatOptions::new(variant, ADF_BLOCKS, b"Both");
            amiga_ffs::format(&mut disk, &opts).expect("format");
            let vol = Volume::open(disk, None).expect("open");
            let root = vol.root_lba();
            let mut m = Mutator::open(vol).expect("mutator");
            let meta = Metadata::new();
            let devs = m.create_dir(root, b"Devs", &meta).expect("makedir");
            m.create_file(devs, b"Payload", &meta, &payload)
                .expect("write");
            m.create_file(root, b"Doomed", &meta, b"hello")
                .expect("write");
            m.delete(root, b"Doomed").expect("delete");
            m.create_file(root, b"Small", &meta, b"hello")
                .expect("write");
            m.into_volume().into_inner().save(&ours);
        }

        // The trees agree.
        let mut a = Volume::open(ImageDisk::open(&ours), None).unwrap();
        let mut b = Volume::open(ImageDisk::open(&theirs), None).unwrap();
        let (ra, rb) = (a.root_lba(), b.root_lba());
        assert_eq!(
            walk(&mut a, ra, ""),
            walk(&mut b, rb, ""),
            "{variant:?}: the two trees differ"
        );
        for path in ["Devs/Payload", "Small"] {
            let ea = a.lookup_path(ra, path.as_bytes()).unwrap().unwrap();
            let eb = b.lookup_path(rb, path.as_bytes()).unwrap().unwrap();
            assert_eq!(
                a.read_file(ea.lba).unwrap(),
                b.read_file(eb.lba).unwrap(),
                "{variant:?}: {path} differs"
            );
        }

        // ...and so does the accounting, which is the part a delete that
        // leaked would get wrong. The *blocks* differ (allocation order
        // is a don't-care); how many are in use does not.
        assert!(a.validate().is_clean(), "{variant:?}: ours");
        assert!(b.validate().is_clean(), "{variant:?}: theirs");
        assert_eq!(
            a.read_bitmap().unwrap().allocated_count(),
            b.read_bitmap().unwrap().allocated_count(),
            "{variant:?}: the two volumes disagree about how much is in use"
        );

        // And the oracle can still read what we wrote, after the delete.
        let back = scratch.path("back");
        run(
            &argv,
            &[
                ours.to_str().unwrap(),
                "read",
                "Devs/Payload",
                back.to_str().unwrap(),
            ],
        );
        assert_eq!(std::fs::read(&back).unwrap(), payload, "{variant:?}");
    }
}

// ---------------------------------------------------------------------------
// File contents, differentially
// ---------------------------------------------------------------------------

/// The oracle reads back every byte of a file this crate *rewrote in
/// place* -- appended to across an extension-block boundary, overwritten
/// through the middle, truncated down and grown again.
///
/// This is the claim the model tests in `volumes.rs` cannot make. They
/// compare the crate against a `Vec<u8>` through the crate's own reader,
/// so a writer and a reader that agreed on the same wrong extension-chain
/// arithmetic would pass both. xdftool shares no code with either, and it
/// walks the chain the way an AmigaDOS handler does: header table first,
/// then `T_LIST` blocks, and for `DOS\0` the OFS data headers with their
/// sequence numbers and per-block lengths -- every one of which this
/// wave's copy-on-written boundary block has to have got right.
#[test]
fn xdftool_reads_back_a_file_this_crate_rewrote_in_place() {
    let argv = oracle!();
    let scratch = Scratch::new("filewrite");

    for (variant, tag) in [(Variant::Ofs, "dos0"), (Variant::Ffs, "dos1")] {
        let image = mutable_adf(&scratch, variant, &format!("{tag}.adf"));
        let bs = 512usize;
        let p = layout::data_payload_size(bs, variant.is_ffs()) as u64;
        let slots = hash_table_size(bs) as u64;
        let ext = slots * p;

        // The model, built with the same arithmetic the crate's own tests
        // use: `resize` zero-fills, which is what a grow does.
        let mut model: Vec<u8> = pattern(1000);
        let write = |model: &mut Vec<u8>, off: u64, data: &[u8]| {
            let end = off as usize + data.len();
            if model.len() < end {
                model.resize(end, 0);
            }
            model[off as usize..end].copy_from_slice(data);
        };

        let vol = Volume::open(ImageDisk::open(&image), None).expect("open");
        let root = vol.root_lba();
        let mut m = Mutator::open(vol).expect("mutator");
        m.create_file(root, b"Doc", &Metadata::new(), &model)
            .expect("create_file");

        // Across the header table's last slot and two blocks beyond.
        let grown = pattern((ext + 2 * p) as usize);
        let at = model.len() as u64;
        m.append(root, b"Doc", &grown).expect("append");
        write(&mut model, at, &grown);

        // Partial first block, whole middle, partial last.
        let over = pattern(3 * p as usize);
        m.write_file(root, b"Doc", p / 2, &over)
            .expect("write_file");
        write(&mut model, p / 2, &over);

        // Down to the middle of a block -- on OFS the block that is now
        // last has its recorded length corrected -- and back out past the
        // extension boundary, the gap zero-filled.
        m.truncate(root, b"Doc", p + 33).expect("truncate down");
        model.resize((p + 33) as usize, 0);
        m.truncate(root, b"Doc", ext + p).expect("truncate up");
        model.resize((ext + p) as usize, 0);

        // ...and one more write, landing inside the region the grow
        // zeroed, so a reader that lost the zero-fill shows it here.
        let tail = pattern(500);
        m.write_file(root, b"Doc", ext, &tail).expect("write_file");
        write(&mut model, ext, &tail);

        let mut vol = m.into_volume();
        assert!(vol.validate().is_clean(), "{variant:?}: our own validator");
        assert_eq!(
            read_named(&mut vol, root, b"Doc"),
            model,
            "{variant:?}: our own reader"
        );
        vol.into_inner().save(&image);
        let path = image.to_str().unwrap();

        // The oracle's turn: the size in its listing, then every byte.
        let listing = run(&argv, &[path, "list"]);
        assert!(
            listing.contains(&format!("{}", model.len())),
            "{variant:?}: xdftool does not see a {}-byte Doc:\n{listing}",
            model.len()
        );
        let back = scratch.path("back");
        run(&argv, &[path, "read", "Doc", back.to_str().unwrap()]);
        assert_eq!(
            std::fs::read(&back).unwrap(),
            model,
            "{variant:?}: xdftool read different bytes"
        );

        // And the volume is still one it can allocate from, after all
        // that freeing and reallocating.
        let host = scratch.path("theirs");
        std::fs::write(&host, pattern(3000)).unwrap();
        run(&argv, &[path, "write", host.to_str().unwrap(), "Theirs"]);
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after xdftool wrote into it: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(read_named(&mut vol, root, b"Theirs"), pattern(3000));
        assert_eq!(read_named(&mut vol, root, b"Doc"), model);
    }
}

/// Look a name up in a directory and read the file whole -- the pattern
/// every assertion below wants, written once because in one expression it
/// would borrow the volume twice.
fn read_named(vol: &mut Volume<ImageDisk>, dir: u64, name: &[u8]) -> Vec<u8> {
    let lba = vol
        .lookup(dir, name)
        .expect("lookup")
        .unwrap_or_else(|| panic!("{} is not there", String::from_utf8_lossy(name)))
        .lba;
    vol.read_file(lba).expect("read_file")
}

/// The same *content* sequence through both implementations.
///
/// xdftool has no in-place write -- `write` replaces a file whole -- so
/// the sequence has to be expressed in each implementation's own terms:
/// it deletes and rewrites, this crate truncates and writes. What must
/// agree is the volume they end up with: the same tree, the same bytes,
/// and the same number of blocks in use, which is the part a rewrite that
/// leaked its old data blocks would get wrong.
#[test]
fn the_same_file_content_through_both_implementations_agrees() {
    let argv = oracle!();
    let scratch = Scratch::new("bothfiles");

    for (variant, tag) in [(Variant::Ofs, "DOS0"), (Variant::Ffs, "DOS1")] {
        let first = pattern(20_000);
        let second = pattern(45_000);
        let host_a = scratch.path("a");
        let host_b = scratch.path("b");
        std::fs::write(&host_a, &first).unwrap();
        std::fs::write(&host_b, &second).unwrap();

        // Theirs: write 20 000 bytes, then replace them with 45 000 --
        // which crosses the extension-block boundary the first one did
        // not reach.
        let theirs = scratch.path(&format!("theirs-{tag}.adf"));
        run(
            &argv,
            &[
                "-f",
                theirs.to_str().unwrap(),
                "format",
                "Both",
                tag,
                "+",
                "write",
                host_a.to_str().unwrap(),
                "Doc",
                "+",
                "delete",
                "Doc",
                "+",
                "write",
                host_b.to_str().unwrap(),
                "Doc",
            ],
        );

        // Ours: create it at 20 000, then grow it in place to 45 000 --
        // an append and an overwrite rather than a delete and a create,
        // which is the whole point of the comparison.
        let ours = scratch.path(&format!("ours-{tag}.adf"));
        {
            let mut disk = ImageDisk::blank(ADF_BLOCKS);
            let opts = FormatOptions::new(variant, ADF_BLOCKS, b"Both");
            amiga_ffs::format(&mut disk, &opts).expect("format");
            let vol = Volume::open(disk, None).expect("open");
            let root = vol.root_lba();
            let mut m = Mutator::open(vol).expect("mutator");
            m.create_file(root, b"Doc", &Metadata::new(), &first)
                .expect("create");
            m.append(root, b"Doc", &second[first.len()..])
                .expect("append");
            m.write_file(root, b"Doc", 0, &second[..first.len()])
                .expect("overwrite");
            m.into_volume().into_inner().save(&ours);
        }

        let mut a = Volume::open(ImageDisk::open(&ours), None).unwrap();
        let mut b = Volume::open(ImageDisk::open(&theirs), None).unwrap();
        let (ra, rb) = (a.root_lba(), b.root_lba());
        assert_eq!(walk(&mut a, ra, ""), walk(&mut b, rb, ""), "{variant:?}");
        assert_eq!(read_named(&mut a, ra, b"Doc"), second, "{variant:?}: ours");
        assert_eq!(
            read_named(&mut b, rb, b"Doc"),
            second,
            "{variant:?}: theirs"
        );
        assert!(a.validate().is_clean(), "{variant:?}: ours");
        assert!(b.validate().is_clean(), "{variant:?}: theirs");
        assert_eq!(
            a.read_bitmap().unwrap().allocated_count(),
            b.read_bitmap().unwrap().allocated_count(),
            "{variant:?}: the two volumes disagree about how much is in use"
        );

        // And the oracle reads every byte of the file we grew in place.
        let back = scratch.path("back");
        run(
            &argv,
            &[
                ours.to_str().unwrap(),
                "read",
                "Doc",
                back.to_str().unwrap(),
            ],
        );
        assert_eq!(std::fs::read(&back).unwrap(), second, "{variant:?}");
    }
}

// ---------------------------------------------------------------------------
// The third leg: fstool, the *readable* oracle
// ---------------------------------------------------------------------------

/// How to invoke `fstool`, if it can be invoked at all.
///
/// Same shape as [`xdftool`] above: an explicit override first, then
/// `PATH`. `fstool` is a Rust crate rather than a Python package, so
/// there is no module-invocation fallback to try.
fn fstool() -> Option<String> {
    let candidates = match std::env::var("AMIGA_FFS_FSTOOL") {
        Ok(explicit) => vec![explicit],
        Err(_) => vec!["fstool".to_string()],
    };
    candidates.into_iter().find(|bin| {
        Command::new(bin)
            .arg("--version")
            .output()
            .map_or(false, |o| o.status.success())
    })
}

/// Print the reason and skip. Returns the tool when it is there.
macro_rules! fstool_oracle {
    () => {
        match fstool() {
            Some(t) => t,
            None => {
                eprintln!(
                    "SKIP: fstool not found. Install it (`cargo install fstool --locked`) or \
                     set AMIGA_FFS_FSTOOL to run the fstool leg of the differential suite."
                );
                return;
            }
        }
    };
}

/// Run fstool and hand back its raw stdout. `cat` prints file bytes, so
/// this cannot be a `String`. Unlike xdftool, fstool exits non-zero on a
/// filesystem error, so the status is the whole check.
fn fs_run(bin: &str, args: &[&str]) -> Vec<u8> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .expect("fstool was found a moment ago");
    assert!(
        out.status.success(),
        "fstool {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// The same, for the subcommands whose output is text.
fn fs_text(bin: &str, args: &[&str]) -> String {
    String::from_utf8(fs_run(bin, args)).expect("fstool prints UTF-8")
}

/// Drive `fstool shell` over stdin. `mkdir` and `put` live only there —
/// `add` is the non-interactive half and there is no non-interactive
/// `mkdir` — so the in-place mutation leg needs it.
fn fs_shell(bin: &str, image: &str, script: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(bin)
        .args(["shell", image])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fstool shell");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(script.as_bytes())
        .expect("write shell script");
    let out = child.wait_with_output().expect("fstool shell");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "fstool shell failed:\n{text}");
    text
}

/// The variant string `fstool info` prints, spelled the way *it* spells
/// it rather than the way this crate would.
///
/// The difference is deliberate and is the first disagreement this leg
/// records: fstool decodes the dostype byte as three independent bits
/// (`ffs | intl<<1 | dircache<<2`), so it calls `DOS\4` "OFS+DC" where
/// this crate calls it international — AmigaDOS's directory-cache mode
/// implies international case folding, and [`Variant::is_intl`] says so.
/// It also has no long-name bit at all, so `DOS\6`/`DOS\7` come out
/// labelled "+DC". Nothing downstream of the label depends on it for the
/// variants tested here; see the long-name test for where it does.
fn fstool_variant_label(variant: Variant) -> String {
    let byte = variant.dostype() & 0xFF;
    format!(
        "DOS\\{byte} ({}{}{})",
        if byte & 1 != 0 { "FFS" } else { "OFS" },
        // Intl is a property of the *variant*, not of bit 1 alone: every
        // dircache flavour is international too (there is no non-intl
        // dircache variant), which is exactly the bug fstool 0.4.27 fixed
        // (KarpelesLab/fstool#42) — its own `info` now says `+INTL+DC`
        // for `DOS\4`/`DOS\5`, matching `Variant::is_intl` here.
        if variant.is_intl() { "+INTL" } else { "" },
        if byte & 4 != 0 { "+DC" } else { "" },
    )
}

/// Latin-1 bytes as fstool spells them back: one byte, one code point.
///
/// [`walk`] above uses `from_utf8`, which is honest for the xdftool
/// fixture because it is ASCII. The fstool fixture deliberately is not.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// One file in the fstool fixture.
///
/// Two spellings of the name, because there are two: the bytes on the
/// volume (Latin-1) and the UTF-8 string a host filename and an fstool
/// command line are written in.
struct FixtureFile {
    /// Directory it lives in, `""` for the root.
    dir: String,
    /// Its name as stored on the volume.
    name: Vec<u8>,
    /// The same name, as fstool and the host spell it.
    display: String,
    bytes: Vec<u8>,
}

impl FixtureFile {
    /// The whole path, in fstool's spelling, without a leading slash.
    fn path(&self) -> String {
        if self.dir.is_empty() {
            self.display.clone()
        } else {
            format!("{}/{}", self.dir, self.display)
        }
    }

    /// The whole path as it is spelled *on the volume* — Latin-1, so one
    /// byte for `é` where [`FixtureFile::path`] has two. The two are not
    /// interchangeable, which is the entire reason the fixture carries
    /// both: a lookup asked in UTF-8 hashes to a different slot and
    /// misses.
    fn amiga_path(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.dir.is_empty() {
            out.extend_from_slice(self.dir.as_bytes());
            out.push(b'/');
        }
        out.extend_from_slice(&self.name);
        out
    }
}

/// The xdftool fixture tree plus one Latin-1 name.
///
/// `Café` is the name that separates a byte-transparent implementation
/// from one that assumed ASCII: `é` is one byte (`0xE9`) on the volume
/// and two in the UTF-8 the host filesystem and fstool's own output use.
/// Long names are absent on purpose — see
/// [`fstool_reads_a_long_name_as_a_truncated_one`].
fn fstool_fixture() -> Vec<FixtureFile> {
    let mut out: Vec<FixtureFile> = fixture_files(false)
        .into_iter()
        .map(|(path, bytes)| {
            let (dir, leaf) = match path.rsplit_once('/') {
                Some((d, l)) => (d.to_string(), l.to_string()),
                None => (String::new(), path),
            };
            FixtureFile {
                dir,
                name: leaf.clone().into_bytes(),
                display: leaf,
                bytes,
            }
        })
        .collect();
    out.push(FixtureFile {
        dir: String::new(),
        name: b"Caf\xe9".to_vec(),
        display: "Caf\u{e9}".to_string(),
        bytes: pattern(64),
    });
    out
}

/// The whole tree as `(path, kind)`, recursively, read through
/// `fstool ls` — which prints TAB-separated `block<TAB>Kind<TAB>name`.
///
/// The block column is deliberately dropped: fstool allocates from the
/// bottom of the volume and this crate scans forward from a hint, so
/// *which* block an entry landed on is a documented don't-care. What
/// must agree is which entries exist, where, and of what kind.
fn fstool_tree(
    bin: &str,
    image: &str,
    dir: &str,
    prefix: &str,
    out: &mut Vec<(String, EntryKind)>,
) {
    let listing = fs_text(bin, &["ls", image, dir]);
    for line in listing.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let _block = fields.next().expect("block column");
        let kind = match fields.next().expect("kind column") {
            "Dir" => EntryKind::Directory,
            "Regular" => EntryKind::File,
            other => panic!("fstool ls printed kind {other:?} in:\n{listing}"),
        };
        let name = fields.next().expect("name column");
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_directory() {
            let sub = if dir == "/" {
                format!("/{name}")
            } else {
                format!("{dir}/{name}")
            };
            fstool_tree(bin, image, &sub, &path, out);
        }
        out.push((path, kind));
    }
}

/// [`walk`], for a fixture whose names are not all ASCII.
fn walk_latin1(vol: &mut Volume<ImageDisk>, dir: u64, prefix: &str) -> Vec<(String, EntryKind)> {
    let mut out = Vec::new();
    for entry in vol.read_dir(dir).expect("read_dir") {
        let name = latin1(&entry.name);
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.kind.is_directory() {
            out.extend(walk_latin1(vol, entry.lba, &path));
        }
        out.push((path, entry.kind));
    }
    out.sort();
    out
}

/// The fixture tree as `(path, kind)`, sorted — what both directions
/// compare against.
fn fstool_fixture_tree() -> Vec<(String, EntryKind)> {
    let mut want: Vec<(String, EntryKind)> = FIXTURE_DIRS
        .iter()
        .map(|d| (d.to_string(), EntryKind::Directory))
        .collect();
    for file in fstool_fixture() {
        want.push((file.path(), EntryKind::File));
    }
    want.sort();
    want
}

/// Build the fixture tree with this crate's writer, Latin-1 name and all.
fn populate_fstool_adf(scratch: &Scratch, variant: Variant, label: &str, name: &str) -> PathBuf {
    let disk = ImageDisk::blank(ADF_BLOCKS);
    let opts = FormatOptions::new(variant, ADF_BLOCKS, label.as_bytes());
    let mut pop = Populator::new(disk, &opts).expect("populate");
    let root = pop.root_lba();
    let meta = Metadata::new();

    let mut dirs: Vec<(String, u64)> = vec![(String::new(), root)];
    for dir in FIXTURE_DIRS {
        let (parent, leaf) = match dir.rsplit_once('/') {
            Some((p, l)) => (p.to_string(), l),
            None => (String::new(), dir),
        };
        let parent_lba = dirs.iter().find(|(p, _)| *p == parent).expect("parent").1;
        let lba = pop
            .create_dir(parent_lba, leaf.as_bytes(), &meta)
            .expect("create_dir");
        dirs.push((dir.to_string(), lba));
    }
    for file in fstool_fixture() {
        let parent_lba = dirs.iter().find(|(p, _)| *p == file.dir).expect("parent").1;
        pop.create_file(parent_lba, &file.name, &meta, &file.bytes)
            .expect("create_file");
    }

    let disk = pop.finish().expect("finish");
    let out = scratch.path(name);
    disk.save(&out);
    out
}

/// The same tree on the host, for `fstool create` to read.
fn host_fixture_tree(scratch: &Scratch) -> PathBuf {
    let root = scratch.path("hosttree");
    std::fs::create_dir_all(&root).expect("host tree");
    for dir in FIXTURE_DIRS {
        std::fs::create_dir_all(root.join(dir)).expect("host subdir");
    }
    for file in fstool_fixture() {
        std::fs::write(root.join(file.path()), &file.bytes).expect("host file");
    }
    root
}

/// The variants fstool's AFFS backend can be asked to *create*.
///
/// `AffsFormatOpts` has exactly two knobs, `ffs` and `intl`, so the
/// creatable set is `DOS\0`–`DOS\3` and nothing else: there is no
/// `dircache` option (`-O dircache=true` is rejected by name) and no
/// long-name one. `DOS\4`/`DOS\5` are *readable* — see the read leg —
/// but only this crate and xdftool can write them.
const FSTOOL_CREATABLE: [(Variant, &str, &str); 4] = [
    (Variant::Ofs, "ofs", "false"),
    (Variant::Ffs, "ffs", "false"),
    (Variant::OfsIntl, "ofs", "true"),
    (Variant::FfsIntl, "ffs", "true"),
];

// ---------------------------------------------------------------------------
// fstool reads what this crate wrote
// ---------------------------------------------------------------------------

/// Every variant fstool can read, written by this crate and read back
/// through it: the variant string, the whole tree, and every byte.
///
/// `DOS\0`–`DOS\5`, which is more than fstool can *write*. Its reader
/// walks all 72 hash buckets and every same-hash chain rather than
/// hashing a name to find it, so the fold table it would have used never
/// comes into play and a directory-cache volume reads back correctly
/// even though fstool knows nothing about dircache blocks. `DOS\6` and
/// `DOS\7` are the two it cannot read; that is its own test.
#[test]
fn fstool_reads_every_variant_it_can_read_that_this_crate_wrote() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-read");

    for byte in 0u32..=5 {
        let variant = Variant::from_dostype(DOSTYPE_MAGIC | byte).unwrap();
        let label = format!("FsVol{byte}");
        let image = populate_fstool_adf(&scratch, variant, &label, &format!("ours{byte}.adf"));
        let path = image.to_str().unwrap();

        // It knows what it is looking at, down to the variant byte.
        let info = fs_text(&bin, &["info", path]);
        assert!(
            info.contains("fs kind:           affs"),
            "{variant:?}: fstool did not recognise our image as AFFS:\n{info}"
        );
        assert!(
            info.contains(&format!("volume name:       {label:?}")),
            "{variant:?}: fstool read a different volume name:\n{info}"
        );
        assert!(
            info.contains(&format!(
                "variant:           {}",
                fstool_variant_label(variant)
            )),
            "{variant:?}: fstool read a different variant:\n{info}"
        );

        // The whole tree, entry for entry, through a reader that shares
        // no code with the writer -- and no code with xdftool either,
        // which is the point of having a third leg at all.
        let mut found = Vec::new();
        fstool_tree(&bin, path, "/", "", &mut found);
        found.sort();
        assert_eq!(found, fstool_fixture_tree(), "{variant:?}: fstool's tree");
        for (path, _) in &found {
            assert!(!path.is_empty() && !path.ends_with('/'), "{variant:?}");
        }

        // Every byte of every file, including the 40 000-byte one that
        // has to cross an extension block on OFS and FFS alike, the
        // empty one, and the Latin-1 name.
        for file in fstool_fixture() {
            let amiga = format!("/{}", file.path());
            assert_eq!(
                fs_run(&bin, &["cat", path, &amiga]),
                file.bytes,
                "{variant:?}: fstool read {amiga} differently"
            );
        }
    }
}

/// What fstool does with a `DOS\6`/`DOS\7` volume, asserted so the
/// limitation is a fact this suite records rather than one it assumes.
///
/// It is not a refusal. `Affs::open` accepts any boot flag byte 0..=7
/// and decodes bit 2 as "dircache", so a long-name volume opens and is
/// mislabelled `+DC`; `read_name` then reads the BCPL name at the
/// classic offset `0x1b0` and clamps its length to 30. On `DOS\6`/`DOS\7`
/// the name does not live there, so what comes back is not the name.
///
/// This is exactly the failure mode this crate exists to not have, seen
/// from the outside — which makes it the sharpest available statement of
/// why long-name support is not something a reader gets for free. The
/// assertion is deliberately weak (fstool does not see the real name)
/// rather than an exact transcription of the garbage, because the garbage
/// is not a contract.
#[test]
fn fstool_now_refuses_a_long_name_volume_outright() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-long");
    let long = "a-file-name-of-fifty-characters-for-the-lnfs-tests";

    for variant in [Variant::OfsIntlLongname, Variant::FfsIntlLongname] {
        let byte = variant.dostype() & 0xFF;
        let image = populate_adf(&scratch, variant, "LongNames", &format!("long{byte}.adf"));
        let path = image.to_str().unwrap();

        // This crate reads it, which is what makes the next assertion a
        // statement about fstool and not about the image.
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let root = vol.root_lba();
        assert!(vol.lookup(root, long.as_bytes()).unwrap().is_some());
        assert!(vol.validate().is_clean());

        // fstool used to open it and call it a dircache volume, since it
        // had no long-name bit to decode -- the exact trap this crate
        // exists to refuse (raised as the aside in KarpelesLab/fstool#43,
        // "Affs::open accepts boot flags 6 and 7... refusing would be
        // more honest than reading them at the wrong offsets"). Fixed in
        // 0.4.27: it now refuses to open the volume at all.
        let out = Command::new(&bin)
            .args(["info", path])
            .output()
            .expect("fstool was found a moment ago");
        assert!(
            !out.status.success(),
            "fstool opened a long-name volume; the #43 aside regressed"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("long-filename") || stderr.contains("not supported"),
            "fstool refused for an unexpected reason:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// This crate reads what fstool wrote
// ---------------------------------------------------------------------------

/// A volume `fstool create -t affs` produced, opened here.
///
/// The other direction of the read leg, and the one that catches a
/// *reader* assumption rather than a writer one: nothing in this image
/// came from this crate, its allocator ran bottom-up where ours runs
/// from a hint, and its root block leaves the header key and the boot
/// block's root pointer zero.
#[test]
fn this_crate_reads_a_volume_fstool_created() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-made");
    let src = host_fixture_tree(&scratch);
    let src = src.to_str().unwrap();

    for (variant, fstype, intl) in FSTOOL_CREATABLE {
        let byte = variant.dostype() & 0xFF;
        let image = scratch.path(&format!("theirs{byte}.adf"));
        fs_run(
            &bin,
            &[
                "create",
                "-t",
                "affs",
                src,
                "-o",
                image.to_str().unwrap(),
                // The ADF geometry the rest of this suite uses. fstool's
                // default is a 1 MiB image, which is fine too -- see
                // below -- but pinning it here keeps the root block at
                // 880 where every other test in this file expects it.
                "--size",
                "880KiB",
                "-O",
                &format!("fstype={fstype},intl={intl},volume_label=FsMade"),
            ],
        );

        let mut vol = Volume::open(ImageDisk::open(&image), None).expect("open fstool's image");
        assert_eq!(vol.variant(), variant, "the dostype fstool wrote");
        assert_eq!(vol.root().name, b"FsMade");
        assert_eq!(vol.block_size(), 512);
        assert_eq!(vol.root_lba(), 880, "fstool puts the root at total/2");

        let root = vol.root_lba();
        assert_eq!(
            walk_latin1(&mut vol, root, ""),
            fstool_fixture_tree(),
            "{variant:?}: the tree fstool wrote"
        );
        for file in fstool_fixture() {
            let entry = vol
                .lookup_path(root, &file.amiga_path())
                .unwrap()
                .unwrap_or_else(|| panic!("{variant:?}: {} not found", file.path()));
            assert_eq!(entry.byte_size as usize, file.bytes.len(), "{variant:?}");
            assert_eq!(
                vol.read_file(entry.lba).unwrap(),
                file.bytes,
                "{variant:?}: contents of {}",
                file.path()
            );
        }
        // The Latin-1 name is found by its bytes, under this volume's
        // own fold table -- the intl one on DOS\2/DOS\3, where `é` folds
        // to `É`, and the classic one on DOS\0/DOS\1, where it does not.
        assert!(vol.lookup(root, b"Caf\xe9").unwrap().is_some());

        // Zero findings: checksums, hash slots, parent pointers, own
        // keys and the bitmap in both directions, on bytes this crate
        // never touched.
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?}: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(report.summary.directories, 1 + FIXTURE_DIRS.len() as u64);
        assert_eq!(report.summary.files, fstool_fixture().len() as u64);
        assert_eq!(report.summary.orphans, 0);
        assert_eq!(report.summary.reachable_but_free, 0);
        assert!(report.summary.extension_blocks >= 1);
        assert_eq!(report.summary.dircache_blocks, 0, "fstool writes no cache");
    }

    // Geometry is a documented don't-care. fstool's own default is a
    // 1 MiB image -- 2048 blocks, root at 1024 -- and that opens and
    // validates here too, which is the statement that the root-block
    // formula agrees at more than one size.
    let image = scratch.path("default.adf");
    fs_run(
        &bin,
        &["create", "-t", "affs", src, "-o", image.to_str().unwrap()],
    );
    let mut vol = Volume::open(ImageDisk::open(&image), None).expect("open");
    assert_eq!(vol.variant(), Variant::FfsIntl, "fstool's default variant");
    assert_eq!(vol.root_lba(), 1024, "1 MiB / 512 / 2");
    let root = vol.root_lba();
    assert_eq!(walk_latin1(&mut vol, root, ""), fstool_fixture_tree());
    assert!(vol.validate().is_clean());
}

// ---------------------------------------------------------------------------
// Mutation agreement: two independent writers taking turns
// ---------------------------------------------------------------------------

/// fstool mutates a volume this crate created, in place.
///
/// The leg that matters most. `fstool add` / `rm` and the shell's
/// `mkdir` / `put` are its *incremental* path -- `AffsEditor`, which
/// touches only the bitmap, the parent's hash chain and the blocks it
/// allocates or frees, leaving the rest of the image byte-for-byte
/// alone. So it is allocating out of a bitmap this crate laid down,
/// splicing into hash chains this crate built, and freeing an extension
/// chain this crate wrote -- and then this crate has to still find every
/// block accounted for.
#[test]
fn this_crate_reads_a_volume_fstool_mutated_in_place() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-mutates");
    let added = pattern(9000);
    let host = scratch.path("payload");
    std::fs::write(&host, &added).unwrap();

    for (variant, _, _) in FSTOOL_CREATABLE {
        let byte = variant.dostype() & 0xFF;
        let image = populate_fstool_adf(&scratch, variant, "Turns", &format!("turns{byte}.adf"));
        let path = image.to_str().unwrap();

        let before = Volume::open(ImageDisk::open(&image), None)
            .unwrap()
            .read_bitmap()
            .unwrap()
            .allocated_count();

        // Its non-interactive half: one file in, two out. `big.dat`
        // spans an extension block, so freeing it is the case where a
        // writer that forgot the `T_LIST` chain leaks 80 blocks.
        fs_run(&bin, &["add", path, host.to_str().unwrap(), "/Devs/Added"]);
        fs_run(&bin, &["rm", path, "/empty"]);
        fs_run(&bin, &["rm", path, "/big.dat"]);

        // ...and its interactive half, which is where `mkdir` lives.
        fs_shell(
            &bin,
            path,
            &format!(
                "mkdir /NewDir\nput {} /NewDir/inner\nquit\n",
                host.to_str().unwrap()
            ),
        );

        let mut vol = Volume::open(ImageDisk::open(&image), None).expect("open");
        let root = vol.root_lba();

        // The tree we asked for, seen from here.
        let mut want = fstool_fixture_tree();
        want.retain(|(p, _)| p != "empty" && p != "big.dat");
        want.push(("Devs/Added".to_string(), EntryKind::File));
        want.push(("NewDir".to_string(), EntryKind::Directory));
        want.push(("NewDir/inner".to_string(), EntryKind::File));
        want.sort();
        assert_eq!(walk_latin1(&mut vol, root, ""), want, "{variant:?}");

        // Every byte of what it wrote, and of what was already there.
        for (amiga, bytes) in [
            ("Devs/Added", added.clone()),
            ("NewDir/inner", added.clone()),
            ("Devs/system-configuration", pattern(232)),
            ("S/Startup-Sequence", b"Echo \"Hello\"\n".to_vec()),
        ] {
            let entry = vol.lookup_path(root, amiga.as_bytes()).unwrap().unwrap();
            assert_eq!(
                vol.read_file(entry.lba).unwrap(),
                bytes,
                "{variant:?}: {amiga}"
            );
        }
        assert_eq!(
            read_named(&mut vol, root, b"Caf\xe9"),
            pattern(64),
            "{variant:?}"
        );

        // Zero findings -- and in particular no orphan, which is what a
        // `rm` that freed a header but not its data or extension blocks
        // would leave behind, and no reachable-but-free block, which is
        // what an `add` that allocated without marking would.
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after fstool mutated it: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(report.summary.orphans, 0, "{variant:?}");
        assert_eq!(report.summary.reachable_but_free, 0, "{variant:?}");

        // And the accounting moved the way the operations say it should:
        // a 40 000-byte file and an empty one gone, two 9 000-byte files
        // and a directory added. Both writers agree on the bit polarity
        // and the bit order, or this number would be nonsense.
        let after = vol.read_bitmap().unwrap().allocated_count();
        assert_eq!(after, report.summary.allocated, "{variant:?}");
        assert!(after < before, "{variant:?}: {after} vs {before}");
    }
}

/// Look a path up and hand back its header block -- the borrow dance
/// [`read_named`] does, for a path rather than a name.
fn vol_lba(vol: &mut Volume<ImageDisk>, dir: u64, name: &[u8]) -> u64 {
    vol.lookup(dir, name)
        .expect("lookup")
        .unwrap_or_else(|| panic!("{} is not there", latin1(name)))
        .lba
}

/// ...and the reverse: this crate's [`Mutator`] edits a volume fstool
/// created, and fstool reads the result.
///
/// The same image, mutated by two implementations in turn, checked by
/// the one that did not write last. Every operation the mutator has:
/// a directory and a file created, an entry renamed *across*
/// directories (which is a splice out of one hash chain and into
/// another), one deleted, and metadata set.
#[test]
fn fstool_reads_a_volume_this_crate_mutated() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-turns");
    let src = host_fixture_tree(&scratch);
    let created = pattern(9000);

    for (variant, fstype, intl) in FSTOOL_CREATABLE {
        let byte = variant.dostype() & 0xFF;
        let image = scratch.path(&format!("theirs{byte}.adf"));
        fs_run(
            &bin,
            &[
                "create",
                "-t",
                "affs",
                src.to_str().unwrap(),
                "-o",
                image.to_str().unwrap(),
                "--size",
                "880KiB",
                "-O",
                &format!("fstype={fstype},intl={intl},volume_label=Turns"),
            ],
        );

        let vol = Volume::open(ImageDisk::open(&image), None).expect("open");
        let root = vol.root_lba();
        let mut m = Mutator::open(vol).expect("mutator");
        let devs = m.volume().lookup(root, b"Devs").unwrap().unwrap().lba;
        let keymaps = m.volume().lookup(devs, b"Keymaps").unwrap().unwrap().lba;
        m.create_file(keymaps, b"usa1", &Metadata::new(), &created)
            .expect("create_file");
        m.create_dir(root, b"Libs", &Metadata::new())
            .expect("create_dir");
        // Across directories, and with a Latin-1 name on both ends.
        m.rename(root, b"Caf\xe9", devs, b"Th\xe9").expect("rename");
        m.delete(root, b"empty").expect("delete");
        let usa1 = m.volume().lookup(keymaps, b"usa1").unwrap().unwrap().lba;
        m.set_metadata(
            usa1,
            &MetaUpdate::new().protection(meta::FIBF_WRITE | meta::FIBF_DELETE),
        )
        .expect("set_metadata");
        let mut vol = m.into_volume();
        assert!(vol.validate().is_clean(), "{variant:?}: our own validator");
        vol.into_inner().save(&image);
        let path = image.to_str().unwrap();

        // fstool's turn: the tree it sees is the one we asked for.
        let mut found = Vec::new();
        fstool_tree(&bin, path, "/", "", &mut found);
        found.sort();
        let mut want = fstool_fixture_tree();
        want.retain(|(p, _)| p != "empty" && p != "Caf\u{e9}");
        want.push(("Devs/Keymaps/usa1".to_string(), EntryKind::File));
        want.push(("Devs/Th\u{e9}".to_string(), EntryKind::File));
        want.push(("Libs".to_string(), EntryKind::Directory));
        want.sort();
        assert_eq!(found, want, "{variant:?}: fstool's view of our mutations");

        // ...and every byte of what we created, of what we moved, and of
        // what neither of us touched.
        for (amiga, bytes) in [
            ("/Devs/Keymaps/usa1".to_string(), created.clone()),
            ("/Devs/Th\u{e9}".to_string(), pattern(64)),
            ("/big.dat".to_string(), pattern(40_000)),
        ] {
            assert_eq!(
                fs_run(&bin, &["cat", path, &amiga]),
                bytes,
                "{variant:?}: fstool read {amiga} differently"
            );
        }

        // Now it writes into the volume we mutated -- allocating from a
        // bitmap we edited in place -- and this crate still validates
        // what it left behind. Two writers, three turns, one image.
        let host = scratch.path("theirs-payload");
        std::fs::write(&host, pattern(2000)).unwrap();
        fs_run(&bin, &["add", path, host.to_str().unwrap(), "/Libs/Theirs"]);
        let mut vol = Volume::open(ImageDisk::open(&image), None).unwrap();
        let report = vol.validate();
        assert!(
            report.is_clean(),
            "{variant:?} after fstool wrote into our mutated volume: {:#?}",
            report
                .findings
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        let root = vol.root_lba();
        let libs = vol_lba(&mut vol, root, b"Libs");
        assert_eq!(read_named(&mut vol, libs, b"Theirs"), pattern(2000));
    }
}

/// The disagreement this leg found, asserted so it stays a fact.
///
/// `fstool` cannot *create* `DOS\4`/`DOS\5`, but it opens one and
/// mutates it without a word of complaint — and gets two things wrong,
/// both of which this crate's validator names. Neither is a judgement
/// call: AmigaDOS is the specification and both implementations are
/// trying to be it.
///
/// **One: the wrong fold table.** `Variant::from_flag` in
/// `fs/affs/mod.rs` decodes the boot flag byte as three independent bits
/// and sets `intl = flag & 2 != 0`, so `DOS\4` (flag 4) and `DOS\5`
/// (flag 5) come out `intl: false`. Directory-cache mode *implies*
/// international case folding — which is why [`Variant::is_intl`] here
/// is `!matches!(self, Ofs | Ffs)` rather than a bit test — so
/// `hash_name(name, self.variant.intl)` in `fs/affs/writer.rs` hashes an
/// accented name down the classic table. The header lands in a slot no
/// AmigaDOS lookup will ever walk: enumeration finds the file, `Lock()`
/// does not. This is invisible on `DOS\0`–`DOS\3`, where the bit test
/// and the rule agree, which is exactly why it survived.
///
/// **Two: the cache is not maintained.** `AffsEditor` has no notion of a
/// dircache, so the block stays as this crate left it. On `DOS\4`/`DOS\5`
/// AmigaDOS serves `List` *from the cache*, so the added file is not
/// merely hard to open — it is not there at all.
///
/// The conclusion is not "fix the test": it is that fstool's supported
/// set for writing is `DOS\0`–`DOS\3`, which is what
/// [`FSTOOL_CREATABLE`] says and what every other fstool test here
/// stays inside. This test exists so that if fstool grows dircache
/// support, the suite says so out loud instead of quietly gaining
/// coverage nobody noticed.
#[test]
fn fstool_now_correctly_mutates_a_dircache_volume_it_still_cannot_create() {
    let bin = fstool_oracle!();
    let scratch = Scratch::new("fstool-dc");
    let host = scratch.path("payload");
    std::fs::write(&host, b"x").unwrap();

    for variant in [Variant::OfsIntlDircache, Variant::FfsIntlDircache] {
        assert!(variant.is_intl(), "dircache mode implies international");
        let byte = variant.dostype() & 0xFF;
        let image = populate_fstool_adf(&scratch, variant, "Cached", &format!("dc{byte}.adf"));
        let path = image.to_str().unwrap();

        // `\u{e9}clair`: an accented first byte, so the two fold tables
        // used to disagree about it. Filed as KarpelesLab/fstool#42
        // (wrong fold table on DOS\4/DOS\5) and #43 (dircache never
        // maintained on write); both fixed in fstool 0.4.27. This test
        // used to assert the two resulting corruptions; now it asserts
        // they are gone, so a regression upstream is caught here too.
        fs_run(&bin, &["add", path, host.to_str().unwrap(), "/\u{e9}clair"]);

        let mut vol = Volume::open(ImageDisk::open(&image), None).expect("open");
        let root = vol.root_lba();

        assert!(
            walk_latin1(&mut vol, root, "")
                .iter()
                .any(|(p, _)| p == "\u{e9}clair"),
            "{variant:?}: fstool did not add the file at all"
        );
        // Lookup by name -- what an AmigaDOS `Lock()` does -- now finds
        // it too, under this volume's own (international) fold table.
        assert!(
            vol.lookup(root, b"\xe9clair").unwrap().is_some(),
            "{variant:?}: fstool's fold-table fix (#42) regressed"
        );

        let report = vol.validate();
        assert!(
            report.findings.is_empty(),
            "{variant:?}: {:#?}",
            report.findings
        );
        assert_eq!(report.summary.orphans, 0, "{variant:?}");
        assert_eq!(report.summary.reachable_but_free, 0, "{variant:?}");
    }
}
