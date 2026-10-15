use super::*;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;

fn gzip(chunks: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in chunks {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(chunk).unwrap();
        bytes.extend(encoder.finish().unwrap());
    }
    bytes
}

fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap()
}

#[test]
fn gzip_concatenated_layer_composes_and_streams_all_files() {
    let layer = tar(&[("first.txt", b"first"), ("second.txt", b"second")]);
    let compressed = gzip(&[b"", &layer[..600], &layer[600..], b""]);
    let manifest = br#"[{"Config":"config.json","RepoTags":["gzip-fixture:latest"],"Layers":["layer.tar.gz"]}]"#;
    let outer = tar(&[
        ("manifest.json", manifest),
        ("config.json", b"{}"),
        ("layer.tar.gz", &compressed),
    ]);
    let mut file = tempfile::Builder::new().suffix(".tar").tempfile().unwrap();
    file.write_all(&outer).unwrap();
    file.flush().unwrap();
    let image = DockerImage::open(file.path()).unwrap();
    assert_eq!(image.read("first.txt", usize::MAX).unwrap(), b"first");
    assert_eq!(image.read("second.txt", usize::MAX).unwrap(), b"second");
    assert_eq!(image.layers()[0].compressed_size, compressed.len() as u64);
    assert_eq!(image.layers()[0].uncompressed_size, layer.len() as u64);
    let mut output = Vec::new();
    image
        .stream("second.txt", 2, |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(output, b"second");
}

#[test]
fn gzip_late_member_corruption_cannot_be_ignored_by_layer_composition() {
    let layer = tar(&[("valid.txt", b"first")]);
    let mut tail = gzip(&[b""]);
    let offset = tail.len() - 8;
    tail[offset] ^= 1;
    let mut compressed = gzip(&[&layer]);
    compressed.extend(tail);
    let manifest = br#"[{"Config":"config.json","Layers":["layer.tar.gz"]}]"#;
    let outer = tar(&[
        ("manifest.json", manifest),
        ("config.json", b"{}"),
        ("layer.tar.gz", &compressed),
    ]);
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&outer).unwrap();
    file.flush().unwrap();
    let error = DockerImage::open(file.path()).unwrap_err();
    assert!(format!("{error:#}").contains("CRC"));
}

#[test]
fn gzip_layer_decoder_drains_independent_corpus_and_rejects_invalid_end() {
    let bytes = include_bytes!("../../../../../vendor/gzipf/tests/data/concatenated.gz");
    let output = decode_gzip(bytes).unwrap();
    let mut expected_bytes = b"LovelyCarve independent gzip fixture.\n".repeat(4096);
    for _ in 0..16 {
        expected_bytes.extend(0_u8..=255);
    }
    assert_eq!(output, expected_bytes);
    let mut invalid = bytes.to_vec();
    invalid.extend([0]);
    assert!(decode_gzip(&invalid).is_err());
    assert!(decode_gzip(&bytes[..bytes.len() - 1]).is_err());
}
