//! Salsa20/20 with a 256-bit key and a 64-bit nonce: the KDBX 3 inner stream.

use zeroize::Zeroize;

const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

pub(crate) struct Salsa20 {
    state: [u32; 16],
    block: [u8; 64],
    offset: usize,
}

impl Drop for Salsa20 {
    fn drop(&mut self) {
        self.state.zeroize();
        self.block.zeroize();
    }
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[b] ^= x[a].wrapping_add(x[d]).rotate_left(7);
    x[c] ^= x[b].wrapping_add(x[a]).rotate_left(9);
    x[d] ^= x[c].wrapping_add(x[b]).rotate_left(13);
    x[a] ^= x[d].wrapping_add(x[c]).rotate_left(18);
}

impl Salsa20 {
    pub(crate) fn new(key: &[u8; 32], nonce: &[u8; 8]) -> Self {
        let mut state = [0u32; 16];
        state[0] = SIGMA[0];
        for index in 0..4 {
            state[1 + index] = word(&key[4 * index..]);
            state[11 + index] = word(&key[16 + 4 * index..]);
        }
        state[5] = SIGMA[1];
        state[6] = word(&nonce[..4]);
        state[7] = word(&nonce[4..]);
        // state[8], state[9]: the 64-bit block counter, starting at zero.
        state[10] = SIGMA[2];
        state[15] = SIGMA[3];
        Self {
            state,
            block: [0; 64],
            offset: 64,
        }
    }

    fn refill(&mut self) {
        let mut x = self.state;
        for _ in 0..10 {
            quarter(&mut x, 0, 4, 8, 12);
            quarter(&mut x, 5, 9, 13, 1);
            quarter(&mut x, 10, 14, 2, 6);
            quarter(&mut x, 15, 3, 7, 11);
            quarter(&mut x, 0, 1, 2, 3);
            quarter(&mut x, 5, 6, 7, 4);
            quarter(&mut x, 10, 11, 8, 9);
            quarter(&mut x, 15, 12, 13, 14);
        }
        for (index, value) in x.iter().enumerate() {
            let sum = value.wrapping_add(self.state[index]);
            self.block[4 * index..4 * index + 4].copy_from_slice(&sum.to_le_bytes());
        }
        x.zeroize();
        self.state[8] = self.state[8].wrapping_add(1);
        if self.state[8] == 0 {
            self.state[9] = self.state[9].wrapping_add(1);
        }
        self.offset = 0;
    }

    /// XOR the next keystream bytes into `data`.
    pub(crate) fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            if self.offset == self.block.len() {
                self.refill();
            }
            *byte ^= self.block[self.offset];
            self.offset += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystream_matches_pycryptodome_across_block_boundaries() {
        // PyCryptodome 3: Salsa20.new(key=bytes(range(32)),
        // nonce=bytes(range(100, 108))).encrypt(bytes(130)).hex()
        let expected = concat!(
            "dac41fef88bcf58c96656f911f208f70fc63e045cdcd6d30eb76fdbc24be6930",
            "5c8f6ec704d87dd6af859d5753768f9c4e274b2312fb7b5a3eb3706e027549bc",
            "861039863cd6d2692b720d0b967d4dcbd30d7ec15e0ae9c2af4ee90109ac536e",
            "401e348f59d61781ba4ef9c07498c7cac43505b113aaebb9e5b6865f6a94c1ef",
            "10c4"
        );
        let key: [u8; 32] = std::array::from_fn(|index| index as u8);
        let nonce: [u8; 8] = std::array::from_fn(|index| 100 + index as u8);
        let mut stream = Salsa20::new(&key, &nonce);
        let mut data = vec![0u8; 130];
        let (first, rest) = data.split_at_mut(7);
        stream.apply(first);
        let (second, third) = rest.split_at_mut(60);
        stream.apply(second);
        stream.apply(third);
        assert_eq!(crate::bytes::hex(&data), expected);
    }
}
