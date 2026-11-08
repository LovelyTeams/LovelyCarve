//! dm-crypt sector ciphers. Cipher primitives and CBC come from RustCrypto.
//! Key schedules are constructed once, rather than once per payload sector.

use aes::cipher::{
    block_padding::NoPadding, BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockSizeUser,
    InnerIvInit, KeyInit,
};
use aes::{Aes128, Aes192, Aes256};
use blowfish::Blowfish;
use rc4::{consts::U256, Rc4, StreamCipher};
use serpent::Serpent;
use sha2::Digest;
use xts_mode::Xts128;
use zeroize::Zeroizing;

use crate::{LuksError, Result};

fn unsupported(what: &'static str, value: impl Into<String>) -> LuksError {
    LuksError::Unsupported {
        what,
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    #[test]
    fn independent_large_sector_vectors_cover_plain_wrap_and_benbi_block_size() {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/compatibility.json")).unwrap();
        for vector in corpus["vectors"].as_array().unwrap() {
            let key: Vec<u8> = vector["key"]
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            let mut bytes = base64::engine::general_purpose::STANDARD
                .decode(vector["ciphertext"].as_str().unwrap())
                .unwrap();
            SectorCipher::new(
                vector["cipher"].as_str().unwrap(),
                vector["mode"].as_str().unwrap(),
                &key,
            )
            .unwrap()
            .decrypt_area(&mut bytes, 512, vector["sector"].as_u64().unwrap() as u128)
            .unwrap();
            assert_eq!(
                bytes,
                (0..1024)
                    .map(|index| (index * 17 + 7) as u8)
                    .collect::<Vec<_>>(),
                "{vector}"
            );
        }
    }

    #[test]
    fn unsupported_combinations_and_alignment_are_rejected() {
        for (name, mode, size) in [
            ("twofish", "cbc-plain", 32),
            ("aes", "cbc-lmk", 32),
            ("aes", "cbc-essiv:sha1", 32),
            ("arc4", "cbc-plain", 16),
            ("aes", "xts-plain64", 24),
        ] {
            assert!(validate_cipher(name, mode, size).is_err());
        }
        let cipher = SectorCipher::new("aes", "cbc-plain64", &[0; 32]).unwrap();
        assert!(cipher.decrypt_area(&mut [0; 513], 512, 0).is_err());
        assert!(cipher.decrypt_area(&mut [0; 512], 0, 0).is_err());
        assert!(cipher
            .decrypt_area(&mut [0; 1024], 512, u64::MAX as u128)
            .is_err());
    }
}

#[derive(Clone, Copy)]
enum Chaining {
    Cbc,
    Ecb,
    Xts,
}

enum IvMode {
    Null,
    Plain,
    Plain64,
    Benbi,
    Essiv(Box<BlockCipher>),
}

enum BlockCipher {
    Aes128(Aes128),
    Aes192(Aes192),
    Aes256(Aes256),
    Blowfish(Box<Blowfish>),
    Serpent(Serpent),
}

impl BlockCipher {
    fn new(name: &str, key: &[u8]) -> Result<Self> {
        let invalid = |_| unsupported("cipher key size", format!("{name}: {} bytes", key.len()));
        match name {
            "aes" => match key.len() {
                16 => Aes128::new_from_slice(key)
                    .map(Self::Aes128)
                    .map_err(invalid),
                24 => Aes192::new_from_slice(key)
                    .map(Self::Aes192)
                    .map_err(invalid),
                32 => Aes256::new_from_slice(key)
                    .map(Self::Aes256)
                    .map_err(invalid),
                _ => Err(invalid(aes::cipher::InvalidLength)),
            },
            "blowfish" => Blowfish::new_from_slice(key)
                .map(|cipher| Self::Blowfish(Box::new(cipher)))
                .map_err(invalid),
            "serpent" => Serpent::new_from_slice(key)
                .map(Self::Serpent)
                .map_err(invalid),
            _ => Err(unsupported("cipher", name)),
        }
    }

    fn block_size(&self) -> usize {
        match self {
            Self::Blowfish(_) => 8,
            _ => 16,
        }
    }

    fn encrypt_iv(&self, iv: &mut [u8]) {
        match self {
            Self::Aes128(cipher) => cipher.encrypt_block(iv.into()),
            Self::Aes192(cipher) => cipher.encrypt_block(iv.into()),
            Self::Aes256(cipher) => cipher.encrypt_block(iv.into()),
            Self::Blowfish(cipher) => cipher.encrypt_block(iv.into()),
            Self::Serpent(cipher) => cipher.encrypt_block(iv.into()),
        }
    }

    fn decrypt(&self, chaining: Chaining, iv: &[u8], data: &mut [u8]) -> Result<()> {
        match self {
            Self::Aes128(cipher) => decrypt_blocks(cipher, chaining, iv, data),
            Self::Aes192(cipher) => decrypt_blocks(cipher, chaining, iv, data),
            Self::Aes256(cipher) => decrypt_blocks(cipher, chaining, iv, data),
            Self::Blowfish(cipher) => decrypt_blocks(cipher.as_ref(), chaining, iv, data),
            Self::Serpent(cipher) => decrypt_blocks(cipher, chaining, iv, data),
        }
    }
}

fn decrypt_blocks<C>(cipher: &C, chaining: Chaining, iv: &[u8], data: &mut [u8]) -> Result<()>
where
    C: aes::cipher::BlockCipher + BlockDecrypt + BlockSizeUser + Clone,
{
    match chaining {
        Chaining::Cbc => {
            let length = data.len();
            let iv = aes::cipher::generic_array::GenericArray::from_slice(iv);
            cbc::Decryptor::<C>::inner_iv_init(cipher.clone(), iv)
                .decrypt_padded_mut::<NoPadding>(data)
                .map_err(|_| unsupported("sector alignment", length.to_string()))?;
        }
        Chaining::Ecb => {
            for block in data.chunks_exact_mut(C::block_size()) {
                cipher.decrypt_block(block.into());
            }
        }
        Chaining::Xts => return Err(unsupported("chaining mode", "XTS block path")),
    }
    Ok(())
}

enum Cipher {
    Block(Box<BlockCipher>),
    Xts128(Box<Xts128<Aes128>>),
    Xts256(Box<Xts128<Aes256>>),
    Arc4(Zeroizing<Vec<u8>>),
}

/// Validates a cipher/mode/key length before any expensive password derivation.
/// Valid IVs are null, plain, plain64, benbi and ESSIV SHA1/SHA256 where the
/// hash size is a valid key size for the underlying cipher.
pub fn validate_cipher(name: &str, mode: &str, key_bytes: usize) -> Result<()> {
    if key_bytes > 256 {
        return Err(unsupported("cipher key size", key_bytes.to_string()));
    }
    SectorCipher::new(name, mode, &vec![0; key_bytes]).map(|_| ())
}

pub(crate) struct SectorCipher {
    cipher: Cipher,
    chaining: Chaining,
    iv: IvMode,
}

impl SectorCipher {
    pub(crate) fn new(name: &str, mode: &str, key: &[u8]) -> Result<Self> {
        let (chain, iv) = mode.split_once('-').unwrap_or((mode, ""));
        let chaining = match chain {
            "cbc" => Chaining::Cbc,
            "ecb" => Chaining::Ecb,
            "xts" => Chaining::Xts,
            _ => return Err(unsupported("cipher mode", mode)),
        };
        let cipher = match (name, chaining) {
            ("aes", Chaining::Xts) => match key.len() {
                32 => Cipher::Xts128(Box::new(Xts128::new(
                    Aes128::new_from_slice(&key[..16])
                        .map_err(|_| unsupported("xts key size", "32"))?,
                    Aes128::new_from_slice(&key[16..])
                        .map_err(|_| unsupported("xts key size", "32"))?,
                ))),
                64 => Cipher::Xts256(Box::new(Xts128::new(
                    Aes256::new_from_slice(&key[..32])
                        .map_err(|_| unsupported("xts key size", "64"))?,
                    Aes256::new_from_slice(&key[32..])
                        .map_err(|_| unsupported("xts key size", "64"))?,
                ))),
                _ => return Err(unsupported("xts key size", key.len().to_string())),
            },
            ("arc4", Chaining::Ecb) if (5..=256).contains(&key.len()) => {
                Cipher::Arc4(Zeroizing::new(key.to_vec()))
            }
            ("arc4", _) => return Err(unsupported("ARC4 mode/key size", mode)),
            (_, Chaining::Xts) => return Err(unsupported("XTS cipher", name)),
            (_, _) => Cipher::Block(Box::new(BlockCipher::new(name, key)?)),
        };
        let iv = match iv {
            "" if matches!(chaining, Chaining::Ecb) => IvMode::Null,
            "null" => IvMode::Null,
            "plain" => IvMode::Plain,
            "plain64" => IvMode::Plain64,
            "benbi" => IvMode::Benbi,
            "essiv:sha1" | "essiv:sha256" if matches!(chaining, Chaining::Cbc) => {
                let salt = Zeroizing::new(if iv == "essiv:sha1" {
                    sha1::Sha1::digest(key).to_vec()
                } else {
                    sha2::Sha256::digest(key).to_vec()
                });
                IvMode::Essiv(Box::new(BlockCipher::new(name, &salt)?))
            }
            _ => return Err(unsupported("initialization vector mode", iv)),
        };
        Ok(Self {
            cipher,
            chaining,
            iv,
        })
    }

    pub(crate) fn decrypt_area(
        &self,
        data: &mut [u8],
        unit_size: usize,
        base_sector: u128,
    ) -> Result<()> {
        if !matches!(unit_size, 512 | 4096) || data.len() % unit_size != 0 {
            return Err(unsupported(
                "sector alignment",
                format!("{} / {unit_size}", data.len()),
            ));
        }
        let step = (unit_size / 512) as u128;
        for (unit, chunk) in data.chunks_exact_mut(unit_size).enumerate() {
            let sector = base_sector
                .checked_add(unit as u128 * step)
                .and_then(|sector| u64::try_from(sector).ok())
                .ok_or_else(|| unsupported("IV sector overflow", base_sector.to_string()))?;
            let block_size = match &self.cipher {
                Cipher::Block(cipher) => cipher.block_size(),
                _ => 16,
            };
            let iv = self.initialization_vector(sector, block_size);
            match &self.cipher {
                Cipher::Block(cipher) => cipher.decrypt(self.chaining, &iv[..block_size], chunk)?,
                Cipher::Xts128(cipher) => cipher.decrypt_sector(chunk, iv),
                Cipher::Xts256(cipher) => cipher.decrypt_sector(chunk, iv),
                Cipher::Arc4(key) => {
                    // RustCrypto has a type-level key length. Its RC4 KSA consumes
                    // exactly 256 cyclic key bytes: expanding that cycle to U256
                    // preserves the KSA for every supported 5..=256-byte key.
                    let mut expanded = Zeroizing::new([0; 256]);
                    for (destination, source) in expanded.iter_mut().zip(key.iter().cycle()) {
                        *destination = *source;
                    }
                    Rc4::<U256>::new((&*expanded).into()).apply_keystream(chunk);
                }
            }
        }
        Ok(())
    }

    fn initialization_vector(&self, sector: u64, block_size: usize) -> [u8; 16] {
        let mut iv = [0; 16];
        match &self.iv {
            IvMode::Null => {}
            IvMode::Plain => iv[..4].copy_from_slice(&(sector as u32).to_le_bytes()),
            IvMode::Plain64 => iv[..8].copy_from_slice(&sector.to_le_bytes()),
            IvMode::Benbi => iv[block_size - 8..block_size].copy_from_slice(
                &sector
                    .wrapping_mul(512 / block_size as u64)
                    .wrapping_add(1)
                    .to_be_bytes(),
            ),
            IvMode::Essiv(cipher) => {
                iv[..8].copy_from_slice(&sector.to_le_bytes());
                cipher.encrypt_iv(&mut iv[..block_size]);
            }
        }
        iv
    }
}
