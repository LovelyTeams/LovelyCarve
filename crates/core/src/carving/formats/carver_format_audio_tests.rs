use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Clone)]
struct Bytes(Arc<Vec<u8>>);

impl CarveSource for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
        let start = (offset as usize).min(self.0.len());
        let length = destination.len().min(self.0.len() - start);
        destination[..length].copy_from_slice(&self.0[start..start + length]);
        Ok(length)
    }

    fn worker_handle(&self) -> Self {
        self.clone()
    }
}

fn carve_wav(bytes: &[u8]) -> Option<u64> {
    let source = Bytes(Arc::new(bytes.to_vec()));
    let cancellation = CancellationToken::new();
    let mut lookahead = Vec::new();
    let mut reader = CarveReader::new(
        &source,
        0,
        bytes.len() as u64,
        &bytes[..bytes.len().min(512)],
        &mut lookahead,
        &cancellation,
    );
    scan_wav(&mut reader)
        .ok()
        .flatten()
        .map(|result| result.size)
}

/// A WAVE file from `(id, payload)` chunks; the container length counts
/// every chunk and its pad byte, as a conforming writer's would.
fn wave(form: &[u8; 4], chunks: &[(&[u8; 4], Vec<u8>)], big_endian: bool) -> Vec<u8> {
    let encode = |value: u32| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut body = b"WAVE".to_vec();
    for (id, payload) in chunks {
        body.extend_from_slice(*id);
        body.extend_from_slice(&encode(payload.len() as u32));
        body.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = form.to_vec();
    out.extend_from_slice(&encode(body.len() as u32));
    out.extend_from_slice(&body);
    out
}

fn with_slack(bytes: &[u8]) -> Vec<u8> {
    let mut evidence = bytes.to_vec();
    evidence.extend_from_slice(&[0x7e; 600]);
    evidence
}

#[test]
fn container_length_covers_the_pad_byte_and_trailing_chunks() {
    let odd = wave(
        b"RIFF",
        &[(b"fmt ", vec![1; 16]), (b"data", vec![9; 45])],
        false,
    );
    assert_eq!(carve_wav(&with_slack(&odd)), Some(odd.len() as u64));

    let listed = wave(
        b"RIFF",
        &[
            (b"fmt ", vec![1; 16]),
            (b"data", vec![9; 64]),
            (b"LIST", b"INFOISFT\x05\0\0\0test\0\0".to_vec()),
        ],
        false,
    );
    assert_eq!(carve_wav(&with_slack(&listed)), Some(listed.len() as u64));
}

#[test]
fn float_rifx_and_rf64_variants_are_recognised() {
    let float = wave(
        b"RIFF",
        &[
            (b"fmt ", vec![3; 18]),
            (b"fact", vec![0; 4]),
            (b"data", vec![9; 32]),
        ],
        false,
    );
    assert_eq!(carve_wav(&with_slack(&float)), Some(float.len() as u64));

    let rifx = wave(
        b"RIFX",
        &[(b"fmt ", vec![1; 16]), (b"data", vec![9; 32])],
        true,
    );
    assert_eq!(carve_wav(&with_slack(&rifx)), Some(rifx.len() as u64));

    // RF64: placeholder 32-bit sizes, real ones in ds64.
    let mut ds64 = Vec::new();
    let total = 4 + (8 + 28) + (8 + 16) + (8 + 20);
    ds64.extend_from_slice(&(total as u64).to_le_bytes());
    ds64.extend_from_slice(&20u64.to_le_bytes());
    ds64.extend_from_slice(&[0; 12]);
    let mut rf64 = wave(
        b"RF64",
        &[
            (b"ds64", ds64),
            (b"fmt ", vec![1; 16]),
            (b"data", vec![9; 20]),
        ],
        false,
    );
    rf64[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let data_size = rf64.len() - 20 - 4;
    rf64[data_size..data_size + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(carve_wav(&with_slack(&rf64)), Some(rf64.len() as u64));
}

#[test]
fn header_without_audio_is_rejected_without_walking_the_evidence() {
    let mut empty = b"RIFF\x24\0\0\0WAVE".to_vec();
    empty.extend_from_slice(&[0u8; 64 * 1024]);
    assert_eq!(carve_wav(&empty), None);
}
