//! AES-XTS-128 decryption over the `xts-mode` crate (never hand-rolled).
//!
//! CoreStorage uses AES-XTS-128 with a little-endian tweak = the 0-based unit
//! index. Two units exist in the format: 8192-byte metadata units (tweak =
//! block index within the encrypted-metadata region) and 512-byte LV sectors
//! (tweak = logical sector number). `xts-mode`'s `get_tweak_default` encodes the
//! sector index as a little-endian 128-bit value, matching CoreStorage exactly
//! (verified against the dfvfs oracle at five offsets).

use aes::cipher::{generic_array::GenericArray, KeyInit};
use aes::Aes128;
use xts_mode::{get_tweak_default, Xts128};

/// Decrypt `buffer` in place as a run of AES-XTS-128 units of `unit_size` bytes.
///
/// `key1`/`key2` are the XTS key halves. `first_unit_index` is the tweak of the
/// first unit; each subsequent unit increments it by one. `buffer.len()` must be
/// a whole multiple of `unit_size`; a trailing partial unit is left untouched
/// (the caller only ever passes whole units).
pub fn decrypt_units(
    buffer: &mut [u8],
    key1: &[u8; 16],
    key2: &[u8; 16],
    unit_size: usize,
    first_unit_index: u128,
) {
    let cipher_1 = Aes128::new(GenericArray::from_slice(key1));
    let cipher_2 = Aes128::new(GenericArray::from_slice(key2));
    let xts = Xts128::<Aes128>::new(cipher_1, cipher_2);
    xts.decrypt_area(buffer, unit_size, first_unit_index, get_tweak_default);
}

/// Encrypt `buffer` in place (the inverse of [`decrypt_units`]); used only by
/// tests to build XTS fixtures. Kept `pub(crate)` so it is not public surface.
#[cfg(any(test, feature = "testutil"))]
pub(crate) fn encrypt_units(
    buffer: &mut [u8],
    key1: &[u8; 16],
    key2: &[u8; 16],
    unit_size: usize,
    first_unit_index: u128,
) {
    let cipher_1 = Aes128::new(GenericArray::from_slice(key1));
    let cipher_2 = Aes128::new(GenericArray::from_slice(key2));
    let xts = Xts128::<Aes128>::new(cipher_1, cipher_2);
    xts.encrypt_area(buffer, unit_size, first_unit_index, get_tweak_default);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypt_inverts_encrypt() {
        let key1 = [0x11u8; 16];
        let key2 = [0x22u8; 16];
        let plaintext: Vec<u8> = (0..1024u32).map(|i| (i & 0xff) as u8).collect();
        let mut buffer = plaintext.clone();
        encrypt_units(&mut buffer, &key1, &key2, 512, 5);
        assert_ne!(buffer, plaintext, "ciphertext must differ from plaintext");
        decrypt_units(&mut buffer, &key1, &key2, 512, 5);
        assert_eq!(buffer, plaintext, "decrypt(encrypt(x)) == x");
    }

    #[test]
    fn tweak_index_matters() {
        // The same block under different unit indices decrypts differently —
        // confirms the tweak is applied per unit.
        let key1 = [0x33u8; 16];
        let key2 = [0x44u8; 16];
        let mut a = vec![0u8; 512];
        let mut b = vec![0u8; 512];
        decrypt_units(&mut a, &key1, &key2, 512, 0);
        decrypt_units(&mut b, &key1, &key2, 512, 1);
        assert_ne!(a, b);
    }
}
