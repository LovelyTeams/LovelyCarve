//! QCOW's legacy 512-byte sector AES-CBC, without padding or a key verifier.
use crate::{Error, Result};
use aes::{
    cipher::{Block, BlockCipherDecrypt, KeyInit},
    Aes128,
};
use zeroize::Zeroizing;

/// Legacy AES-128 key, zeroed on drop. Debug never reveals its bytes.
#[derive(Clone)]
pub struct EncryptionKey(Zeroizing<[u8; 16]>);
impl EncryptionKey {
    /// Supply the exact 16-byte key (no password encoding ambiguity).
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    /// Derive the legacy key from at most 16 ASCII password bytes, zero-padded.
    /// Reject non-ASCII and embedded NUL instead of guessing an encoding.
    pub fn from_password(password: &str) -> Result<Self> {
        if !password.is_ascii() || password.as_bytes().contains(&0) {
            return Err(Error::FileFormat(
                "legacy QCOW password must be ASCII without NUL".into(),
            ));
        }
        let mut key = Zeroizing::new([0; 16]);
        let length = password.len().min(16);
        key[..length].copy_from_slice(&password.as_bytes()[..length]);
        Ok(Self(key))
    }
    /// UTF-16 input, using the same ASCII rules as `from_password`.
    pub fn from_utf16_password(password: &[u16]) -> Result<Self> {
        let password = Zeroizing::new(
            String::from_utf16(password)
                .map_err(|_| Error::FileFormat("invalid UTF-16 password".into()))?,
        );
        Self::from_password(&password)
    }
    pub(crate) fn decrypt(&self, guest_position: u64, bytes: &mut [u8]) -> Result<()> {
        if !guest_position.is_multiple_of(512) || !bytes.len().is_multiple_of(512) {
            return Err(Error::Internal("unaligned AES sector read".into()));
        }
        let cipher = Aes128::new_from_slice(self.0.as_ref())
            .map_err(|_| Error::Internal("invalid AES key length".into()))?;
        for (index, sector) in bytes.as_chunks_mut::<512>().0.iter_mut().enumerate() {
            let mut previous = [0; 16];
            previous[..8].copy_from_slice(&(guest_position / 512 + index as u64).to_le_bytes());
            for block_bytes in sector.as_chunks_mut::<16>().0 {
                let mut encrypted = [0; 16];
                encrypted.copy_from_slice(block_bytes);
                let mut block = Block::<Aes128>::default();
                block.copy_from_slice(block_bytes);
                cipher.decrypt_block(&mut block);
                for ((output, plain), iv) in block_bytes
                    .iter_mut()
                    .zip(block.iter())
                    .zip(previous.iter())
                {
                    *output = plain ^ iv;
                }
                previous = encrypted;
            }
        }
        Ok(())
    }
}
impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EncryptionKey([redacted])")
    }
}
