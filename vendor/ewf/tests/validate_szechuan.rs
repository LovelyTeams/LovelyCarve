use std::io::{Read, Seek, SeekFrom};

/// Resolve an E01 in the shared corpus dir named by `$EWF_TEST_CORPUS`.
///
/// The large third-party E01 corpora (Szechuan Sauce, MaxPowers, PC-MUS-001) are
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
fn validate_szechuan_sauce() {
    let Some(path) = corpus_e01("20200918_0417_DESKTOP-SDN1RPT.E01") else {
        return;
    };

    let mut reader = ewf::EwfReader::open(&path).unwrap();

    assert_eq!(
        reader.total_size(),
        16_106_127_360,
        "Image size mismatch vs libewf"
    );

    // MBR
    let mut mbr = [0u8; 512];
    reader.read_exact(&mut mbr).unwrap();
    assert_eq!(mbr[510], 0x55);
    assert_eq!(mbr[511], 0xAA);

    // GPT header at LBA 1
    reader.seek(SeekFrom::Start(512)).unwrap();
    let mut gpt = [0u8; 92];
    reader.read_exact(&mut gpt).unwrap();
    let gpt_sig = std::str::from_utf8(&gpt[0..8]).unwrap_or("???");
    assert!(gpt_sig.starts_with("EFI PART"), "GPT signature not found");
}

/// Full-media MD5: read every byte through `EwfReader`, hash it, compare against
/// the hash produced by libewf (pyewf) and The Sleuth Kit (`img_cat`).
///
/// libewf full-media MD5: bcd3aef20406df00585341f0c743a1ce
/// Sleuth Kit full-media MD5: bcd3aef20406df00585341f0c743a1ce
#[test]
fn szechuan_full_media_md5() {
    use md5::{Digest, Md5};

    let Some(path) = corpus_e01("20200918_0417_DESKTOP-SDN1RPT.E01") else {
        return;
    };

    let mut reader = ewf::EwfReader::open(&path).unwrap();
    reader.seek(SeekFrom::Start(0)).unwrap();

    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024]; // 1 MB chunks
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
    eprintln!("Szechuan Sauce full-media MD5: {hash}");
    eprintln!("Bytes hashed: {total} / {}", reader.total_size());

    assert_eq!(total, reader.total_size(), "Did not read entire media");
    assert_eq!(
        hash, "bcd3aef20406df00585341f0c743a1ce",
        "Full-media MD5 mismatch vs libewf/Sleuth Kit"
    );
}
