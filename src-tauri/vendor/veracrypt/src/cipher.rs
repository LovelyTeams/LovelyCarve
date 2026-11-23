//! The block ciphers and cipher cascades VeraCrypt encrypts with.
//!
//! Every cipher takes a 256-bit key and a 128-bit block. A cascade lists its
//! ciphers in VeraCrypt's table order: encryption applies the first one first,
//! decryption the last one first, and VeraCrypt names it from the last cipher
//! (`{ Twofish, AES }` is "AES-Twofish").

use cipher::generic_array::GenericArray;
use cipher::inout::InOutBuf;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

use crate::error::{Result, VeraError};

/// Key bytes of every VeraCrypt cipher.
pub const CIPHER_KEY_LEN: usize = 32;

/// A block cipher of VeraCrypt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cipher {
    /// AES-256.
    Aes,
    /// Serpent-256.
    Serpent,
    /// Twofish-256.
    Twofish,
    /// Camellia-256.
    Camellia,
    /// Kuznyechik (GOST R 34.12-2015).
    Kuznyechik,
}

impl Cipher {
    /// VeraCrypt's display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Cipher::Aes => "AES",
            Cipher::Serpent => "Serpent",
            Cipher::Twofish => "Twofish",
            Cipher::Camellia => "Camellia",
            Cipher::Kuznyechik => "Kuznyechik",
        }
    }
}

/// One of VeraCrypt's encryption algorithms: a cipher or a cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CipherChain(&'static [Cipher]);

impl CipherChain {
    /// Every algorithm VeraCrypt reads, in its `EncryptionAlgorithms` table
    /// order, which is also the order the header search tries them.
    pub const ALL: [CipherChain; 15] = {
        use Cipher::{Aes, Camellia, Kuznyechik, Serpent, Twofish};
        [
            CipherChain(&[Aes]),
            CipherChain(&[Serpent]),
            CipherChain(&[Twofish]),
            CipherChain(&[Camellia]),
            CipherChain(&[Kuznyechik]),
            CipherChain(&[Twofish, Aes]),
            CipherChain(&[Serpent, Twofish, Aes]),
            CipherChain(&[Aes, Serpent]),
            CipherChain(&[Aes, Twofish, Serpent]),
            CipherChain(&[Serpent, Twofish]),
            CipherChain(&[Kuznyechik, Camellia]),
            CipherChain(&[Twofish, Kuznyechik]),
            CipherChain(&[Serpent, Camellia]),
            CipherChain(&[Aes, Kuznyechik]),
            CipherChain(&[Camellia, Serpent, Kuznyechik]),
        ]
    };

    /// The ciphers in table order (the first one encrypts first).
    #[must_use]
    pub fn ciphers(self) -> &'static [Cipher] {
        self.0
    }

    /// VeraCrypt's name: the last cipher first, joined with hyphens
    /// (`AES-Twofish-Serpent`).
    #[must_use]
    pub fn name(self) -> String {
        self.0
            .iter()
            .rev()
            .map(|cipher| cipher.name())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// The chain a name from [`Self::name`] gives, ignoring ASCII case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<CipherChain> {
        Self::ALL
            .into_iter()
            .find(|chain| chain.name().eq_ignore_ascii_case(name))
    }

    /// Master key bytes the chain uses: a primary and a secondary XTS key
    /// for each cipher.
    #[must_use]
    pub fn key_len(self) -> usize {
        2 * CIPHER_KEY_LEN * self.0.len()
    }
}

/// A cipher with its key schedule.
pub(crate) enum KeyedCipher {
    Aes(Box<aes::Aes256>),
    Serpent(Box<serpent::Serpent>),
    Twofish(Box<twofish::Twofish>),
    Camellia(Box<camellia::Camellia256>),
    Kuznyechik(Box<kuznyechik::Kuznyechik>),
}

impl KeyedCipher {
    /// Schedule `key` (exactly 32 bytes) for `cipher`.
    pub(crate) fn new(cipher: Cipher, key: &[u8]) -> Result<Self> {
        if key.len() != CIPHER_KEY_LEN {
            return Err(VeraError::InvalidRequest(format!(
                "{} takes a {CIPHER_KEY_LEN}-byte key",
                cipher.name()
            )));
        }
        let invalid = |_| VeraError::InvalidRequest(format!("{} key", cipher.name()));
        Ok(match cipher {
            Cipher::Aes => {
                KeyedCipher::Aes(Box::new(aes::Aes256::new(GenericArray::from_slice(key))))
            }
            // Serpent's typed key is 128 bits; the slice form schedules all 256.
            Cipher::Serpent => KeyedCipher::Serpent(Box::new(
                serpent::Serpent::new_from_slice(key).map_err(invalid)?,
            )),
            Cipher::Twofish => KeyedCipher::Twofish(Box::new(
                twofish::Twofish::new_from_slice(key).map_err(invalid)?,
            )),
            Cipher::Camellia => KeyedCipher::Camellia(Box::new(camellia::Camellia256::new(
                GenericArray::from_slice(key),
            ))),
            Cipher::Kuznyechik => KeyedCipher::Kuznyechik(Box::new(kuznyechik::Kuznyechik::new(
                GenericArray::from_slice(key),
            ))),
        })
    }

    /// Encrypt one block in place.
    pub(crate) fn encrypt_block(&self, block: &mut [u8; 16]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            KeyedCipher::Aes(cipher) => cipher.encrypt_block(block),
            KeyedCipher::Serpent(cipher) => cipher.encrypt_block(block),
            KeyedCipher::Twofish(cipher) => cipher.encrypt_block(block),
            KeyedCipher::Camellia(cipher) => cipher.encrypt_block(block),
            KeyedCipher::Kuznyechik(cipher) => cipher.encrypt_block(block),
        }
    }

    /// Encrypt whole blocks in place; `data.len()` is a multiple of 16.
    pub(crate) fn encrypt_blocks(&self, data: &mut [u8]) {
        let (blocks, _) = InOutBuf::from(data).into_chunks();
        match self {
            KeyedCipher::Aes(cipher) => cipher.encrypt_blocks_inout(blocks),
            KeyedCipher::Serpent(cipher) => cipher.encrypt_blocks_inout(blocks),
            KeyedCipher::Twofish(cipher) => cipher.encrypt_blocks_inout(blocks),
            KeyedCipher::Camellia(cipher) => cipher.encrypt_blocks_inout(blocks),
            KeyedCipher::Kuznyechik(cipher) => cipher.encrypt_blocks_inout(blocks),
        }
    }

    /// Decrypt whole blocks in place; `data.len()` is a multiple of 16.
    pub(crate) fn decrypt_blocks(&self, data: &mut [u8]) {
        let (blocks, _) = InOutBuf::from(data).into_chunks();
        match self {
            KeyedCipher::Aes(cipher) => cipher.decrypt_blocks_inout(blocks),
            KeyedCipher::Serpent(cipher) => cipher.decrypt_blocks_inout(blocks),
            KeyedCipher::Twofish(cipher) => cipher.decrypt_blocks_inout(blocks),
            KeyedCipher::Camellia(cipher) => cipher.decrypt_blocks_inout(blocks),
            KeyedCipher::Kuznyechik(cipher) => cipher.decrypt_blocks_inout(blocks),
        }
    }
}
