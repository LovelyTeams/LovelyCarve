use super::*;
use crate::cipher::{Cipher, CipherChain};

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

const IEEE_KEY1: [u8; 32] = [
    0x27, 0x18, 0x28, 0x18, 0x28, 0x45, 0x90, 0x45, 0x23, 0x53, 0x60, 0x28, 0x74, 0x71, 0x35, 0x26,
    0x62, 0x49, 0x77, 0x57, 0x24, 0x70, 0x93, 0x69, 0x99, 0x59, 0x57, 0x49, 0x66, 0x96, 0x76, 0x27,
];
const IEEE_KEY2: [u8; 32] = [
    0x31, 0x41, 0x59, 0x26, 0x53, 0x58, 0x97, 0x93, 0x23, 0x84, 0x62, 0x64, 0x33, 0x83, 0x27, 0x95,
    0x02, 0x88, 0x41, 0x97, 0x16, 0x93, 0x99, 0x37, 0x51, 0x05, 0x82, 0x09, 0x74, 0x94, 0x45, 0x92,
];

/// IEEE 1619 vector 14, the data unit VeraCrypt's `TestSectorBufEncryption`
/// moves through its buffer (also reproduced with OpenSSL's AES-XTS).
const IEEE_VECTOR_14: &str = "64497e5a831e4a932c09be3e5393376daa599548b816031d224bbf50a818ed2350eae7e96087c8a0db51ad290bd00c1ac1620857635bf246c176ab463be30b808da548081ac847b158e1264be25bb0910bbc92647108089415d45fab1b3d2604e8a8eff1ae4020cfa39936b66827b23f371b92200be90251e6d73c5f86de5fd4a950781933d79a28272b782a2ec313efdfcc0628f43d744c2dc2ff3dcb66999b50c7ca895b0c64791eeaa5f29499fb1c026f84ce5b5c72ba1083cddb5ce45434631665c333b60b11593fb253c5179a2c8db813782a004856a1653011e93fb6d876c18366dd8683f53412c0c180f9c848592d593f8609ca736317d356e13e2bff3a9f59cd9aeb19cd482593d8c46128bb32423b37a9adfb482b99453fbe25a41bf6feb4aa0bef5ed24bf73c762978025482c13115e4015aac992e5613a3b5c2f685b84795cb6e9b2656d8c88157e52c42f978d8634c43d06fea928f2822e465aa6576e9bf419384506cc3ce3c54ac1a6f67dc66f3b30191e698380bc999b05abce19dc0c6dcc2dd001ec535ba18deb2df1a101023108318c75dc98611a09dc48a0acdec676fabdf222f07e026f059b672b56e5cbc8e1d21bbd867dd927212054681d70ea737134cdfce93b6f82ae22423274e58a0821cc5502e2d0ab4585e94de6975be5e0b4efce51cd3e70c25a1fbbbd609d273ad5b0d59631c531f6a0a57b9";

/// IEEE 1619 vector 10 (data unit 0xff).
const IEEE_VECTOR_10_PREFIX: &str =
    "1c3b3a102f770386e4836c99e370cf9bea00803f5e482357a4ae12d414a3e63b";

fn ieee_plaintext() -> Vec<u8> {
    (0..512).map(|index| index as u8).collect()
}

#[test]
fn aes_matches_ieee_1619_vectors() {
    let mut key = IEEE_KEY1.to_vec();
    key.extend_from_slice(&IEEE_KEY2);
    let xts = Xts::new(CipherChain::ALL[0], &key).unwrap();
    let mut unit = ieee_plaintext();
    xts.encrypt(&mut unit, 0xff_ffff_ffff);
    assert_eq!(unit, unhex(IEEE_VECTOR_14));
    xts.decrypt(&mut unit, 0xff_ffff_ffff);
    assert_eq!(unit, ieee_plaintext());
    let mut unit = ieee_plaintext();
    xts.encrypt(&mut unit, 0xff);
    assert_eq!(unit[..32], unhex(IEEE_VECTOR_10_PREFIX));
}

/// CRC-32s VeraCrypt's `TestSectorBufEncryption` (`src/Common/Tests.c`)
/// expects for every algorithm: four data units at numbers
/// `0xFFFFFFFFFF - case`, then the same buffer as units 0–3.
const SECTOR_BUFFER_CRCS: [(&str, [u32; 4], u32); 15] = [
    (
        "AES",
        [0x888f_2990, 0xea28_ea34, 0xe058_f5a2, 0x1047_3dc9],
        0x33b9_1fab,
    ),
    (
        "Serpent",
        [0x7edf_ecb3, 0x357b_aaaa, 0xc7b9_fca5, 0xb526_3e0c],
        0x3494_d480,
    ),
    (
        "Twofish",
        [0x9152_5124, 0x2895_cc47, 0x6bee_346d, 0xb1c4_5759],
        0xc4d6_5b46,
    ),
    (
        "Camellia",
        [0x2436_badb, 0x247d_2272, 0x72b4_9cde, 0xb838_d2c1],
        0x8176_b223,
    ),
    (
        "Kuznyechik",
        [0x6b86_e72e, 0xa4f8_637d, 0xfd83_e76d, 0xb24f_c47b],
        0xd6d3_9cdb,
    ),
    (
        "AES-Twofish",
        [0x6cea_7fa2, 0x6905_2c4c, 0x88db_8de5, 0xf16f_d8c5],
        0x14ce_7385,
    ),
    (
        "AES-Twofish-Serpent",
        [0xa2d7_d82a, 0xdbf7_6412, 0xdf0e_a03e, 0xdade_dff7],
        0x0ec8_1bf7,
    ),
    (
        "Serpent-AES",
        [0x6dd1_33b3, 0x0e57_17d2, 0x39f8_3cd9, 0x8a79_fa2c],
        0x42f9_19ad,
    ),
    (
        "Serpent-Twofish-AES",
        [0xe536_daf8, 0x3ae8_9e7f, 0x2cc1_301a, 0xcac7_bdc7],
        0x208d_5c58,
    ),
    (
        "Twofish-Serpent",
        [0x2686_c859, 0x8a20_1780, 0x8dd1_3796, 0xe951_96cb],
        0xbe78_cec1,
    ),
    (
        "Camellia-Kuznyechik",
        [0x4ea3_4e89, 0xb3ad_8559, 0xde36_1313, 0x519d_2bf9],
        0xe69d_680d,
    ),
    (
        "Kuznyechik-Twofish",
        [0x748f_8631, 0x8a4b_0888, 0xe031_0188, 0xfc2b_6b45],
        0xe0ae_f0d1,
    ),
    (
        "Camellia-Serpent",
        [0x1056_9a42, 0x4de9_5152, 0xe17b_5fe7, 0xdbf9_93fa],
        0x58aa_d727,
    ),
    (
        "Kuznyechik-AES",
        [0xc479_f95d, 0x58ea_f88d, 0xbe30_0cc2, 0x9b68_1c2e],
        0x4641_234a,
    ),
    (
        "Kuznyechik-Serpent-Camellia",
        [0x9d8a_c7ee, 0x5d7d_347f, 0x884b_62ee, 0x5c6c_3997],
        0x755d_ad72,
    ),
];

/// The key material of `TestSectorBufEncryption`: `key1` holds the primary
/// keys (IEEE key 1, IEEE key 2, then a third row); the secondary keys are
/// IEEE key 2 followed by bytes `32, 33, …`.
fn sector_buffer_key(chain: CipherChain) -> Vec<u8> {
    let count = chain.ciphers().len();
    let mut primary = IEEE_KEY1.to_vec();
    primary.extend_from_slice(&IEEE_KEY2);
    primary.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0]);
    primary.extend(1..=0x13u8);
    let mut secondary: Vec<u8> = (0..=255u8).collect();
    secondary[..32].copy_from_slice(&IEEE_KEY2);
    let mut key = primary[..32 * count].to_vec();
    key.extend_from_slice(&secondary[..32 * count]);
    key
}

#[test]
fn every_algorithm_matches_veracrypt_sector_buffer_self_test() {
    let plaintext: Vec<u8> = ieee_plaintext().repeat(4);
    assert_eq!(crc32fast::hash(&plaintext), 0x9f5e_dd58);
    assert_eq!(
        CipherChain::ALL.map(CipherChain::name),
        SECTOR_BUFFER_CRCS.map(|(name, _, _)| name.to_string())
    );
    for (chain, (name, unit_crcs, buffer_crc)) in
        CipherChain::ALL.into_iter().zip(SECTOR_BUFFER_CRCS)
    {
        let xts = Xts::new(chain, &sector_buffer_key(chain))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        for (case, expected) in unit_crcs.into_iter().enumerate() {
            let mut buffer = plaintext.clone();
            let first = 0xff_ffff_ffff - case as u64;
            xts.encrypt(&mut buffer, first);
            assert_eq!(crc32fast::hash(&buffer), expected, "{name} case {case}");
            if chain.ciphers() == [Cipher::Aes] {
                assert_eq!(buffer[512 * case..512 * (case + 1)], unhex(IEEE_VECTOR_14));
            }
            xts.decrypt(&mut buffer, first);
            assert_eq!(buffer, plaintext, "{name} case {case} decrypts back");
        }
        let mut buffer = plaintext.clone();
        xts.encrypt(&mut buffer, 0);
        assert_eq!(crc32fast::hash(&buffer), buffer_crc, "{name} buffer");
        xts.decrypt(&mut buffer, 0);
        assert_eq!(buffer, plaintext, "{name} buffer decrypts back");
    }
}

/// An independent XTS implementation (`xts-mode`) agrees on data units and
/// on a 448-byte header unit.
#[test]
fn single_ciphers_match_the_xts_mode_crate() {
    use cipher::KeyInit;
    let key: Vec<u8> = (0..64u8).map(|byte| byte.wrapping_mul(29) ^ 0x5c).collect();
    let data: Vec<u8> = (0..1536u32).map(|index| (index * 7 + 3) as u8).collect();
    let header: Vec<u8> = data[..448].to_vec();
    for chain in &CipherChain::ALL[..3] {
        let ours = Xts::new(*chain, &key).unwrap();
        let mut expected = data.clone();
        let mut expected_header = header.clone();
        let (k1, k2) = key.split_at(32);
        match chain.ciphers()[0] {
            Cipher::Aes => {
                let xts = xts_mode::Xts128::new(
                    aes::Aes256::new_from_slice(k1).unwrap(),
                    aes::Aes256::new_from_slice(k2).unwrap(),
                );
                xts.encrypt_area(&mut expected, 512, 77, xts_mode::get_tweak_default);
                xts.encrypt_area(&mut expected_header, 448, 0, xts_mode::get_tweak_default);
            }
            Cipher::Serpent => {
                let xts = xts_mode::Xts128::new(
                    serpent::Serpent::new_from_slice(k1).unwrap(),
                    serpent::Serpent::new_from_slice(k2).unwrap(),
                );
                xts.encrypt_area(&mut expected, 512, 77, xts_mode::get_tweak_default);
                xts.encrypt_area(&mut expected_header, 448, 0, xts_mode::get_tweak_default);
            }
            Cipher::Twofish => {
                let xts = xts_mode::Xts128::new(
                    twofish::Twofish::new_from_slice(k1).unwrap(),
                    twofish::Twofish::new_from_slice(k2).unwrap(),
                );
                xts.encrypt_area(&mut expected, 512, 77, xts_mode::get_tweak_default);
                xts.encrypt_area(&mut expected_header, 448, 0, xts_mode::get_tweak_default);
            }
            Cipher::Camellia | Cipher::Kuznyechik => {
                unreachable!("first three chains are AES, Serpent, Twofish")
            }
        }
        let mut actual = data.clone();
        ours.encrypt(&mut actual, 77);
        assert_eq!(actual, expected, "{}", chain.name());
        ours.decrypt(&mut actual, 77);
        assert_eq!(actual, data);
        let mut actual = header.clone();
        ours.encrypt(&mut actual, 0);
        assert_eq!(actual, expected_header, "{} header", chain.name());
    }
}

#[test]
fn short_keys_are_rejected() {
    assert!(Xts::new(CipherChain::ALL[5], &[0u8; 127]).is_err());
    assert!(Xts::new(CipherChain::ALL[5], &[0u8; 128]).is_ok());
}

#[test]
fn cascade_names_follow_veracrypt() {
    assert_eq!(CipherChain::ALL[6].name(), "AES-Twofish-Serpent");
    assert_eq!(
        CipherChain::from_name("serpent-twofish-aes"),
        Some(CipherChain::ALL[8])
    );
    assert_eq!(CipherChain::ALL[14].key_len(), 192);
}
