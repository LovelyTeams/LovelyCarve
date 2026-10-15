use super::*;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;

// Independent bitwise CRC and stored-block fixture writer; neither uses the
// production crc32fast or DEFLATE implementation.
fn crc(bytes: &[u8]) -> u32 {
    let mut value = !0_u32;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb88320 & 0_u32.wrapping_sub(value & 1));
        }
    }
    !value
}

fn stored(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= u16::MAX as usize);
    let length = payload.len() as u16;
    let mut bytes = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    bytes.push(1); // BFINAL=1, BTYPE=00; byte-aligned stored block.
    bytes.extend(length.to_le_bytes());
    bytes.extend((!length).to_le_bytes());
    bytes.extend(payload);
    bytes.extend(crc(payload).to_le_bytes());
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes
}

fn payload() -> Vec<u8> {
    let mut bytes = b"LovelyCarve independent gzip fixture.\n".repeat(4096);
    for _ in 0..16 {
        bytes.extend(0_u8..=255);
    }
    bytes
}

fn decode(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = GzipDecoder::new(bytes);
    let mut result = Vec::new();
    reader.read_to_end(&mut result)?;
    assert!(reader.is_complete());
    Ok(result)
}

#[test]
fn signature_probe_is_only_a_probe() {
    assert!(!check_signature(&[]));
    assert!(!check_signature(&[0x1f]));
    assert!(check_signature(&[0x1f, 0x8b]));
    assert!(decode(&[0x1f, 0x8b]).is_err());
}

#[test]
fn independent_stored_stream_and_geometry() {
    let bytes = stored(b"hello");
    let mut decoder = GzipDecoder::new(bytes.as_slice());
    assert!(decoder.members().is_empty());
    let mut result = Vec::new();
    decoder.read_to_end(&mut result).unwrap();
    assert_eq!(result, b"hello");
    assert_eq!(decoder.compressed_offset(), bytes.len() as u64);
    assert_eq!(decoder.uncompressed_size(), 5);
    let member = &decoder.members()[0];
    assert_eq!(
        (member.header_offset, member.compressed_data_offset),
        (0, 10)
    );
    assert_eq!(
        (member.compressed_data_size, member.trailer_offset),
        (10, 20)
    );
    assert_eq!(member.end_offset, bytes.len() as u64);
    assert_eq!(member.uncompressed_size, 5);
    assert_eq!(member.crc32, 0x3610a686);
    assert_eq!(member.isize, 5);
    assert_eq!(member.operating_system, 255);
    assert_eq!(member.modification_time, 0);
    assert_eq!(member.name_utf8(), None);
    assert_eq!(member.comment_utf16(), None);
}

#[test]
fn python_zlib_stored_fixed_and_dynamic_corpus() {
    let expected = payload();
    for bytes in [
        include_bytes!("../tests/data/stored.gz").as_slice(),
        include_bytes!("../tests/data/fixed.gz").as_slice(),
        include_bytes!("../tests/data/dynamic.gz").as_slice(),
        include_bytes!("../tests/data/optional.gz").as_slice(),
        include_bytes!("../tests/data/concatenated.gz").as_slice(),
    ] {
        assert_eq!(decode(bytes).unwrap(), expected);
    }
    assert_eq!(include_bytes!("../tests/data/stored.gz")[10] >> 1 & 3, 0);
    assert_eq!(include_bytes!("../tests/data/fixed.gz")[10] >> 1 & 3, 1);
    assert_eq!(include_bytes!("../tests/data/dynamic.gz")[10] >> 1 & 3, 2);
}

#[test]
fn all_optional_metadata_and_latin1() {
    let bytes = include_bytes!("../tests/data/optional.gz");
    let file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    let member = file.member(0).unwrap();
    assert_eq!(member.flags, 0x1f);
    assert!(member.is_text());
    assert_eq!(member.modification_time, 1_700_000_000);
    assert_eq!(member.operating_system, 3);
    assert_eq!(
        member.extra.as_deref(),
        Some(b"LC\x04\0\x01\x02\x03\x04".as_slice())
    );
    assert_eq!(member.name.as_deref(), Some(b"caf\xe9.log".as_slice()));
    assert_eq!(member.name_utf8().as_deref(), Some("café.log"));
    assert_eq!(
        member.name_utf16().unwrap(),
        "café.log".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(member.comment_utf8().as_deref(), Some("naïve fixture"));
    assert_eq!(
        member.comment_utf16().unwrap(),
        "naïve fixture".encode_utf16().collect::<Vec<_>>()
    );
    assert!(member.header_crc16.is_some());
    assert_eq!(member.uncompressed_size, payload().len() as u64);
    assert!(file.member(1).is_none());
}

#[test]
fn present_empty_metadata_differs_from_absent() {
    let mut bytes = stored(b"data");
    bytes[3] = 0x1c;
    bytes.splice(10..10, [0, 0, 0, 0]); // XLEN=0, FNAME="", FCOMMENT="".
    let file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    let member = &file.members()[0];
    assert_eq!(member.extra, Some(Vec::new()));
    assert_eq!(member.name_utf8(), Some(String::new()));
    assert_eq!(member.comment_utf16(), Some(Vec::new()));
}

#[test]
fn unknown_os_and_extra_flags_are_preserved() {
    let mut bytes = stored(b"data");
    bytes[8] = 222;
    bytes[9] = 19;
    let file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(file.members()[0].extra_flags, 222);
    assert_eq!(file.members()[0].operating_system, 19);
}

#[test]
fn members_include_empty_streams_and_precise_source_ranges() {
    let bytes = include_bytes!("../tests/data/concatenated.gz");
    let file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(file.members().len(), 5);
    let sizes: Vec<_> = file.members().iter().map(|m| m.uncompressed_size).collect();
    assert_eq!(sizes, [0, 80000, 0, payload().len() as u64 - 80000, 0]);
    let mut source = 0;
    let mut logical = 0;
    for member in file.members() {
        assert_eq!(member.header_offset, source);
        assert_eq!(member.uncompressed_offset, logical);
        assert!(member.compressed_data_size > 0);
        source = member.end_offset;
        logical += member.uncompressed_size;
    }
    assert_eq!(source, bytes.len() as u64);
    assert_eq!(logical, file.uncompressed_size());
}

struct Fragmented<'a> {
    bytes: &'a [u8],
    width: usize,
    interrupt: bool,
}
impl Read for Fragmented<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.interrupt {
            self.interrupt = false;
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let count = self.width.min(output.len()).min(self.bytes.len());
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[test]
fn fragmented_input_and_tiny_output() {
    for width in [1, 2, 3, 7, 16384] {
        let bytes = include_bytes!("../tests/data/concatenated.gz");
        let source = Fragmented {
            bytes,
            width,
            interrupt: true,
        };
        let mut decoder = GzipDecoder::new(source);
        let mut result = Vec::new();
        let mut output = [0; 3];
        loop {
            let count = decoder.read(&mut output).unwrap();
            if count == 0 {
                break;
            }
            result.extend_from_slice(&output[..count]);
        }
        assert_eq!(result, payload());
        assert!(decoder.is_complete());
    }
}

#[test]
fn seek_read_and_read_at_match_sequential_bytes() {
    let bytes = include_bytes!("../tests/data/concatenated.gz");
    let expected = payload();
    let mut file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    for offset in [80000, 79997, 0, expected.len() - 9, 65530, 1, 80123] {
        let mut buffer = [0; 37];
        let count = file.read_at(offset as u64, &mut buffer).unwrap();
        assert_eq!(&buffer[..count], &expected[offset..offset + count]);
        assert_eq!(file.offset(), (offset + count) as u64);
    }
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut complete = Vec::new();
    file.read_to_end(&mut complete).unwrap();
    assert_eq!(complete, expected);
    file.seek(SeekFrom::End(-5)).unwrap();
    let mut tail = [0; 5];
    file.read_exact(&mut tail).unwrap();
    assert_eq!(&tail, &expected[expected.len() - 5..]);
    file.seek(SeekFrom::Current(-5)).unwrap();
    assert_eq!(file.offset(), expected.len() as u64 - 5);
}

#[test]
fn seek_overflow_negative_and_past_eof() {
    let mut file = GzipFile::from_reader(Cursor::new(stored(b"abc"))).unwrap();
    assert!(file.seek(SeekFrom::Current(-1)).is_err());
    assert!(file.seek(SeekFrom::End(-4)).is_err());
    assert_eq!(file.offset(), 0);
    assert_eq!(file.seek(SeekFrom::Start(u64::MAX)).unwrap(), u64::MAX);
    assert!(file.seek(SeekFrom::Current(1)).is_err());
    assert_eq!(file.read(&mut [0; 1]).unwrap(), 0);
    file.seek(SeekFrom::Start(1)).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"bc");
}

#[test]
fn empty_member_is_valid_but_empty_file_is_not() {
    assert_eq!(decode(&stored(b"")).unwrap(), b"");
    assert!(decode(b"").is_err());
    let mut file = GzipFile::from_reader(Cursor::new(stored(b""))).unwrap();
    assert_eq!(file.uncompressed_size(), 0);
    assert_eq!(file.members().len(), 1);
    assert_eq!(file.read(&mut [0; 1]).unwrap(), 0);
}

#[test]
fn every_truncation_of_minimal_member_fails() {
    let bytes = stored(b"abc");
    for end in 0..bytes.len() {
        assert!(
            decode(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
        assert!(GzipFile::from_reader(Cursor::new(&bytes[..end])).is_err());
    }
}

#[test]
fn optional_header_truncations_fail() {
    let bytes = include_bytes!("../tests/data/optional.gz");
    let file = GzipFile::from_reader(Cursor::new(bytes)).unwrap();
    let body_offset = file.members()[0].compressed_data_offset as usize;
    for end in 0..=body_offset {
        assert!(decode(&bytes[..end]).is_err());
    }
}

#[test]
fn bad_crc_bad_isize_and_bad_deflate_fail() {
    let original = stored(b"payload");
    for offset in [10, 13, 15, original.len() - 8, original.len() - 4] {
        let mut bytes = original.clone();
        bytes[offset] ^= 0xff;
        assert!(decode(&bytes).is_err(), "accepted mutation at {offset}");
        assert!(GzipFile::from_reader(Cursor::new(bytes)).is_err());
    }
}

#[test]
fn bad_header_crc_fails() {
    let mut bytes = include_bytes!("../tests/data/optional.gz").to_vec();
    bytes[4] ^= 1;
    let error = decode(&bytes).unwrap_err();
    assert!(error.to_string().contains("header CRC"));
}

#[test]
fn invalid_signature_method_reserved_flags_and_trailing_junk_fail() {
    for (offset, byte) in [(0, 0), (1, 0), (2, 0), (2, 9), (3, 0x20), (3, 0x80)] {
        let mut bytes = stored(b"data");
        bytes[offset] = byte;
        assert!(decode(&bytes).is_err());
    }
    for junk in [b"\0".as_slice(), b"junk", b"\x1f\x8b"] {
        let mut bytes = stored(b"data");
        bytes.extend(junk);
        assert!(decode(&bytes).is_err());
    }
}

#[test]
fn corrupt_later_member_cannot_be_ignored() {
    let mut bytes = stored(b"good");
    let mut second = stored(b"bad");
    let offset = second.len() - 8;
    second[offset] ^= 1;
    bytes.extend(second);
    let mut decoder = GzipDecoder::new(bytes.as_slice());
    assert!(decoder.finish().is_err());
    assert_eq!(decoder.members().len(), 1);
    assert!(!decoder.is_complete());
    assert!(GzipFile::from_reader(Cursor::new(bytes)).is_err());
}

#[test]
fn output_limit_and_exact_boundary() {
    let bytes = stored(b"abcd");
    for limit in [0, 3, 4] {
        let options = Options {
            max_uncompressed_bytes: limit,
            ..Options::default()
        };
        let mut decoder = GzipDecoder::with_options(bytes.as_slice(), options);
        assert_eq!(decoder.finish().is_ok(), limit == 4);
    }
    let mut joined = stored(b"ab");
    joined.extend(stored(b"cd"));
    let options = Options {
        max_uncompressed_bytes: 3,
        ..Options::default()
    };
    assert!(GzipDecoder::with_options(joined.as_slice(), options)
        .finish()
        .is_err());
}

#[test]
fn compressed_limit_and_exact_boundary() {
    let bytes = stored(b"abcd");
    for limit in [0, bytes.len() as u64 - 1, bytes.len() as u64] {
        let options = Options {
            max_compressed_bytes: limit,
            ..Options::default()
        };
        let mut decoder = GzipDecoder::with_options(bytes.as_slice(), options);
        assert_eq!(decoder.finish().is_ok(), limit == bytes.len() as u64);
    }
}

#[test]
fn member_count_limit_counts_empty_members() {
    let mut bytes = stored(b"");
    bytes.extend(stored(b""));
    for limit in [0, 1, 2] {
        let options = Options {
            max_members: limit,
            ..Options::default()
        };
        assert_eq!(
            GzipDecoder::with_options(bytes.as_slice(), options)
                .finish()
                .is_ok(),
            limit == 2
        );
    }
}

#[test]
fn header_metadata_limits_and_exact_boundaries() {
    let bytes = stored(b"");
    for limit in [0, 9, 10] {
        let options = Options {
            max_header_bytes: limit,
            ..Options::default()
        };
        assert_eq!(
            GzipDecoder::with_options(bytes.as_slice(), options)
                .finish()
                .is_ok(),
            limit == 10
        );
    }
    let mut joined = bytes.clone();
    joined.extend(&bytes);
    for limit in [19, 20] {
        let options = Options {
            max_metadata_bytes: limit,
            ..Options::default()
        };
        assert_eq!(
            GzipDecoder::with_options(joined.as_slice(), options)
                .finish()
                .is_ok(),
            limit == 20
        );
    }
    let mut named = bytes;
    named[3] = 8;
    named.splice(10..10, b"long-name\0".iter().copied());
    let options = Options {
        max_header_bytes: 12,
        ..Options::default()
    };
    assert!(GzipDecoder::with_options(named.as_slice(), options)
        .finish()
        .is_err());
}

#[test]
fn oversized_extra_length_and_unterminated_name_fail() {
    let mut extra = stored(b"");
    extra[3] = 4;
    extra.splice(10..10, [255, 255]);
    let options = Options {
        max_header_bytes: 20,
        ..Options::default()
    };
    assert!(GzipDecoder::with_options(extra.as_slice(), options)
        .finish()
        .is_err());
    let mut name = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
    name.extend(b"no-terminator");
    assert!(decode(&name).is_err());
}

#[test]
fn cancellation_before_open_and_mid_stream_is_terminal() {
    let flag = Arc::new(AtomicBool::new(true));
    let options = Options::default().with_cancellation(flag.clone());
    let bytes = include_bytes!("../tests/data/dynamic.gz");
    assert!(GzipFile::from_reader_with_options(Cursor::new(bytes), options.clone()).is_err());
    flag.store(false, Ordering::Relaxed);
    let mut decoder = GzipDecoder::with_options(bytes.as_slice(), options);
    assert_eq!(decoder.read(&mut [0; 1]).unwrap(), 1);
    flag.store(true, Ordering::Relaxed);
    assert!(decoder
        .finish()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert!(decoder.has_failed());
    flag.store(false, Ordering::Relaxed);
    assert!(decoder.finish().is_err());
}

#[test]
fn zero_length_read_does_not_claim_validation() {
    let mut decoder = GzipDecoder::new(b"invalid".as_slice());
    assert_eq!(decoder.read(&mut []).unwrap(), 0);
    assert!(!decoder.is_complete());
    assert!(decoder.finish().is_err());
    assert_eq!(decoder.read(&mut []).unwrap(), 0);
    assert!(decoder.read(&mut [0; 1]).is_err());
}

#[test]
fn reader_start_is_rewound_for_seekable_files() {
    let bytes = stored(b"data");
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(15);
    let mut file = GzipFile::from_reader(cursor).unwrap();
    let mut result = Vec::new();
    file.read_to_end(&mut result).unwrap();
    assert_eq!(result, b"data");
}

#[test]
fn bounded_malformed_corpus_never_panics() {
    let original = stored(b"malformed corpus");
    for offset in 0..original.len() {
        for mask in [1, 0x80, 0xff] {
            let mut bytes = original.clone();
            bytes[offset] ^= mask;
            let options = Options {
                max_header_bytes: 128,
                max_uncompressed_bytes: 1024,
                max_members: 8,
                ..Options::default()
            };
            let result = GzipDecoder::with_options(bytes.as_slice(), options).finish();
            // Mutating metadata can be valid; every mutation must terminate.
            match result {
                Ok(()) => {}
                Err(error) => assert!(!error.to_string().is_empty()),
            }
        }
    }
}

#[test]
fn reader_probe_preserves_errors_and_advances_two_bytes() {
    assert_eq!(VERSION, "0.1.0");
    let mut cursor = Cursor::new(stored(b"test"));
    assert!(check_reader_signature(&mut cursor).unwrap());
    assert_eq!(cursor.position(), 2);
    assert!(!check_reader_signature(&mut b"xx".as_slice()).unwrap());
    assert_eq!(
        check_reader_signature(&mut b"x".as_slice())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn unicode_host_path_open_and_signature_are_read_only() -> std::io::Result<()> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("lovelycarve-gzipf-{}-{unique}", std::process::id()));
    std::fs::create_dir(&directory)?;
    let path = directory.join("只读检材.gz");
    let original = stored(b"test");
    let result = (|| {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::io::Write::write_all(&mut output, &original)?;
        drop(output);
        assert!(check_file_signature(&path)?);
        let mut file = GzipFile::open(&path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        assert_eq!(data, b"test");
        drop(file);
        assert_eq!(std::fs::read(&path)?, original);
        Ok::<_, std::io::Error>(())
    })();
    // Native, exact paths created by this test; no recursive filesystem cleanup.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::remove_dir(&directory)?;
    result
}

struct SharedSource(std::rc::Rc<std::cell::RefCell<Cursor<Vec<u8>>>>);
impl Read for SharedSource {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().read(output)
    }
}
impl Seek for SharedSource {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        self.0.borrow_mut().seek(from)
    }
}

#[test]
fn source_growth_and_truncation_after_open_fail_permanently() {
    for grow in [false, true] {
        let shared = std::rc::Rc::new(std::cell::RefCell::new(Cursor::new(stored(b"data"))));
        let mut file = GzipFile::from_reader(SharedSource(shared.clone())).unwrap();
        if grow {
            shared.borrow_mut().get_mut().extend(stored(b"extra"));
        } else {
            shared.borrow_mut().get_mut().truncate(12);
        }
        let mut data = Vec::new();
        assert!(file.read_to_end(&mut data).is_err());
        assert!(file.has_failed());
        assert!(file.read(&mut [0; 1]).is_err());
        assert!(file.seek(SeekFrom::Start(0)).is_err());
    }
}

#[test]
fn trailing_empty_member_corruption_after_open_is_not_ignored() {
    let mut bytes = stored(b"data");
    bytes.extend(stored(b""));
    let shared = std::rc::Rc::new(std::cell::RefCell::new(Cursor::new(bytes)));
    let mut file = GzipFile::from_reader(SharedSource(shared.clone())).unwrap();
    let mut source = shared.borrow_mut();
    let offset = source.get_ref().len() - 8;
    source.get_mut()[offset] ^= 1;
    drop(source);
    assert!(file.read_to_end(&mut Vec::new()).is_err());
    assert!(file.has_failed());
}

#[test]
fn source_io_error_is_not_reported_as_eof_or_success() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source failed",
            ))
        }
    }
    let mut decoder = GzipDecoder::new(Broken);
    let error = decoder.finish().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(decoder.has_failed());
    assert!(!decoder.is_complete());
}
