//! Decrypt the payload. KDBX 3 checks the key against the stream start bytes
//! and reads SHA-256 hashed blocks; KDBX 4 checks the header SHA-256, then
//! the key against the header HMAC, reads HMAC blocks and an inner header.

use std::io::Read;

use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::blocks::{self, HEADER_BLOCK};
use crate::bytes::{fixed, hex, Cursor};
use crate::crypto;
use crate::error::{Error, Result};
use crate::header::{Cipher, Compression, Header, InnerStreamAlgorithm};
use crate::key::Key;
use crate::model::Binary;
use crate::Limits;

pub(crate) struct Plaintext {
    pub(crate) transformed_key: Zeroizing<[u8; 32]>,
    pub(crate) xml: Zeroizing<Vec<u8>>,
    pub(crate) inner_stream: InnerStreamAlgorithm,
    pub(crate) inner_stream_key: Zeroizing<Vec<u8>>,
    /// KDBX 4: the inner header binaries, in order.
    pub(crate) binaries: Vec<Binary>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn decrypt(
    header: &Header,
    data: &[u8],
    key: &Key,
    limits: &Limits,
) -> Result<Plaintext> {
    if let Cipher::Unknown(uuid) = header.cipher {
        return Err(Error::Unsupported(format!("cipher {}", hex(&uuid))));
    }
    if let Compression::Unknown(flags) = header.compression {
        return Err(Error::Unsupported(format!("compression {flags}")));
    }
    let transformed_key = match key.transformed() {
        Some(transformed) => Zeroizing::new(*transformed),
        None => header.kdf.transform(&key.composite(), limits)?,
    };
    let master_key = Zeroizing::new(<[u8; 32]>::from(
        Sha256::new()
            .chain_update(&header.master_seed)
            .chain_update(&transformed_key[..])
            .finalize(),
    ));
    if header.version.is_kdbx4() {
        kdbx4(header, data, transformed_key, &master_key, limits)
    } else {
        kdbx3(header, data, transformed_key, &master_key, limits)
    }
}

fn kdbx3(
    header: &Header,
    data: &[u8],
    transformed_key: Zeroizing<[u8; 32]>,
    master_key: &[u8; 32],
    limits: &Limits,
) -> Result<Plaintext> {
    let mut warnings = Vec::new();
    let encrypted = data
        .get(header.payload_offset()..)
        .filter(|encrypted| encrypted.len() >= 32)
        .ok_or(Error::Truncated("payload"))?;
    let expected = header
        .stream_start_bytes
        .as_deref()
        .ok_or_else(|| Error::Malformed("header without StreamStartBytes".into()))?;
    // The first 32 decrypted bytes check the key before the whole payload
    // is decrypted.
    let start = crypto::decrypt(
        header.cipher,
        master_key,
        &header.encryption_iv,
        &encrypted[..32],
    )?;
    if start[..] != *expected {
        return Err(Error::InvalidCredentials);
    }
    let mut plain = crypto::decrypt(header.cipher, master_key, &header.encryption_iv, encrypted)?;
    crypto::unpad(header.cipher, &mut plain)?;
    let (payload, trailing) = blocks::read_hashed_blocks(&plain[32..], limits.max_payload)?;
    if trailing > 0 {
        warnings.push(format!("{trailing} bytes after the final payload block"));
    }
    let xml = decompress(header.compression, payload, limits)?;
    Ok(Plaintext {
        transformed_key,
        xml,
        inner_stream: header
            .inner_stream
            .ok_or_else(|| Error::Malformed("header without InnerRandomStreamID".into()))?,
        inner_stream_key: Zeroizing::new(header.protected_stream_key.clone().unwrap_or_default()),
        binaries: Vec::new(),
        warnings,
    })
}

fn kdbx4(
    header: &Header,
    data: &[u8],
    transformed_key: Zeroizing<[u8; 32]>,
    master_key: &[u8; 32],
    limits: &Limits,
) -> Result<Plaintext> {
    let mut warnings = Vec::new();
    if header.sha256_matches() != Some(true) {
        return Err(Error::HeaderChecksum);
    }
    let mut hmac_key = Zeroizing::new([0u8; 64]);
    hmac_key.copy_from_slice(
        &Sha512::new()
            .chain_update(&header.master_seed)
            .chain_update(&transformed_key[..])
            .chain_update([1])
            .finalize(),
    );
    let stored_hmac = header.stored_hmac.ok_or(Error::Truncated("header HMAC"))?;
    if !blocks::hmac_matches(
        HEADER_BLOCK,
        &hmac_key,
        &[&data[..header.length]],
        &stored_hmac,
    ) {
        return Err(Error::InvalidCredentials);
    }
    let encrypted = data
        .get(header.payload_offset()..)
        .ok_or(Error::Truncated("payload"))?;
    let (ciphertext, trailing) =
        blocks::read_hmac_blocks(encrypted, &hmac_key, limits.max_payload)?;
    if trailing > 0 {
        warnings.push(format!("{trailing} bytes after the final payload block"));
    }
    let mut plain = crypto::decrypt(
        header.cipher,
        master_key,
        &header.encryption_iv,
        &ciphertext,
    )?;
    crypto::unpad(header.cipher, &mut plain)?;
    let payload = decompress(header.compression, plain, limits)?;

    let mut cursor = Cursor::new(&payload);
    let mut inner_stream = None;
    let mut inner_stream_key = None;
    let mut binaries = Vec::new();
    loop {
        let kind = cursor.u8("inner header")?;
        let size = cursor.length_i32("inner header")?;
        let field = cursor.take(size, "inner header")?;
        match kind {
            0 => break,
            1 => {
                let id = fixed::<4>(field)
                    .ok_or_else(|| Error::Malformed("inner header stream id".into()))?;
                inner_stream = Some(InnerStreamAlgorithm::from_id(u32::from_le_bytes(id)));
            }
            2 => inner_stream_key = Some(Zeroizing::new(field.to_vec())),
            3 => {
                let (flags, content) = field
                    .split_first()
                    .ok_or_else(|| Error::Malformed("inner header binary without flags".into()))?;
                binaries.push(Binary {
                    data: content.to_vec(),
                    protected: flags & 1 != 0,
                    compressed: false,
                    unreadable: None,
                });
            }
            other => warnings.push(format!("inner header field {other} ignored")),
        }
    }
    let xml = Zeroizing::new(cursor.remaining().to_vec());
    Ok(Plaintext {
        transformed_key,
        xml,
        inner_stream: inner_stream
            .ok_or_else(|| Error::Malformed("inner header without InnerRandomStreamID".into()))?,
        inner_stream_key: inner_stream_key
            .ok_or_else(|| Error::Malformed("inner header without InnerRandomStreamKey".into()))?,
        binaries,
        warnings,
    })
}

fn decompress(
    compression: Compression,
    data: Zeroizing<Vec<u8>>,
    limits: &Limits,
) -> Result<Zeroizing<Vec<u8>>> {
    match compression {
        Compression::None => Ok(data),
        Compression::Gzip => {
            let mut out = Zeroizing::new(Vec::new());
            let limit = limits.max_payload;
            flate2::read::GzDecoder::new(&data[..])
                .take(limit as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|error| Error::Malformed(format!("payload GZip stream: {error}")))?;
            if out.len() > limit {
                return Err(Error::Limit(format!(
                    "decompressed payload larger than {limit} bytes"
                )));
            }
            Ok(out)
        }
        Compression::Unknown(flags) => Err(Error::Unsupported(format!("compression {flags}"))),
    }
}
