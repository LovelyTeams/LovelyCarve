//! Optional external oracle. The raw stream must come from an independent tool.
use phdi::PhdiImage;
use std::fs::File;
use std::io::Read;

#[test]
#[ignore = "requires PHDI_ORACLE_DESCRIPTOR and PHDI_ORACLE_RAW"]
fn external_logical_bytes_match_full_stream_and_random_ranges() {
    let descriptor = std::env::var_os("PHDI_ORACLE_DESCRIPTOR").expect("PHDI_ORACLE_DESCRIPTOR");
    let raw = std::env::var_os("PHDI_ORACLE_RAW").expect("PHDI_ORACLE_RAW");
    let mut reader = PhdiImage::open(descriptor).expect("open external image");
    let mut raw = File::open(raw).expect("open independent raw oracle");
    assert_eq!(reader.media_size(), raw.metadata().unwrap().len());
    let mut expected = [0; 65537];
    let mut actual = [0; 65537];
    let mut offset = 0;
    while offset < reader.media_size() {
        let count = (reader.media_size() - offset).min(expected.len() as u64) as usize;
        raw.read_exact(&mut expected[..count]).unwrap();
        assert_eq!(reader.read(&mut actual[..count]).unwrap(), count);
        assert_eq!(
            &actual[..count],
            &expected[..count],
            "sequential offset {offset}"
        );
        assert_eq!(reader.read_at(offset, &mut actual[..count]).unwrap(), count);
        assert_eq!(
            &actual[..count],
            &expected[..count],
            "positioned offset {offset}"
        );
        let start = count / 3;
        assert_eq!(
            reader
                .read_at(offset + start as u64, &mut actual[..count - start])
                .unwrap(),
            count - start
        );
        assert_eq!(&actual[..count - start], &expected[start..count]);
        offset += count as u64;
    }
    assert_eq!(reader.read(&mut actual).unwrap(), 0);
}
