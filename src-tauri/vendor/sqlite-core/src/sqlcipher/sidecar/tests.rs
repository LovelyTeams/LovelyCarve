//! Sidecars of a two-page `SQLCipher` 4 database (raw key), encrypted here with
//! `aes` and `hmac` directly. Each test reads the encrypted and the decrypted
//! sidecar with the crate's own readers and expects the same verdicts: which
//! commits verify, which records verify, where the records start.

use cipher::block_padding::NoPadding;
use cipher::{BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha2::Sha512;

use super::super::{
    decrypt, derive_keys, CipherParams, Decrypted, PageCipher, PageLayout, SqlCipherKey, IV_LEN,
    KEY_LEN, SALT_LEN, SQLITE_MAGIC,
};
use super::{
    frame_checksum, DecryptedSidecar, SidecarError, JOURNAL_HEADER_LEN, WAL_FRAME_HEADER_LEN,
    WAL_HEADER_LEN,
};
use crate::{Database, JournalHeader, RollbackJournal, WalChecksumEndian};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

const PAGE_SIZE: usize = 4096;
const SALTS: [u8; 8] = [0xaa, 0xbb, 0xcc, 0xdd, 0x11, 0x22, 0x33, 0x44];
const NONCE: u32 = 0x1357_2468;

/// The encrypted database, its plaintext pages, and what decrypting it gives.
struct Fixture {
    cipher: PageCipher,
    salt: [u8; SALT_LEN],
    database: Decrypted,
}

impl Fixture {
    fn new() -> Self {
        let profile = CipherParams::compatibility(4).expect("v4").profile();
        let key = SqlCipherKey::RawKey([0x42; KEY_LEN]);
        let salt = [0x17; SALT_LEN];
        let (enc_key, hmac_key) = derive_keys(&profile, &key, &salt);
        let cipher = PageCipher {
            profile,
            enc_key,
            hmac_key,
        };
        let mut encrypted = encrypt(&cipher, &salt, &page_one(), 1, 0x01);
        encrypted.extend(encrypt(&cipher, &salt, &leaf(b"original"), 2, 0x02));
        let database = decrypt(&encrypted, &key).expect("database");
        Self {
            cipher,
            salt,
            database,
        }
    }

    fn encrypt(&self, plain: &[u8], pgno: u32, iv: u8) -> Vec<u8> {
        encrypt(&self.cipher, &self.salt, plain, pgno, iv)
    }

    /// `encrypted` as decryption returns it: the plaintext body before the
    /// reserved tail, then the tail (IV and HMAC) as stored.
    fn decrypted(&self, plain: &[u8], encrypted: &[u8]) -> Vec<u8> {
        let tail = PAGE_SIZE - self.cipher.profile.reserve;
        [&plain[..tail], &encrypted[tail..]].concat()
    }
}

/// A page 1 the page-one check accepts.
fn page_one() -> Vec<u8> {
    let mut page = vec![0; PAGE_SIZE];
    page[..SALT_LEN].copy_from_slice(SQLITE_MAGIC);
    page[16..18].copy_from_slice(&(PAGE_SIZE as u16).to_be_bytes());
    page[18] = 1;
    page[19] = 1;
    page[20] = 80;
    page[21..24].copy_from_slice(&[64, 32, 32]);
    page[28..32].copy_from_slice(&2_u32.to_be_bytes());
    page[100] = 0x0d;
    page
}

/// An empty table leaf carrying `marker`.
fn leaf(marker: &[u8]) -> Vec<u8> {
    let mut page = vec![0; PAGE_SIZE];
    page[0] = 0x0d;
    page[16..16 + marker.len()].copy_from_slice(marker);
    page
}

fn encrypt(cipher: &PageCipher, salt: &[u8; SALT_LEN], plain: &[u8], pgno: u32, iv: u8) -> Vec<u8> {
    let profile = &cipher.profile;
    let layout = PageLayout::for_page(profile, pgno).expect("layout");
    let iv = [iv; IV_LEN];
    let mut out = vec![0; profile.page_size];
    if pgno == 1 {
        out[..SALT_LEN].copy_from_slice(salt);
    }
    let mut body = plain[layout.start..layout.iv_start].to_vec();
    let length = body.len();
    let encrypted = Aes256CbcEnc::new_from_slices(&cipher.enc_key, &iv)
        .expect("key")
        .encrypt_padded_mut::<NoPadding>(&mut body, length)
        .expect("whole blocks");
    out[layout.start..layout.iv_start].copy_from_slice(encrypted);
    out[layout.iv_start..layout.iv_start + IV_LEN].copy_from_slice(&iv);
    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&cipher.hmac_key).expect("hmac key");
    mac.update(&out[layout.start..layout.iv_start + IV_LEN]);
    mac.update(&pgno.to_le_bytes());
    out[layout.iv_start + IV_LEN..layout.iv_start + IV_LEN + profile.hmac_len]
        .copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// A WAL as `SQLite` writes it, frame by frame.
struct Wal {
    bytes: Vec<u8>,
    chain: (u32, u32),
}

impl Wal {
    fn new() -> Self {
        let mut bytes = vec![0; WAL_HEADER_LEN];
        bytes[..4].copy_from_slice(&0x377f_0683_u32.to_be_bytes());
        bytes[4..8].copy_from_slice(&3_007_000_u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&(PAGE_SIZE as u32).to_be_bytes());
        bytes[16..24].copy_from_slice(&SALTS);
        let chain = crate::wal_checksum(WalChecksumEndian::Big, 0, 0, &bytes[..24]);
        bytes[24..28].copy_from_slice(&chain.0.to_be_bytes());
        bytes[28..32].copy_from_slice(&chain.1.to_be_bytes());
        Self { bytes, chain }
    }

    /// Append a frame; `commit` is the database size it commits, 0 for none.
    fn frame(mut self, pgno: u32, commit: u32, salts: [u8; 8], page: &[u8]) -> Self {
        let mut header = [0; WAL_FRAME_HEADER_LEN];
        header[..4].copy_from_slice(&pgno.to_be_bytes());
        header[4..8].copy_from_slice(&commit.to_be_bytes());
        header[8..16].copy_from_slice(&salts);
        self.chain = frame_checksum(WalChecksumEndian::Big, self.chain, &header, page);
        header[16..20].copy_from_slice(&self.chain.0.to_be_bytes());
        header[20..24].copy_from_slice(&self.chain.1.to_be_bytes());
        self.bytes.extend_from_slice(&header);
        self.bytes.extend_from_slice(page);
        self
    }

    /// Flip a bit of the `index`-th frame at `offset` within the frame.
    fn corrupt(mut self, index: usize, offset: usize) -> Self {
        self.bytes[WAL_HEADER_LEN + index * (WAL_FRAME_HEADER_LEN + PAGE_SIZE) + offset] ^= 1;
        self
    }
}

/// Whether each commit of `wal` verifies, by the crate's WAL reader.
fn trusted_commits(database: &[u8], wal: &[u8]) -> Vec<bool> {
    Database::wal_timeline_from(database, wal)
        .expect("timeline")
        .commit_snapshots()
        .iter()
        .map(crate::CommitSnapshot::checksum_valid)
        .collect()
}

#[test]
fn wal_frames_decrypt_with_the_checksums_sqlite_would_have_written() {
    let fixture = Fixture::new();
    let (second, first) = (leaf(b"updated"), page_one());
    let (second_encrypted, first_encrypted) = (
        fixture.encrypt(&second, 2, 0x03),
        fixture.encrypt(&first, 1, 0x04),
    );
    let wal = Wal::new()
        .frame(2, 2, SALTS, &second_encrypted)
        .frame(1, 2, SALTS, &first_encrypted);

    let sidecar = fixture.database.decrypt_wal(&wal.bytes).expect("wal");
    assert_eq!(sidecar.omitted, 0);
    let plain = &fixture.database.plaintext;
    assert_eq!(trusted_commits(plain, &wal.bytes), vec![true, true]);
    assert_eq!(trusted_commits(plain, &sidecar.plaintext), vec![true, true]);
    let replayed = Database::open_with_wal(plain.clone(), &sidecar.plaintext).expect("open");
    let pages: Vec<(u32, Vec<u8>)> = replayed
        .wal_frame_pages()
        .iter()
        .map(|frame| (frame.page_no, frame.page.clone()))
        .collect();
    assert_eq!(
        pages,
        vec![
            (2, fixture.decrypted(&second, &second_encrypted)),
            (1, fixture.decrypted(&first, &first_encrypted)),
        ]
    );
}

#[test]
fn a_checksum_chain_that_breaks_stays_broken() {
    let fixture = Fixture::new();
    let page = |marker: &[u8], iv| fixture.encrypt(&leaf(marker), 2, iv);
    let other_salts = [0x99; 8];
    let wal = Wal::new()
        .frame(2, 2, SALTS, &page(b"first", 0x05))
        .frame(2, 2, SALTS, &page(b"second", 0x06))
        .frame(2, 2, SALTS, &page(b"third", 0x07))
        .frame(2, 2, other_salts, &page(b"older", 0x08))
        .corrupt(1, 16);

    let sidecar = fixture.database.decrypt_wal(&wal.bytes).expect("wal");
    assert_eq!(sidecar.omitted, 0);
    let plain = &fixture.database.plaintext;
    let verdicts = trusted_commits(plain, &wal.bytes);
    assert_eq!(&verdicts[..3], [true, false, false]);
    assert_eq!(trusted_commits(plain, &sidecar.plaintext), verdicts);
}

#[test]
fn decryption_stops_at_the_first_frame_that_does_not_authenticate() {
    let fixture = Fixture::new();
    let page = |marker: &[u8], iv| fixture.encrypt(&leaf(marker), 2, iv);
    let wal = Wal::new()
        .frame(2, 2, SALTS, &page(b"first", 0x09))
        .frame(2, 2, SALTS, &page(b"second", 0x0a))
        .frame(2, 2, SALTS, &page(b"third", 0x0b))
        .corrupt(1, WAL_FRAME_HEADER_LEN + 100);

    let sidecar = fixture.database.decrypt_wal(&wal.bytes).expect("wal");
    assert_eq!(sidecar.omitted, 2);
    assert_eq!(
        sidecar.plaintext.len(),
        WAL_HEADER_LEN + WAL_FRAME_HEADER_LEN + PAGE_SIZE
    );
}

#[test]
fn wal_headers_are_checked() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.database.decrypt_wal(&[0; 16]),
        Err(SidecarError::Malformed)
    );
    assert_eq!(
        fixture.database.decrypt_wal(&[0x11; WAL_HEADER_LEN]),
        Err(SidecarError::Malformed)
    );
    let mut wal = Wal::new().bytes;
    wal[8..12].copy_from_slice(&1024_u32.to_be_bytes());
    assert_eq!(
        fixture.database.decrypt_wal(&wal),
        Err(SidecarError::PageSizeMismatch {
            database: 4096,
            wal: 1024
        })
    );
}

/// A journal record as `SQLite` writes it; `valid` picks a right or a wrong
/// checksum over the (encrypted) page.
fn record(pgno: u32, page: &[u8], valid: bool) -> Vec<u8> {
    let checksum = crate::journal_cksum(NONCE, page) ^ u32::from(!valid);
    [&pgno.to_be_bytes()[..], page, &checksum.to_be_bytes()].concat()
}

/// A journal header padded to `sector` bytes; `None` for one PERSIST zeroed.
fn journal_header(records: Option<u32>, sector: usize) -> Vec<u8> {
    let mut header = vec![0; sector];
    if let Some(records) = records {
        header[..8].copy_from_slice(&crate::JOURNAL_MAGIC);
        header[8..12].copy_from_slice(&records.to_be_bytes());
        header[12..16].copy_from_slice(&NONCE.to_be_bytes());
        header[16..20].copy_from_slice(&2_u32.to_be_bytes());
        header[20..24].copy_from_slice(&(sector as u32).to_be_bytes());
        header[24..28].copy_from_slice(&(PAGE_SIZE as u32).to_be_bytes());
    }
    header
}

/// A record as the journal reader returns it: page number, bytes, checksum verdict.
type JournalRecord = (u32, Vec<u8>, Option<bool>);

/// What the journal reader makes of `journal`: its header and its records.
fn read_journal(journal: &[u8]) -> (JournalHeader, Vec<JournalRecord>) {
    let parsed = RollbackJournal::parse(journal, PAGE_SIZE as u32).expect("journal");
    let records = parsed
        .page_images()
        .iter()
        .map(|image| (image.pgno, image.bytes.clone(), image.checksum_valid))
        .collect();
    (*parsed.header(), records)
}

#[test]
fn an_intact_journal_header_keeps_each_records_checksum_verdict() {
    let fixture = Fixture::new();
    let (second, first) = (leaf(b"before"), page_one());
    let (second_encrypted, first_encrypted) = (
        fixture.encrypt(&second, 2, 0x0c),
        fixture.encrypt(&first, 1, 0x0d),
    );
    let journal = [
        journal_header(Some(2), 512),
        record(2, &second_encrypted, true),
        record(1, &first_encrypted, false),
    ]
    .concat();

    let sidecar = fixture
        .database
        .decrypt_rollback_journal(&journal)
        .expect("journal");
    assert_eq!(sidecar.omitted, 0);
    let (encrypted_header, encrypted_records) = read_journal(&journal);
    let (header, records) = read_journal(&sidecar.plaintext);
    assert_eq!(header, encrypted_header);
    assert_eq!(
        records,
        vec![
            (2, fixture.decrypted(&second, &second_encrypted), Some(true)),
            (1, fixture.decrypted(&first, &first_encrypted), Some(false)),
        ]
    );
    let verdicts: Vec<Option<bool>> = encrypted_records.iter().map(|record| record.2).collect();
    assert_eq!(verdicts, vec![Some(true), Some(false)]);
}

#[test]
fn a_zeroed_journal_header_is_read_from_the_same_sector() {
    let fixture = Fixture::new();
    let (second, first) = (leaf(b"before"), page_one());
    let (second_encrypted, first_encrypted) = (
        fixture.encrypt(&second, 2, 0x0e),
        fixture.encrypt(&first, 1, 0x0f),
    );
    let journal = [
        journal_header(None, 4096),
        record(2, &second_encrypted, true),
        record(1, &first_encrypted, true),
    ]
    .concat();

    let sidecar = fixture
        .database
        .decrypt_rollback_journal(&journal)
        .expect("journal");
    let (header, records) = read_journal(&sidecar.plaintext);
    assert_eq!(header, read_journal(&journal).0);
    assert_eq!(
        header,
        JournalHeader::ReconstructedZeroed {
            page_size: 4096,
            sector_size: 4096
        }
    );
    assert_eq!(
        records,
        vec![
            (2, fixture.decrypted(&second, &second_encrypted), None),
            (1, fixture.decrypted(&first, &first_encrypted), None),
        ]
    );
    // Without the nonce nothing can check the stored checksums: kept as they were.
    let tail = |journal: &[u8]| journal[journal.len() - 4..].to_vec();
    assert_eq!(tail(&sidecar.plaintext), tail(&journal));
}

#[test]
fn journal_decryption_stops_at_the_first_record_that_does_not_authenticate() {
    let fixture = Fixture::new();
    let page = |marker: &[u8], iv| fixture.encrypt(&leaf(marker), 2, iv);
    let mut tampered = page(b"second", 0x11);
    tampered[100] ^= 1;
    let journal = [
        journal_header(Some(0), 512),
        record(2, &page(b"first", 0x10), true),
        record(2, &tampered, true),
        record(2, &page(b"third", 0x12), true),
    ]
    .concat();

    let sidecar: DecryptedSidecar = fixture
        .database
        .decrypt_rollback_journal(&journal)
        .expect("journal");
    assert_eq!(sidecar.omitted, 2);
    assert_eq!(sidecar.plaintext.len(), 512 + 4 + PAGE_SIZE + 4);
}

#[test]
fn a_journal_shorter_than_its_header_is_malformed() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .database
            .decrypt_rollback_journal(&[0; JOURNAL_HEADER_LEN - 1]),
        Err(SidecarError::Malformed)
    );
}
