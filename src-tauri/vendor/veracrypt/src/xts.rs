//! XTS (IEEE 1619) over 512-byte data units, for single ciphers and
//! cascades, as VeraCrypt applies it to headers and to the data area.
//!
//! Data unit `n` uses tweak `E_K2(LE64(n) || 0^64)`; each following block of
//! the unit multiplies the tweak by α in GF(2^128). A cascade applies one
//! XTS layer per cipher, each with its own key pair.

use crate::cipher::{CipherChain, KeyedCipher, CIPHER_KEY_LEN};
use crate::error::{Result, VeraError};

/// Bytes of one XTS data unit, whatever the volume's sector size.
pub const DATA_UNIT: usize = 512;

const BLOCK: usize = 16;
const BLOCKS_PER_UNIT: usize = DATA_UNIT / BLOCK;

#[derive(Clone, Copy)]
enum Direction {
    // The reader only decrypts; tests encrypt to check against VeraCrypt.
    #[cfg_attr(not(test), allow(dead_code))]
    Encrypt,
    Decrypt,
}

struct Layer {
    data: KeyedCipher,
    tweak: KeyedCipher,
}

/// A keyed cipher chain in XTS mode.
pub(crate) struct Xts {
    /// One layer per cipher, in table order.
    layers: Vec<Layer>,
}

impl Xts {
    /// Key `chain` from master key material: cipher `j` of `n` takes its
    /// primary key at byte `32 j` and its secondary (tweak) key at
    /// `32 (n + j)`, as VeraCrypt's `EAInit` / `EAInitMode`.
    pub(crate) fn new(chain: CipherChain, key: &[u8]) -> Result<Self> {
        let count = chain.ciphers().len();
        if key.len() < chain.key_len() {
            return Err(VeraError::InvalidRequest(format!(
                "{} needs {} key bytes",
                chain.name(),
                chain.key_len()
            )));
        }
        let slice = |index: usize| &key[CIPHER_KEY_LEN * index..CIPHER_KEY_LEN * (index + 1)];
        let layers = chain
            .ciphers()
            .iter()
            .enumerate()
            .map(|(index, &cipher)| {
                Ok(Layer {
                    data: KeyedCipher::new(cipher, slice(index))?,
                    tweak: KeyedCipher::new(cipher, slice(count + index))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { layers })
    }

    /// Decrypt `data` in place. Its data unit `k` has number
    /// `first_unit + k`; a final unit shorter than 512 bytes (a 448-byte
    /// header) is one unit of its own length. `data.len()` is a multiple of
    /// 16.
    pub(crate) fn decrypt(&self, data: &mut [u8], first_unit: u64) {
        for layer in self.layers.iter().rev() {
            layer.apply(data, first_unit, Direction::Decrypt);
        }
    }

    /// Encrypt `data` in place, the inverse of [`Self::decrypt`].
    #[cfg(test)]
    pub(crate) fn encrypt(&self, data: &mut [u8], first_unit: u64) {
        for layer in &self.layers {
            layer.apply(data, first_unit, Direction::Encrypt);
        }
    }
}

impl Layer {
    fn apply(&self, data: &mut [u8], first_unit: u64, direction: Direction) {
        let mut tweaks = [0u128; BLOCKS_PER_UNIT];
        for (index, unit) in data.chunks_mut(DATA_UNIT).enumerate() {
            let number = first_unit.wrapping_add(index as u64);
            let mut tweak = [0u8; BLOCK];
            tweak[..8].copy_from_slice(&number.to_le_bytes());
            self.tweak.encrypt_block(&mut tweak);
            let mut value = u128::from_le_bytes(tweak);
            let whole = unit.len() / BLOCK * BLOCK;
            let unit = &mut unit[..whole];
            for (slot, block) in tweaks.iter_mut().zip(unit.chunks_exact_mut(BLOCK)) {
                *slot = value;
                xor(block, value);
                value = double(value);
            }
            match direction {
                Direction::Encrypt => self.data.encrypt_blocks(unit),
                Direction::Decrypt => self.data.decrypt_blocks(unit),
            }
            for (slot, block) in tweaks.iter().zip(unit.chunks_exact_mut(BLOCK)) {
                xor(block, *slot);
            }
        }
        tweaks.fill(0);
    }
}

/// Multiply a little-endian tweak by α in GF(2^128) (x^128 + x^7 + x^2 + x + 1).
fn double(value: u128) -> u128 {
    (value << 1) ^ ((value >> 127) * 0x87)
}

fn xor(block: &mut [u8], value: u128) {
    let mut bytes = [0u8; BLOCK];
    bytes.copy_from_slice(block);
    block.copy_from_slice(&(u128::from_le_bytes(bytes) ^ value).to_le_bytes());
}

#[cfg(test)]
#[path = "xts_tests.rs"]
mod tests;
