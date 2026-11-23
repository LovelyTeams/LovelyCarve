//! Whole-volume tests over containers built with independent primitives:
//! the `pbkdf2` and `xts-mode` crates, not this crate's KDF or XTS code.

use std::sync::atomic::{AtomicU64, Ordering};

use aes::cipher::KeyInit;
use xts_mode::{get_tweak_default, Xts128};

use crate::*;

const PASSWORD: &[u8] = b"lovelycarve-test";

fn control() -> Control<'static> {
    Control {
        cancelled: &|| false,
        progress: None,
        threads: None,
    }
}

fn request(
    password: &[u8],
    pim: u32,
    prf: Option<Prf>,
    headers: HeaderSelection,
) -> UnlockRequest<'_> {
    UnlockRequest {
        password,
        pim,
        prf,
        headers,
    }
}

#[derive(Clone)]
struct Spec {
    magic: [u8; 4],
    version: u16,
    required: u16,
    hidden_size: u64,
    volume_size: u64,
    area_start: u64,
    area_length: u64,
    flags: u32,
    sector_size: u32,
    master: [u8; 64],
}

impl Spec {
    fn volume(area_start: u64, size: u64) -> Self {
        let mut master = [0u8; 64];
        for (index, byte) in master.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37) ^ 0xa5;
        }
        Spec {
            magic: *b"VERA",
            version: 5,
            required: 0x010b,
            hidden_size: 0,
            volume_size: size,
            area_start,
            area_length: size,
            flags: 0,
            sector_size: 512,
            master,
        }
    }

    fn plain_header(&self) -> [u8; 448] {
        let mut data = [0u8; 448];
        data[0..4].copy_from_slice(&self.magic);
        data[4..6].copy_from_slice(&self.version.to_be_bytes());
        data[6..8].copy_from_slice(&self.required.to_be_bytes());
        data[28..36].copy_from_slice(&self.hidden_size.to_be_bytes());
        data[36..44].copy_from_slice(&self.volume_size.to_be_bytes());
        data[44..52].copy_from_slice(&self.area_start.to_be_bytes());
        data[52..60].copy_from_slice(&self.area_length.to_be_bytes());
        data[60..64].copy_from_slice(&self.flags.to_be_bytes());
        data[64..68].copy_from_slice(&self.sector_size.to_be_bytes());
        data[192..256].copy_from_slice(&self.master);
        let keys = crc32fast::hash(&data[192..]);
        data[8..12].copy_from_slice(&keys.to_be_bytes());
        let fields = crc32fast::hash(&data[..188]);
        data[188..192].copy_from_slice(&fields.to_be_bytes());
        data
    }

    /// The header sector, its key derived with SHA-512 or SHA-256.
    fn sector(&self, password: &[u8], salt_seed: u8, sha256: bool, iterations: u32) -> [u8; 512] {
        let salt: Vec<u8> = (0..64u8)
            .map(|index| index.wrapping_mul(7) ^ salt_seed)
            .collect();
        let mut key = [0u8; 64];
        if sha256 {
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, &salt, iterations, &mut key);
        } else {
            pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, &salt, iterations, &mut key);
        }
        let mut header = self.plain_header().to_vec();
        aes_xts(&key).encrypt_area(&mut header, 448, 0, get_tweak_default);
        let mut sector = [0u8; 512];
        sector[..64].copy_from_slice(&salt);
        sector[64..].copy_from_slice(&header);
        sector
    }
}

fn aes_xts(key: &[u8]) -> Xts128<aes::Aes256> {
    Xts128::new(
        aes::Aes256::new_from_slice(&key[..32]).unwrap(),
        aes::Aes256::new_from_slice(&key[32..64]).unwrap(),
    )
}

fn plaintext(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(13) ^ seed)
        .collect()
}

/// Encrypt `data` as the data area at absolute offset `start`.
fn encrypt_at(master: &[u8; 64], data: &[u8], start: u64) -> Vec<u8> {
    let mut encrypted = data.to_vec();
    aes_xts(master).encrypt_area(
        &mut encrypted,
        512,
        u128::from(start / 512),
        get_tweak_default,
    );
    encrypted
}

/// A V2 container: normal header at 0, data at 128 KiB, embedded backups.
fn container(data: &[u8], pim_iterations: u32) -> (Vec<u8>, Spec) {
    let spec = Spec::volume(131_072, data.len() as u64);
    let mut image = vec![0u8; 131_072 + data.len() + 131_072];
    image[..512].copy_from_slice(&spec.sector(PASSWORD, 1, false, pim_iterations));
    image[131_072..131_072 + data.len()].copy_from_slice(&encrypt_at(&spec.master, data, 131_072));
    let backup = image.len() - 131_072;
    image[backup..backup + 512].copy_from_slice(&spec.sector(PASSWORD, 2, false, pim_iterations));
    (image, spec)
}

#[test]
fn a_container_unlocks_and_reads_aligned_unaligned_and_parallel_spans() {
    let data = plaintext(512 * 1024, 0x3c);
    let (image, _) = container(&data, 16_000);
    let volume = unlock(
        &image,
        &request(PASSWORD, 1, Some(Prf::Sha512), HeaderSelection::Auto),
        &control(),
    )
    .unwrap();
    let info = volume.info();
    assert_eq!(
        (
            info.flavor,
            info.header,
            info.kdf.prf,
            info.kdf.iterations,
            info.chain.name(),
            info.size
        ),
        (
            Flavor::VeraCrypt,
            HeaderKind::Normal,
            Prf::Sha512,
            16_000,
            "AES".to_string(),
            data.len() as u64
        )
    );
    assert!(info.encryption_complete && !info.weak_master_key && !info.plaintext_boot_sector);
    let mut whole = vec![0u8; data.len()];
    volume.read_at(0, &mut whole).unwrap();
    assert_eq!(whole, data, "parallel full read");
    for (offset, len) in [
        (0usize, 512usize),
        (511, 2),
        (1000, 3000),
        (data.len() - 7, 7),
        (4096, 0),
    ] {
        let mut part = vec![0u8; len];
        volume.read_at(offset as u64, &mut part).unwrap();
        assert_eq!(part, data[offset..offset + len], "{offset}+{len}");
    }
    let mut past = [0u8; 2];
    assert!(volume.read_at(data.len() as u64 - 1, &mut past).is_err());
}

#[test]
fn the_backup_header_opens_a_volume_whose_primary_header_is_gone() {
    let data = plaintext(8192, 0x11);
    let (mut image, _) = container(&data, 16_000);
    image[..512].fill(0);
    let auto = request(PASSWORD, 1, Some(Prf::Sha512), HeaderSelection::Auto);
    assert!(matches!(
        unlock(&image, &auto, &control()),
        Err(VeraError::AuthenticationFailed)
    ));
    let backup = request(PASSWORD, 1, Some(Prf::Sha512), HeaderSelection::Backup);
    let volume = unlock(&image, &backup, &control()).unwrap();
    assert_eq!(volume.info().header, HeaderKind::BackupNormal);
    let mut read = vec![0u8; data.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read, data);
}

#[test]
fn auto_finds_the_hidden_volume_its_password_opens() {
    let outer = plaintext(196_608, 0x01);
    let (mut image, _) = container(&outer, 16_000);
    // The hidden volume sits inside the outer data area, as VeraCrypt puts it.
    let hidden_data = plaintext(16_384, 0x7e);
    let hidden_start = 131_072 + 65_536;
    let mut hidden = Spec::volume(hidden_start, hidden_data.len() as u64);
    hidden.hidden_size = hidden_data.len() as u64;
    hidden.master[0] ^= 0xff;
    image[65_536..65_536 + 512].copy_from_slice(&hidden.sector(
        b"hidden password",
        3,
        false,
        16_000,
    ));
    let encrypted = encrypt_at(&hidden.master, &hidden_data, hidden_start);
    image[hidden_start as usize..hidden_start as usize + hidden_data.len()]
        .copy_from_slice(&encrypted);
    let hidden_request = request(
        b"hidden password",
        1,
        Some(Prf::Sha512),
        HeaderSelection::Auto,
    );
    let volume = unlock(&image, &hidden_request, &control()).unwrap();
    assert_eq!(
        (
            volume.info().header,
            volume.info().hidden_volume_size,
            volume.info().view_start
        ),
        (HeaderKind::Hidden, 16_384, hidden_start)
    );
    let mut read = vec![0u8; hidden_data.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read, hidden_data);
    let normal_only = request(
        b"hidden password",
        1,
        Some(Prf::Sha512),
        HeaderSelection::Normal,
    );
    assert!(unlock(&image, &normal_only, &control()).is_err());
    let outer_volume = unlock(
        &image,
        &request(PASSWORD, 1, Some(Prf::Sha512), HeaderSelection::Auto),
        &control(),
    )
    .unwrap();
    assert_eq!(outer_volume.info().header, HeaderKind::Normal);
}

/// A drive with a system-encrypted partition: header in sector 62, SHA-256
/// at the boot count (2048 per PIM), absolute data unit numbers.
fn system_drive(encrypted_len: u64, boot_sector_plain: bool) -> (Vec<u8>, Vec<u8>, Spec) {
    let partition = plaintext(256 * 1024, 0x5a);
    let start = 64 * 1024u64;
    let mut spec = Spec::volume(start, partition.len() as u64);
    spec.area_length = encrypted_len;
    spec.flags = FLAG_SYSTEM;
    let mut drive = vec![0u8; 512 * 1024];
    drive[..2].copy_from_slice(&[0x55, 0xaa]);
    drive[31_744..31_744 + 512].copy_from_slice(&spec.sector(PASSWORD, 9, true, 2048));
    let end = start as usize + partition.len();
    drive[start as usize..end].copy_from_slice(&partition);
    let encrypted = encrypt_at(&spec.master, &partition[..encrypted_len as usize], start);
    drive[start as usize..start as usize + encrypted.len()].copy_from_slice(&encrypted);
    if boot_sector_plain {
        drive[start as usize..start as usize + 8]
            .copy_from_slice(&[0xEB, 0x52, 0x90, b'N', b'T', b'F', b'S', b' ']);
    }
    (drive, partition, spec)
}

#[test]
fn system_encryption_opens_a_partition_or_the_whole_drive() {
    let (drive, partition, _) = system_drive(256 * 1024, false);
    let range = SystemRange::Partition {
        offset: 64 * 1024,
        length: partition.len() as u64,
    };
    let system = request(PASSWORD, 1, None, HeaderSelection::System(range));
    let volume = unlock(&drive, &system, &control()).unwrap();
    let info = volume.info();
    assert_eq!(
        (
            info.header,
            info.kdf.prf,
            info.kdf.iterations,
            info.size,
            info.encryption_complete
        ),
        (
            HeaderKind::System,
            Prf::Sha256,
            2048,
            partition.len() as u64,
            true
        )
    );
    assert!(info.system());
    let mut read = vec![0u8; partition.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read, partition);

    let drive_view = unlock(
        &drive,
        &request(
            PASSWORD,
            1,
            None,
            HeaderSelection::System(SystemRange::Drive),
        ),
        &control(),
    )
    .unwrap();
    assert_eq!(drive_view.size(), drive.len() as u64);
    let mut whole = vec![0u8; drive.len()];
    drive_view.read_at(0, &mut whole).unwrap();
    assert_eq!(whole[..2], [0x55, 0xaa], "the boot area reads as stored");
    assert_eq!(whole[64 * 1024..64 * 1024 + partition.len()], partition[..]);
    // A partition starting outside the encrypted area is not this volume's.
    let outside = SystemRange::Partition {
        offset: 0,
        length: 64 * 1024,
    };
    assert!(unlock(
        &drive,
        &request(PASSWORD, 1, None, HeaderSelection::System(outside)),
        &control()
    )
    .is_err());
}

#[test]
fn unfinished_system_encryption_and_a_rewritten_boot_sector_read_as_veracrypt_does() {
    let (drive, partition, _) = system_drive(64 * 1024, true);
    let range = SystemRange::Partition {
        offset: 64 * 1024,
        length: partition.len() as u64,
    };
    let volume = unlock(
        &drive,
        &request(
            PASSWORD,
            1,
            Some(Prf::Sha256),
            HeaderSelection::System(range),
        ),
        &control(),
    )
    .unwrap();
    let info = volume.info();
    assert!(!info.encryption_complete && info.plaintext_boot_sector);
    let mut read = vec![0u8; partition.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read[..8], [0xEB, 0x52, 0x90, b'N', b'T', b'F', b'S', b' ']);
    assert_eq!(
        read[512..],
        partition[512..],
        "encrypted head and stored tail"
    );
}

#[test]
fn truecrypt_headers_open_with_truecrypt_iteration_counts() {
    let data = plaintext(4096, 0x42);
    let mut spec = Spec::volume(131_072, data.len() as u64);
    spec.magic = *b"TRUE";
    spec.required = 0x0700;
    let mut image = vec![0u8; 131_072 + data.len()];
    image[..512].copy_from_slice(&spec.sector(PASSWORD, 4, false, 1000));
    image[131_072..].copy_from_slice(&encrypt_at(&spec.master, &data, 131_072));
    let volume = unlock(
        &image,
        &request(PASSWORD, 0, Some(Prf::Sha512), HeaderSelection::Auto),
        &control(),
    )
    .unwrap();
    assert_eq!(
        (
            volume.info().flavor,
            volume.info().kdf.family,
            volume.info().kdf.iterations
        ),
        (Flavor::TrueCrypt, Family::TrueCrypt, 1000)
    );
    let mut read = vec![0u8; data.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read, data);
}

#[test]
fn wrong_passwords_fail_and_every_derivation_reports_progress() {
    let (image, _) = container(&plaintext(4096, 0), 16_000);
    let done = AtomicU64::new(0);
    let total = AtomicU64::new(0);
    let progress = |step: Progress| {
        done.store(step.done, Ordering::Relaxed);
        total.store(step.total, Ordering::Relaxed);
    };
    let watched = Control {
        cancelled: &|| false,
        progress: Some(&progress),
        threads: Some(3),
    };
    let wrong = request(b"wrong", 1, None, HeaderSelection::Normal);
    assert!(matches!(
        unlock(&image, &wrong, &watched),
        Err(VeraError::AuthenticationFailed)
    ));
    // SHA-512 3, SHA-256 6, BLAKE2s 6, Whirlpool 3, Streebog 3, RIPEMD-160 10
    // PBKDF2 blocks, plus one Argon2id.
    assert_eq!(
        (done.load(Ordering::Relaxed), total.load(Ordering::Relaxed)),
        (32, 32)
    );
}

#[test]
fn a_cancelled_search_stops_with_cancelled() {
    let (image, _) = container(&plaintext(4096, 0), 16_000);
    let cancelled = Control {
        cancelled: &|| true,
        progress: None,
        threads: Some(2),
    };
    assert!(matches!(
        unlock(
            &image,
            &request(b"wrong", 0, None, HeaderSelection::Auto),
            &cancelled
        ),
        Err(VeraError::Cancelled)
    ));
}

#[test]
fn header_limits_and_master_key_warnings_are_reported() {
    let data = plaintext(4096, 0);
    let mut spec = Spec::volume(131_072, data.len() as u64);
    spec.version = 6;
    let mut image = vec![0u8; 131_072 + data.len()];
    image[..512].copy_from_slice(&spec.sector(PASSWORD, 5, false, 16_000));
    let sha512 = request(PASSWORD, 1, Some(Prf::Sha512), HeaderSelection::Normal);
    assert!(matches!(
        unlock(&image, &sha512, &control()),
        Err(VeraError::Unsupported(_))
    ));

    let mut spec = Spec::volume(131_072, data.len() as u64);
    spec.sector_size = 4096;
    let primary: [u8; 32] = spec.master[..32].try_into().unwrap();
    spec.master[32..].copy_from_slice(&primary);
    image[..512].copy_from_slice(&spec.sector(PASSWORD, 5, false, 16_000));
    image[131_072..].copy_from_slice(&encrypt_at(&spec.master, &data, 131_072));
    let volume = unlock(&image, &sha512, &control()).unwrap();
    assert_eq!(
        (volume.info().sector_size, volume.info().weak_master_key),
        (4096, true)
    );
    let mut read = vec![0u8; data.len()];
    volume.read_at(0, &mut read).unwrap();
    assert_eq!(read, data);
}

#[test]
fn requests_out_of_range_fail_before_reading() {
    let (image, _) = container(&plaintext(4096, 0), 16_000);
    assert!(matches!(
        unlock(
            &image,
            &request(b"", 0, None, HeaderSelection::Auto),
            &control()
        ),
        Err(VeraError::InvalidRequest(_))
    ));
    assert!(matches!(
        unlock(
            &image,
            &request(&[b'a'; 129], 0, None, HeaderSelection::Auto),
            &control()
        ),
        Err(VeraError::InvalidRequest(_))
    ));
    assert!(matches!(
        unlock(
            &image,
            &request(PASSWORD, MAX_PIM + 1, None, HeaderSelection::Auto),
            &control()
        ),
        Err(VeraError::InvalidRequest(_))
    ));
    let tiny = vec![0u8; 100];
    assert!(matches!(
        unlock(
            &tiny,
            &request(PASSWORD, 1, None, HeaderSelection::Auto),
            &control()
        ),
        Err(VeraError::TooSmall { .. })
    ));
}
