use std::io::{Read, Seek, SeekFrom};

/// Resolve an E01 in the shared corpus dir named by `$EWF_TEST_CORPUS`.
///
/// The large third-party E01 corpora (PC-MUS-001, MaxPowers, Szechuan Sauce) are
/// gitignored and downloaded on demand; point `EWF_TEST_CORPUS` at the directory
/// that holds them. Returns `None` (with a skip note) when the env var is unset
/// or the file is absent, so the test skips cleanly instead of failing.
fn corpus_e01(name: &str) -> Option<std::path::PathBuf> {
    let Some(dir) = std::env::var_os("EWF_TEST_CORPUS") else {
        eprintln!("skipping {name}: EWF_TEST_CORPUS unset");
        return None;
    };
    let path = std::path::Path::new(&dir).join(name);
    if !path.exists() {
        eprintln!("skipping {name}: {} not found", path.display());
        return None;
    }
    Some(path)
}

#[test]
fn validate_pcmus() {
    let Some(path) = corpus_e01("PC-MUS-001.E01") else {
        return;
    };

    let mut reader = ewf::EwfReader::open(&path).unwrap();

    assert_eq!(
        reader.total_size(),
        256_060_514_304,
        "Media size mismatch vs ewfinfo"
    );

    // MBR
    let mut mbr = [0u8; 512];
    reader.read_exact(&mut mbr).unwrap();
    assert_eq!(mbr[510], 0x55);
    assert_eq!(mbr[511], 0xAA);
}

/// Full-media MD5: read every byte through `EwfReader`, hash it, compare against
/// the hash produced by libewf (ewfexport) and The Sleuth Kit (`img_cat`).
///
/// libewf full-media MD5:     522df9db8289f4f8132cf47b14d20fb8
/// Sleuth Kit full-media MD5: 522df9db8289f4f8132cf47b14d20fb8
#[test]
fn pcmus_full_media_md5() {
    use md5::{Digest, Md5};

    let Some(path) = corpus_e01("PC-MUS-001.E01") else {
        return;
    };

    let mut reader = ewf::EwfReader::open(&path).unwrap();
    reader.seek(SeekFrom::Start(0)).unwrap();

    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut total = 0u64;

    loop {
        let n = reader.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }

    let hash = format!("{:x}", hasher.finalize());
    eprintln!("PC-MUS full-media MD5: {hash}");
    eprintln!("Bytes hashed: {total} / {}", reader.total_size());

    assert_eq!(total, reader.total_size(), "Did not read entire media");
    assert_eq!(
        hash, "522df9db8289f4f8132cf47b14d20fb8",
        "Full-media MD5 mismatch vs libewf/Sleuth Kit"
    );
}
