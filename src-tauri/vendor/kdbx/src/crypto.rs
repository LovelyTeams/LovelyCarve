//! Payload ciphers (AES-256-CBC, Twofish-CBC, ChaCha20) and the inner
//! random stream that protects `Protected="True"` values.

use cipher::consts::U16;
use cipher::generic_array::GenericArray;
use cipher::{BlockDecrypt, BlockSizeUser, KeyInit, KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::bytes::{fixed, hex};
use crate::error::{Error, Result};
use crate::header::{Cipher, InnerStreamAlgorithm};
use crate::salsa20::Salsa20;

/// The fixed Salsa20 nonce of the KDBX 3 inner stream.
const SALSA20_NONCE: [u8; 8] = [0xe8, 0x30, 0x09, 0x4b, 0x97, 0x20, 0x5d, 0x2a];

/// Decrypt `data` without removing padding.
pub(crate) fn decrypt(
    cipher: Cipher,
    key: &[u8; 32],
    iv: &[u8],
    data: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    match cipher {
        Cipher::Aes256 => cbc::<aes::Aes256>(key, iv, data),
        Cipher::Twofish => cbc::<twofish::Twofish>(key, iv, data),
        Cipher::ChaCha20 => chacha20(key, iv, data),
        Cipher::Unknown(uuid) => Err(Error::Unsupported(format!("cipher {}", hex(&uuid)))),
    }
}

/// Remove PKCS#7 padding left by the CBC ciphers.
pub(crate) fn unpad(cipher: Cipher, data: &mut Zeroizing<Vec<u8>>) -> Result<()> {
    match cipher {
        Cipher::Aes256 | Cipher::Twofish => {
            let padding = usize::from(*data.last().ok_or(Error::Truncated("payload"))?);
            let valid = (1..=16).contains(&padding)
                && padding <= data.len()
                && data[data.len() - padding..]
                    .iter()
                    .all(|&byte| usize::from(byte) == padding);
            if !valid {
                return Err(Error::Malformed("payload padding".into()));
            }
            let length = data.len() - padding;
            data.truncate(length);
            Ok(())
        }
        Cipher::ChaCha20 | Cipher::Unknown(_) => Ok(()),
    }
}

fn cbc<C>(key: &[u8; 32], iv: &[u8], data: &[u8]) -> Result<Zeroizing<Vec<u8>>>
where
    C: BlockDecrypt + BlockSizeUser<BlockSize = U16> + KeyInit,
{
    let iv: [u8; 16] = fixed(iv).ok_or_else(|| {
        Error::Malformed(format!(
            "EncryptionIV of {} bytes for a CBC cipher",
            iv.len()
        ))
    })?;
    if data.len() % 16 != 0 {
        return Err(Error::Malformed(
            "payload is not a whole number of cipher blocks".into(),
        ));
    }
    let cipher = C::new_from_slice(key).map_err(|_| Error::Malformed("cipher key".into()))?;
    let mut out = Zeroizing::new(data.to_vec());
    for (index, block) in out.chunks_exact_mut(16).enumerate() {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        let previous = if index == 0 {
            &iv[..]
        } else {
            &data[(index - 1) * 16..index * 16]
        };
        for (byte, chained) in block.iter_mut().zip(previous) {
            *byte ^= chained;
        }
    }
    Ok(out)
}

fn chacha20(key: &[u8; 32], iv: &[u8], data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let nonce: [u8; 12] = fixed(iv).ok_or_else(|| {
        Error::Malformed(format!("EncryptionIV of {} bytes for ChaCha20", iv.len()))
    })?;
    let mut out = Zeroizing::new(data.to_vec());
    let mut cipher = chacha20::ChaCha20::new(
        GenericArray::from_slice(key),
        GenericArray::from_slice(&nonce),
    );
    cipher
        .try_apply_keystream(&mut out)
        .map_err(|_| Error::Limit("ChaCha20 keystream exhausted".into()))?;
    Ok(out)
}

/// The keystream protected values are XORed with, consumed in document
/// order across the whole XML.
pub(crate) enum InnerStream {
    /// Protected values are only base64 encoded.
    None,
    Salsa20(Salsa20),
    ChaCha20(Box<chacha20::ChaCha20>),
    /// An algorithm this reader does not implement: protected values stay
    /// encrypted.
    Unsupported(InnerStreamAlgorithm),
}

impl InnerStream {
    pub(crate) fn new(algorithm: InnerStreamAlgorithm, key: &[u8]) -> Self {
        match algorithm {
            InnerStreamAlgorithm::None => Self::None,
            InnerStreamAlgorithm::Salsa20 => {
                let key = Zeroizing::new(<[u8; 32]>::from(Sha256::digest(key)));
                Self::Salsa20(Salsa20::new(&key, &SALSA20_NONCE))
            }
            InnerStreamAlgorithm::ChaCha20 => {
                let mut hash = Zeroizing::new([0u8; 64]);
                hash.copy_from_slice(&Sha512::digest(key));
                Self::ChaCha20(Box::new(chacha20::ChaCha20::new(
                    GenericArray::from_slice(&hash[..32]),
                    GenericArray::from_slice(&hash[32..44]),
                )))
            }
            other @ (InnerStreamAlgorithm::ArcFourVariant | InnerStreamAlgorithm::Unknown(_)) => {
                Self::Unsupported(other)
            }
        }
    }

    /// Remove the stream from `data`. `Ok(false)` when the algorithm is not
    /// implemented and `data` is left as stored.
    pub(crate) fn apply(&mut self, data: &mut [u8]) -> Result<bool> {
        match self {
            Self::None => Ok(true),
            Self::Salsa20(stream) => {
                stream.apply(data);
                Ok(true)
            }
            Self::ChaCha20(stream) => {
                stream
                    .try_apply_keystream(data)
                    .map_err(|_| Error::Limit("inner stream exhausted".into()))?;
                Ok(true)
            }
            Self::Unsupported(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbc_padding_is_checked() {
        let mut good = Zeroizing::new(vec![1, 2, 3, 4, 4, 4, 4]);
        unpad(Cipher::Aes256, &mut good).unwrap();
        assert_eq!(&good[..], [1, 2, 3]);
        let mut bad = Zeroizing::new(vec![1, 2, 3, 2, 4]);
        assert!(unpad(Cipher::Twofish, &mut bad).is_err());
        let mut zero = Zeroizing::new(vec![1, 0]);
        assert!(unpad(Cipher::Aes256, &mut zero).is_err());
        let mut stream = Zeroizing::new(vec![1, 2, 0]);
        unpad(Cipher::ChaCha20, &mut stream).unwrap();
        assert_eq!(stream.len(), 3);
    }

    #[test]
    fn rejects_iv_lengths_the_cipher_cannot_use() {
        let key = [0; 32];
        assert!(decrypt(Cipher::Aes256, &key, &[0; 12], &[0; 16]).is_err());
        assert!(decrypt(Cipher::ChaCha20, &key, &[0; 16], &[0; 16]).is_err());
        assert!(decrypt(Cipher::Aes256, &key, &[0; 16], &[0; 15]).is_err());
    }
}
