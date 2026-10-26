//! SipHash-2-4, written from the algorithm's published description
//! (Aumasson and Bernstein, "SipHash: a fast short-input PRF", 2012) and
//! checked against its published test vector.

fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13) ^ v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16) ^ v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21) ^ v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17) ^ v[2];
    v[2] = v[2].rotate_left(32);
}

/// SipHash-2-4 of `m` under the key `(k0, k1)`.
pub fn siphash24(k0: u64, k1: u64, m: &[u8]) -> u64 {
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    let (chunks, remainder) = m.as_chunks::<8>();
    for c in chunks {
        let w = u64::from_le_bytes(*c);
        v[3] ^= w;
        round(&mut v);
        round(&mut v);
        v[0] ^= w;
    }
    let mut last = (m.len() as u64 & 0xff) << 56;
    for (i, &b) in remainder.iter().enumerate() {
        last |= u64::from(b) << (8 * i);
    }
    v[3] ^= last;
    round(&mut v);
    round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_published_test_vector() {
        // Key 00..0f, message 00..0e (15 bytes): 0xa129ca6149be45e5.
        let k0 = u64::from_le_bytes([0, 1, 2, 3, 4, 5, 6, 7]);
        let k1 = u64::from_le_bytes([8, 9, 10, 11, 12, 13, 14, 15]);
        let m: Vec<u8> = (0..15).collect();
        assert_eq!(super::siphash24(k0, k1, &m), 0xa129_ca61_49be_45e5);
    }
}
