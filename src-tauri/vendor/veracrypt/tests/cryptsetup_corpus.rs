//! cryptsetup's TrueCrypt / VeraCrypt test images (`tests/tcrypt-images.tar.xz`
//! of <https://gitlab.com/cryptsetup/cryptsetup>), made by the real programs and
//! not redistributed here. As in cryptsetup's `tcrypt-compat-test`, every volume
//! holds a FAT filesystem with serial DEAD-BABE and every hidden volume one with
//! CAFE-BABE; names give the version, PRF, mode and the cipher chain (the older
//! cascades by VeraCrypt's display name, the Kuznyechik ones in table order).
//!
//! ```text
//! VC_TCRYPT_IMAGES=<extracted>/tcrypt-images cargo test --release --test cryptsetup_corpus -- --nocapture
//! ```
//!
//! Unset, the test returns at once; that is not a pass.

use std::path::Path;

use veracrypt::{
    unlock, Control, FileSource, HeaderKind, HeaderSelection, KeyfilePool, Prf, SystemRange,
    UnlockRequest, VeraError, Volume,
};

const PASSWORD: &[u8] = b"aaaaaaaaaaaa";
const HIDDEN_PASSWORD: &[u8] = b"bbbbbbbbbbbb";
const PIM_PASSWORD: &[u8] = b"cccccccccccccccccccc";
const LONG_PASSWORD: &[u8] =
    b"aaaaaaaaaaaabbbbbbbbbbbbccccccccccccddddddddddddeeeeeeeeeeeeffffffffffff";
const SERIAL: u32 = 0xDEAD_BABE;
const HIDDEN_SERIAL: u32 = 0xCAFE_BABE;

fn control() -> Control<'static> {
    Control {
        cancelled: &|| false,
        progress: None,
        threads: None,
    }
}

/// The FAT volume serial of a boot sector (FAT12/16 or FAT32 layout).
fn fat_serial(boot: &[u8]) -> Option<u32> {
    let at = |offset: usize| u32::from_le_bytes(boot[offset..offset + 4].try_into().unwrap());
    match (boot[38], boot[66]) {
        (0x29, _) => Some(at(39)),
        (_, 0x29) => Some(at(67)),
        _ => None,
    }
}

fn serial(volume: &Volume<FileSource>, offset: u64) -> Option<u32> {
    let mut boot = [0u8; 512];
    volume.read_at(offset, &mut boot).ok()?;
    fat_serial(&boot)
}

fn prf_of(name: &str) -> Option<Prf> {
    Some(match name {
        "sha512" => Prf::Sha512,
        "sha256" => Prf::Sha256,
        "blake2s" => Prf::Blake2s,
        "whirlpool" => Prf::Whirlpool,
        "stribog512" => Prf::Streebog,
        "ripemd160" => Prf::Ripemd160,
        "argon2id" => Prf::Argon2id,
        _ => return None,
    })
}

fn open(
    path: &Path,
    password: &[u8],
    pim: u32,
    prf: Option<Prf>,
    headers: HeaderSelection,
) -> veracrypt::Result<Volume<FileSource>> {
    let request = UnlockRequest {
        password,
        pim,
        prf,
        headers,
    };
    unlock(FileSource::open(path)?, &request, &control())
}

/// The system partition of a test drive: MBR partition 1, or GPT partition 3,
/// cut at the end of the image as Linux does for the cut-down test drives.
fn system_partition(path: &Path, gpt: bool) -> (u64, u64) {
    let bytes = std::fs::read(path).unwrap();
    let le32 = |offset: usize| {
        u64::from(u32::from_le_bytes(
            bytes[offset..offset + 4].try_into().unwrap(),
        ))
    };
    let le64 = |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let (offset, length) = if gpt {
        let entries = le64(512 + 72) as usize * 512;
        let entry_size = le32(512 + 84) as usize;
        let third = entries + 2 * entry_size;
        let first = le64(third + 32);
        let last = le64(third + 40);
        (first * 512, (last - first + 1) * 512)
    } else {
        (le32(446 + 8) * 512, le32(446 + 12) * 512)
    };
    (offset, length.min(bytes.len() as u64 - offset))
}

#[test]
fn cryptsetup_images_open_like_cryptsetup() {
    let Ok(directory) = std::env::var("VC_TCRYPT_IMAGES") else {
        eprintln!("VC_TCRYPT_IMAGES unset: skipping the cryptsetup corpus");
        return;
    };
    let directory = Path::new(&directory);
    let keyfiles = [
        std::fs::read(directory.join("keyfile1")).unwrap(),
        std::fs::read(directory.join("keyfile2")).unwrap(),
    ];
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains('-'))
        .collect();
    names.sort();
    let (mut opened, mut legacy) = (0, 0);
    for name in &names {
        let path = directory.join(name);
        let fields: Vec<&str> = name.split('-').collect();
        let hidden = fields.last() == Some(&"hidden");
        let chain = fields[3..fields.len() - usize::from(hidden)].join("-");
        let (hash, mode) = (fields[1], fields[2]);
        // TrueCrypt before 5.0: CBC / LRW and SHA-1, which this reader does not
        // read. The named PRF keeps each failing search short.
        if mode != "xts" || prf_of(hash).is_none() {
            let prf = prf_of(hash).or(Some(Prf::Ripemd160));
            let outcome = open(&path, PASSWORD, 0, prf, HeaderSelection::Auto);
            assert!(
                matches!(outcome, Err(VeraError::AuthenticationFailed)),
                "{name}: {:?}",
                outcome.err()
            );
            legacy += 1;
            continue;
        }
        let pim: u32 = fields[0]
            .strip_prefix("vcpim_1_")
            .map_or(0, |digits| digits.parse().unwrap());
        let typed = match fields[0] {
            prefix if prefix.starts_with("vcpim_") => PIM_PASSWORD,
            "vck_1_nopw" => b"",
            "vck_1_pw72" => LONG_PASSWORD,
            _ => PASSWORD,
        };
        let password = if fields[0].starts_with("tck_") || fields[0].starts_with("vck_") {
            let mut pool = KeyfilePool::new(typed.len());
            for keyfile in &keyfiles {
                pool.add(keyfile).unwrap();
            }
            pool.apply(typed).unwrap().to_vec()
        } else {
            typed.to_vec()
        };
        // Above PIM 31 an automatic search leaves Argon2id out; name it.
        let prf = (hash == "argon2id" && pim > 31).then_some(Prf::Argon2id);

        if let Some(kind) = fields[0].strip_prefix("sys_") {
            let (offset, length) = system_partition(&path, kind.contains("_gpt_"));
            let partition = SystemRange::Partition { offset, length };
            let volume = open(
                &path,
                &password,
                pim,
                prf,
                HeaderSelection::System(partition),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(serial(&volume, 0), Some(SERIAL), "{name}: partition");
            assert_eq!(volume.info().header, HeaderKind::System, "{name}");
            let drive = open(
                &path,
                &password,
                pim,
                prf,
                HeaderSelection::System(SystemRange::Drive),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(serial(&drive, offset), Some(SERIAL), "{name}: drive");
            println!(
                "{name}: system {} / {} {}, partition {offset}+{length}, area {}+{}",
                volume.info().chain.name(),
                volume.info().kdf.prf.name(),
                volume.info().kdf.iterations,
                volume.info().encrypted_area_start,
                volume.info().encrypted_area_length
            );
            opened += 1;
            continue;
        }

        let volume = open(&path, &password, pim, prf, HeaderSelection::Auto)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let info = volume.info();
        let table: Vec<&str> = info
            .chain
            .ciphers()
            .iter()
            .map(|cipher| cipher.name())
            .collect();
        assert!(
            [info.chain.name(), table.join("-")]
                .iter()
                .any(|spelling| spelling.to_lowercase() == chain),
            "{name}: cipher chain {}",
            info.chain.name()
        );
        assert_eq!(Some(info.kdf.prf), prf_of(hash), "{name}: PRF");
        assert_eq!(serial(&volume, 0), Some(SERIAL), "{name}");
        println!(
            "{name}: {} {} / {} {}{}",
            info.flavor.name(),
            info.chain.name(),
            info.kdf.prf.name(),
            info.kdf.iterations,
            if info.kdf.memory_kib > 0 {
                format!(" ({} MiB)", info.kdf.memory_kib / 1024)
            } else {
                String::new()
            }
        );
        opened += 1;
        if hidden {
            let volume = open(&path, HIDDEN_PASSWORD, pim, prf, HeaderSelection::Auto)
                .unwrap_or_else(|error| panic!("{name} (hidden): {error}"));
            assert!(
                volume.info().header.is_hidden(),
                "{name}: {:?}",
                volume.info().header
            );
            assert_eq!(serial(&volume, 0), Some(HIDDEN_SERIAL), "{name} (hidden)");
            println!("{name}: hidden volume via {}", volume.info().header.id());
            opened += 1;
        }
    }
    println!("{opened} volumes opened, {legacy} pre-5.0 TrueCrypt images refused");
    assert!(opened >= 70, "only {opened} volumes opened");
}
