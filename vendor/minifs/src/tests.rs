use super::*;
use sha2::{Digest, Sha256};

const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/synthetic.minifs");

fn set_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn minifs_shared_chunks_paths_ranges_and_empty_files() {
    let mut reader = Reader::open(FIXTURE, Limits::default()).unwrap();
    assert_eq!(reader.index().files.len(), 4);
    assert_eq!(reader.index().chunk_count(), 2);
    assert_eq!(reader.index().image_length(), FIXTURE.len() as u64);
    assert_eq!(reader.index().children("").unwrap().len(), 3);
    assert!(reader.index().children("/web/js/app.js").is_err());
    let payload = reader.read_range("/web/js/app.js", 0, usize::MAX).unwrap();
    assert_eq!(
        payload,
        [
            ((0..=255).collect::<Vec<u8>>()).repeat(289),
            b"MINIFS test\n".to_vec()
        ]
        .concat()
    );
    for (offset, count) in [
        (0, 0),
        (0, 19),
        (4090, 100),
        (65530, 20),
        (payload.len() - 3, 30),
        (payload.len(), 9),
    ] {
        assert_eq!(
            reader
                .read_range("/web/js/app.js", offset as u64, count)
                .unwrap(),
            payload[offset..offset.saturating_add(count).min(payload.len())]
        );
    }
    assert!(reader.read_range("/web/js/app.js", u64::MAX, 1).is_err());
    assert!(reader
        .read_range("/web/js/empty", 0, 100)
        .unwrap()
        .is_empty());
    assert_eq!(reader.read_range("/hello.txt", 0, 100).unwrap(), b"hello\n");
    assert_eq!(
        reader.read_range("/unicode/测试.txt", 0, 100).unwrap(),
        "你好 MINIFS\n".as_bytes()
    );
    assert!(reader.read_range("/web", 0, 1).is_err());
}

#[test]
fn minifs_rejects_metadata_corruption_and_resource_abuse() {
    let names_length = word(FIXTURE, 28).unwrap() as usize;
    let table = 32 + names_length;
    let chunk_table = table + 4 * 20;
    for (offset, value) in [
        (16, 2),
        (20, 0),
        (20, u32::MAX),
        (28, u32::MAX),
        (table, u32::MAX),
        (table, 1),
        (table + 8, 20),
        (table + 12, u32::MAX),
        (table + 16, u32::MAX),
        (chunk_table, 1),
        (chunk_table + 4, u32::MAX),
        (chunk_table + 8, u32::MAX),
        (chunk_table + 12, 0),
    ] {
        let mut bytes = FIXTURE.to_vec();
        set_word(&mut bytes, offset, value);
        assert!(
            Reader::open(bytes.as_slice(), Limits::default()).is_err(),
            "offset {offset}"
        );
    }
    for length in [0, 16, 31, table - 1, chunk_table, FIXTURE.len() - 1] {
        assert!(Reader::open(&FIXTURE[..length], Limits::default()).is_err());
    }
    for path in ["/../xx", "/./xxx", "//xxxx", "/a\\xxx"] {
        let mut bytes = FIXTURE.to_vec();
        bytes[32..38].copy_from_slice(path.as_bytes());
        assert!(Reader::open(bytes.as_slice(), Limits::default()).is_err());
    }
    let mut bytes = FIXTURE.to_vec();
    bytes[32] = 255;
    assert!(Reader::open(bytes.as_slice(), Limits::default()).is_err());
    let mut bytes = FIXTURE.to_vec();
    let first = bytes[table..table + 8].to_vec();
    bytes[table + 20..table + 28].copy_from_slice(&first);
    assert!(Reader::open(bytes.as_slice(), Limits::default()).is_err());
    let limits = Limits {
        path_depth: 1,
        ..Limits::default()
    };
    assert!(Reader::open(FIXTURE, limits).is_err());
}

#[test]
fn minifs_strict_lzma_and_cancellation() {
    let index = Index::open(&FIXTURE, Limits::default()).unwrap();
    let chunk = &index.chunks[0];
    let start = index.data_offset as usize;
    let packed = &FIXTURE[start..start + chunk.packed];
    for missing in 1..=8 {
        assert!(codec::decode(
            &packed[..packed.len() - missing],
            chunk.unpacked,
            16 << 20,
            &mut || Ok(())
        )
        .is_err());
    }
    let mut extra = packed.to_vec();
    extra.push(0);
    assert!(codec::decode(&extra, chunk.unpacked, 16 << 20, &mut || Ok(())).is_err());
    let mut corrupt = packed.to_vec();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(codec::decode(&corrupt, chunk.unpacked, 16 << 20, &mut || Ok(())).is_err());
    assert!(codec::decode(packed, chunk.unpacked - 1, 16 << 20, &mut || Ok(())).is_err());
    assert!(codec::decode(packed, chunk.unpacked + 1, 16 << 20, &mut || Ok(())).is_err());
    let mut bytes = packed.to_vec();
    bytes[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(codec::decode(&bytes, chunk.unpacked, 16 << 20, &mut || Ok(())).is_err());
    let mut reader = Reader::open(FIXTURE, Limits::default()).unwrap();
    let mut checks = 0;
    assert!(reader
        .read_range_with("/web/js/app.js", 0, 100, || {
            checks += 1;
            ensure!(checks < 3, "cancelled");
            Ok(())
        })
        .is_err());
    assert!(reader.cache.is_none());
    assert_eq!(reader.read_range("/hello.txt", 0, 100).unwrap(), b"hello\n");
}

#[test]
#[ignore = "requires MINIFS_IMAGE and MINIFS_ORACLE generated by scripts/minifs-fixtures.py"]
fn minifs_external_python_oracle() {
    let bytes = std::fs::read(std::env::var_os("MINIFS_IMAGE").expect("MINIFS_IMAGE")).unwrap();
    let manifest =
        std::fs::read_to_string(std::env::var_os("MINIFS_ORACLE").expect("MINIFS_ORACLE")).unwrap();
    let mut reader = Reader::open(bytes.as_slice(), Limits::default()).unwrap();
    assert_eq!(reader.index().files.len(), manifest.lines().count());
    for line in manifest.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let path = fields[0];
        let length: usize = fields[1].parse().unwrap();
        let bytes = reader.read_range(path, 0, usize::MAX).unwrap();
        assert_eq!(bytes.len(), length, "{path}");
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), fields[2], "{path}");
        for offset in [0, length / 2, length.saturating_sub(7), length] {
            assert_eq!(
                reader.read_range(path, offset as u64, 101).unwrap(),
                bytes[offset..(offset + 101).min(length)],
                "{path}"
            );
        }
    }
    println!(
        "MINIFS oracle: {} files, {} chunks",
        reader.index().files.len(),
        reader.index().chunk_count()
    );
}
