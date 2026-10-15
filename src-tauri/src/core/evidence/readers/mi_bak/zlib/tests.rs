use super::*;
use flate2::{write::ZlibEncoder, Compression};
use std::io::{BufReader, Cursor, Write};

#[test]
fn android_backup_zlib_handles_tiny_output_and_requires_valid_trailer() {
    let plain = vec![b'x'; 1 << 16];
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&plain).unwrap();
    let compressed = encoder.finish().unwrap();
    for capacity in [1, 2, 8192] {
        let input = BufReader::with_capacity(capacity, Cursor::new(&compressed));
        let mut decoder = ValidatedZlib::new(input);
        let mut actual = Vec::new();
        let mut byte = [0; 1];
        while decoder.read(&mut byte).unwrap() != 0 {
            actual.push(byte[0]);
        }
        assert_eq!(actual, plain);
    }
    for end in [0, 1, compressed.len() - 4, compressed.len() - 1] {
        let mut decoder = ValidatedZlib::new(Cursor::new(&compressed[..end]));
        assert!(decoder.read_to_end(&mut Vec::new()).is_err());
    }
}
