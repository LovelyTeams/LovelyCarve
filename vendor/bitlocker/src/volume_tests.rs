use super::*;
use crate::crypto::{aes_ccm_wrap, password_hash, stretch_key, SectorCipher};
use crate::metadata::{
    ENTRY_TYPE_FVEK, ENTRY_TYPE_VMK, ENTRY_TYPE_VOLUME_HEADER, PROTECTION_CLEAR,
    PROTECTION_PASSWORD,
};
use std::io::Cursor;

const RELOCATED_OFFSET: u64 = 0x4000;
const META_BLOCK_OFFSET: u64 = 0x1000;
const IMAGE_SIZE: usize = 0x5000;
// Smaller than the image so a physical offset past it exercises the
// still-encrypted-region boundary (bytes beyond are returned raw).
const ENCRYPTED_SIZE: u64 = 0x4800;

fn entry(entry_type: u16, value_type: u16, data: &[u8]) -> Vec<u8> {
    let size = (8 + data.len()) as u16;
    let mut v = Vec::new();
    v.extend_from_slice(&size.to_le_bytes());
    v.extend_from_slice(&entry_type.to_le_bytes());
    v.extend_from_slice(&value_type.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(data);
    v
}

/// Build a minimal synthetic BitLocker To Go volume plus the plaintext of its
/// relocated first sector.
fn build_volume(password: &str) -> (Vec<u8>, [u8; 512]) {
    let salt = [0x33u8; 16];
    let fvek = [0x11u8; 16];
    let tweak = [0x22u8; 16];
    let vmk = [0x44u8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    let stretched = stretch_key(&password_hash(password), &salt);
    let vmk_ccm = aes_ccm_wrap(&stretched, &[0x55; 12], &vmk_container);

    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..28].copy_from_slice(&fvek);
    fvek_container[44..60].copy_from_slice(&tweak);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x66; 12], &fvek_container);

    let mut stretch_data = vec![0u8; 4]; // 4-byte method
    stretch_data.extend_from_slice(&salt);
    let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&PROTECTION_PASSWORD.to_le_bytes());
    vmk_data.extend_from_slice(&stretch_entry);
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02; // bytes per sector = 512
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8000u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x5a;
    }
    let cipher = SectorCipher::new(fvek, tweak);
    let ct = cipher.encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

use crate::metadata::{VALUE_TYPE_AES_CCM, VALUE_TYPE_KEY, VALUE_TYPE_STRETCH, VALUE_TYPE_VMK};

#[test]
fn unlock_and_read_synthetic_volume() {
    // Self-consistency (Tier-3): we author both encoder and decoder here, so
    // this proves the pipeline is internally consistent, NOT that it matches
    // BitLocker. The Tier-1 pybde oracle (oracle_bdetogo.rs) is the real proof.
    let (image, plain) = build_volume("test-pw");
    let mut vol = BitLockerVolume::unlock_with_password(Cursor::new(image), "test-pw").unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8000);

    // A metadata-block region reads back as zero in the decrypted view.
    let mut z = [1u8; 512];
    vol.read_at(META_BLOCK_OFFSET, &mut z).unwrap();
    assert_eq!(z, [0u8; 512]);

    // A sector past the relocated volume-header region but still encrypted:
    // physical == logical, decrypted in place (no panic).
    let mut e = [0u8; 512];
    vol.read_at(0x600, &mut e).unwrap();

    // A sector at/after the encrypted-volume-size boundary is returned raw
    // (the image is zero there).
    let mut r = [0u8; 512];
    vol.read_at(ENCRYPTED_SIZE, &mut r).unwrap();
    assert_eq!(r, [0u8; 512]);
}

/// CBC-encrypt one sector for method 0x8002: IV = ECB(FVEK, LE128(off)),
/// then AES-128-CBC encrypt with the FVEK — no diffuser, no tweak. This is a
/// test-authored encoder (Tier-3 self-consistency); the Tier-1 proof that the
/// *decrypt* matches BitLocker is `core/tests/oracle_bitlocker1.rs`.
fn cbc_encrypt_sector(fvek: &[u8; 16], byte_offset: u64, plain: &[u8; 512]) -> [u8; 512] {
    use aes::cipher::block_padding::NoPadding;
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
    use aes::Aes128;

    let mut iv = [0u8; 16];
    iv[0..8].copy_from_slice(&byte_offset.to_le_bytes());
    let mut iv_block = GenericArray::clone_from_slice(&iv);
    Aes128::new(GenericArray::from_slice(fvek)).encrypt_block(&mut iv_block);

    let mut buf = *plain;
    cbc::Encryptor::<Aes128>::new(GenericArray::from_slice(fvek), &iv_block)
        .encrypt_padded_mut::<NoPadding>(&mut buf, 512)
        .unwrap();
    buf
}

/// Build a minimal synthetic method-0x8002 (AES-128-CBC, no diffuser) volume
/// plus the plaintext of its relocated first sector, wrapping the VMK under
/// `protector_type` with `key_hash` as the stretch input. The FVEK container
/// holds only a 128-bit FVEK (no TWEAK) — the key layout for a no-diffuser
/// volume.
fn build_cbc_volume(protector_type: u16, key_hash: [u8; 32]) -> (Vec<u8>, [u8; 512]) {
    let salt = [0x37u8; 16];
    let fvek = [0x13u8; 16];
    let vmk = [0x46u8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    let stretched = stretch_key(&key_hash, &salt);
    let vmk_ccm = aes_ccm_wrap(&stretched, &[0x57; 12], &vmk_container);

    // No TWEAK for 0x8002: the container carries only the FVEK at offset 12.
    let mut fvek_container = vec![0u8; 44];
    fvek_container[12..28].copy_from_slice(&fvek);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x68; 12], &fvek_container);

    let mut stretch_data = vec![0u8; 4];
    stretch_data.extend_from_slice(&salt);
    let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&protector_type.to_le_bytes());
    vmk_data.extend_from_slice(&stretch_entry);
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8002u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x3c;
    }
    let ct = cbc_encrypt_sector(&fvek, RELOCATED_OFFSET, &plain);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

#[test]
fn unlock_and_read_synthetic_cbc_volume() {
    // Self-consistency (Tier-3): we author both the CBC encoder and the
    // decoder here, so this proves the 0x8002 pipeline is internally
    // consistent, NOT that it matches BitLocker. The Tier-1 pybde oracle
    // (oracle_bitlocker1.rs) is the real proof.
    let (image, plain) = build_cbc_volume(PROTECTION_PASSWORD, password_hash("cbc-pw"));
    let mut vol = BitLockerVolume::unlock_with_password(Cursor::new(image), "cbc-pw").unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8002);
}

#[test]
fn unlock_and_read_synthetic_recovery_volume() {
    // Recovery-password unlock over a synthetic 0x8002 volume: the VMK is
    // wrapped with the recovery-key hash under a recovery protector (0x0800).
    // Tier-3 self-consistency for the recovery-pw wiring; the real end-to-end
    // proof is the m8003/vault Tier-1/2 oracles.
    let rk = "111111-111111-111111-111111-111111-111111-111111-111111";
    let key_hash = crate::crypto::recovery_key_hash(rk).unwrap();
    let (image, plain) = build_cbc_volume(crate::metadata::PROTECTION_RECOVERY, key_hash);
    let mut vol = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), rk).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8002);
}

#[test]
fn recovery_unlock_rejects_malformed_password() {
    let rk = "111111-111111-111111-111111-111111-111111-111111-111111";
    let key_hash = crate::crypto::recovery_key_hash(rk).unwrap();
    let (image, _) = build_cbc_volume(crate::metadata::PROTECTION_RECOVERY, key_hash);
    let res = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), "nope");
    assert!(matches!(res, Err(BdeError::InvalidRecoveryPassword { .. })));
}

#[test]
fn no_recovery_protector_errors() {
    // A password-only volume has no recovery protector to unlock with.
    let rk = "111111-111111-111111-111111-111111-111111-111111-111111";
    let (image, _) = build_cbc_volume(PROTECTION_PASSWORD, password_hash("cbc-pw"));
    let res = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), rk);
    assert!(matches!(res, Err(BdeError::NoUnlockProtector { .. })));
}

/// Build a synthetic method-0x8003 (AES-256-CBC, no diffuser) volume plus the
/// plaintext of its relocated first sector. The FVEK container carries a
/// 256-bit FVEK at offset 12 and no TWEAK.
fn build_cbc256_volume(key_hash: [u8; 32]) -> (Vec<u8>, [u8; 512]) {
    let salt = [0x39u8; 16];
    let fvek = [0x24u8; 32];
    let vmk = [0x48u8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    let stretched = stretch_key(&key_hash, &salt);
    let vmk_ccm = aes_ccm_wrap(&stretched, &[0x59; 12], &vmk_container);

    // 0x8003: 32-byte FVEK at offset 12, no TWEAK.
    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..44].copy_from_slice(&fvek);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x6a; 12], &fvek_container);

    let mut stretch_data = vec![0u8; 4];
    stretch_data.extend_from_slice(&salt);
    let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&PROTECTION_RECOVERY.to_le_bytes());
    vmk_data.extend_from_slice(&stretch_entry);
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8003u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x71;
    }
    let ct = SectorCipher::new_cbc256(fvek).encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

#[test]
fn unlock_and_read_synthetic_cbc256_volume() {
    // Tier-3 self-consistency for the 0x8003 pipeline; the Tier-2 proof is
    // oracle_m8003.rs.
    let rk = "222222-222222-222222-222222-222222-222222-222222-222222";
    let key_hash = crate::crypto::recovery_key_hash(rk).unwrap();
    let (image, plain) = build_cbc256_volume(key_hash);
    let mut vol = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), rk).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8003);
}

/// Build a synthetic method-0x8004 (XTS-AES-128) volume plus the plaintext of
/// its relocated first sector. The FVEK container carries the 32-byte XTS key
/// (two AES-128 keys) at offset 12.
fn build_xts128_volume(key_hash: [u8; 32]) -> (Vec<u8>, [u8; 512]) {
    let salt = [0x3bu8; 16];
    let xts_key = [0x35u8; 32];
    let vmk = [0x4au8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    let stretched = stretch_key(&key_hash, &salt);
    let vmk_ccm = aes_ccm_wrap(&stretched, &[0x5b; 12], &vmk_container);

    // 0x8004: 32-byte XTS key at offset 12, no separate TWEAK entry.
    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..44].copy_from_slice(&xts_key);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x6c; 12], &fvek_container);

    let mut stretch_data = vec![0u8; 4];
    stretch_data.extend_from_slice(&salt);
    let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&PROTECTION_RECOVERY.to_le_bytes());
    vmk_data.extend_from_slice(&stretch_entry);
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8004u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x2e;
    }
    let ct = SectorCipher::new_xts128(xts_key).encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

#[test]
fn unlock_and_read_synthetic_xts128_volume() {
    // Tier-3 self-consistency for the 0x8004 pipeline; the Tier-1 proof is
    // oracle_vault.rs.
    let rk = "333333-333333-333333-333333-333333-333333-333333-333333";
    let key_hash = crate::crypto::recovery_key_hash(rk).unwrap();
    let (image, plain) = build_xts128_volume(key_hash);
    let mut vol = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), rk).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8004);
}

/// Build a synthetic **clear-key** (protector 0x0000) method-0x8004 volume plus
/// the plaintext of its relocated first sector. The clear-key VMK carries a KEY
/// property (value type 0x0001: `method(u32)@0` then the 32-byte clear key) that
/// AES-CCM-unwraps the VMK directly — no stretch, no credential. `include_key`
/// false omits the KEY property to exercise the missing-key-material error.
fn build_clearkey_volume(include_key: bool) -> (Vec<u8>, [u8; 512]) {
    let clear_key = [0x77u8; 32];
    let xts_key = [0x35u8; 32];
    let vmk = [0x4au8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    // The clear key unwraps the VMK directly — no stretch stage.
    let vmk_ccm = aes_ccm_wrap(&clear_key, &[0x5b; 12], &vmk_container);

    // 0x8004: 32-byte XTS key at offset 12.
    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..44].copy_from_slice(&xts_key);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x6c; 12], &fvek_container);

    // KEY property: 4-byte method then the 32-byte clear key.
    let mut key_data = vec![0u8; 4];
    key_data.extend_from_slice(&clear_key);
    let key_entry = entry(0, VALUE_TYPE_KEY, &key_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&PROTECTION_CLEAR.to_le_bytes());
    if include_key {
        vmk_data.extend_from_slice(&key_entry);
    }
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8004u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x2e;
    }
    let ct = SectorCipher::new_xts128(xts_key).encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

#[test]
fn unlock_and_read_synthetic_clearkey_volume() {
    // Clear-key unlock with NO credential over a synthetic 0x8004 volume: the
    // VMK is wrapped directly by the clear key stored in the KEY property.
    // Tier-3 self-consistency for the clear-key wiring; the Tier-1 proof is the
    // pybde oracle (oracle_clearkey.rs).
    let (image, plain) = build_clearkey_volume(true);
    let mut vol = BitLockerVolume::unlock_clear_key(Cursor::new(image)).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8004);
}

#[test]
fn no_clear_key_protector_errors() {
    // A password-only volume has no clear-key protector to unlock with.
    let (image, _) = build_volume("test-pw");
    let res = BitLockerVolume::unlock_clear_key(Cursor::new(image));
    assert!(matches!(
        res,
        Err(BdeError::NoUnlockProtector { protector, .. }) if protector == "clear key"
    ));
}

#[test]
fn clear_key_missing_key_property_errors() {
    // A clear-key VMK with no KEY property cannot yield the unwrap key.
    let (image, _) = build_clearkey_volume(false);
    let res = BitLockerVolume::unlock_clear_key(Cursor::new(image));
    assert!(matches!(
        res,
        Err(BdeError::MissingKeyMaterial { what }) if what == "clear key"
    ));
}

/// Build a `.BEK` startup-key file byte blob carrying `external_key`, in the
/// libbde external-key layout: a 48-byte FVE metadata header, then an
/// external-key entry (value type 0x0009) whose payload is a 16-byte key GUID +
/// 8-byte FILETIME followed by a nested KEY property (0x0001: `type(u32)@0` then
/// the 32-byte key).
fn build_bek(external_key: &[u8; 32]) -> Vec<u8> {
    let mut key_data = vec![0u8; 4]; // 4-byte key type
    key_data.extend_from_slice(external_key);
    let key_entry = entry(0, VALUE_TYPE_KEY, &key_data);

    let mut ext_data = vec![0u8; 24]; // 16-byte key GUID + 8-byte FILETIME
    ext_data.extend_from_slice(&key_entry);
    let ext_entry = entry(0, crate::metadata::VALUE_TYPE_EXTERNAL_KEY, &ext_data);

    let mut bek = vec![0u8; 48]; // FVE metadata header
    bek.extend_from_slice(&ext_entry);
    bek
}

/// Build a synthetic method-0x8004 startup-key (protector 0x0200) volume plus
/// the plaintext of its relocated first sector and the matching `.BEK` bytes.
/// The VMK is AES-CCM-wrapped directly by the 256-bit external key (no stretch,
/// no KEY property in the VMK — the key lives in the `.BEK`).
fn build_startupkey_volume() -> (Vec<u8>, [u8; 512], Vec<u8>) {
    let external_key = [0x9au8; 32];
    let xts_key = [0x35u8; 32];
    let vmk = [0x4au8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    // The external key unwraps the VMK directly — no stretch stage.
    let vmk_ccm = aes_ccm_wrap(&external_key, &[0x5c; 12], &vmk_container);

    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..44].copy_from_slice(&xts_key);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x6d; 12], &fvek_container);

    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&crate::metadata::PROTECTION_STARTUP_KEY.to_le_bytes());
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8004u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x2e;
    }
    let ct = SectorCipher::new_xts128(xts_key).encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain, build_bek(&external_key))
}

#[test]
fn unlock_and_read_synthetic_startupkey_volume() {
    // Startup-key (.BEK) unlock over a synthetic 0x8004 volume: the VMK is
    // wrapped directly by the 256-bit external key held in the .BEK. Tier-3
    // self-consistency for the startup-key wiring; the Tier-2 proof is the
    // pybde-verified oracle (tests/oracle_startupkey.rs).
    let (image, plain, bek) = build_startupkey_volume();
    let mut vol = BitLockerVolume::unlock_with_startup_key(Cursor::new(image), &bek).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8004);
}

#[test]
fn no_startup_key_protector_errors() {
    // A password-only volume has no startup-key protector to unlock with.
    let (image, _) = build_volume("test-pw");
    let (_, _, bek) = build_startupkey_volume();
    let res = BitLockerVolume::unlock_with_startup_key(Cursor::new(image), &bek);
    assert!(matches!(
        res,
        Err(BdeError::NoUnlockProtector { protector, .. }) if protector == "startup key"
    ));
}

#[test]
fn startup_key_without_external_key_errors() {
    // A .BEK with no external-key entry cannot yield the unwrap key.
    let (image, _, _) = build_startupkey_volume();
    let empty_bek = vec![0u8; 48]; // header only, no external-key entry
    let res = BitLockerVolume::unlock_with_startup_key(Cursor::new(image), &empty_bek);
    assert!(matches!(
        res,
        Err(BdeError::MissingKeyMaterial { what }) if what == "external key"
    ));
}

#[test]
fn startup_key_external_key_without_key_property_errors() {
    // A .BEK whose external-key entry carries no nested KEY property.
    let (image, _, _) = build_startupkey_volume();
    // External-key entry payload: 16-byte GUID + 8-byte FILETIME, no nested KEY.
    let ext_entry = entry(0, crate::metadata::VALUE_TYPE_EXTERNAL_KEY, &[0u8; 24]);
    let mut bek = vec![0u8; 48];
    bek.extend_from_slice(&ext_entry);
    let res = BitLockerVolume::unlock_with_startup_key(Cursor::new(image), &bek);
    assert!(matches!(
        res,
        Err(BdeError::MissingKeyMaterial { what }) if what == "external key property"
    ));
}

/// Build a synthetic method-0x8005 (XTS-AES-256) volume plus the plaintext of
/// its relocated first sector. The FVEK container carries the 64-byte XTS key
/// (two AES-256 keys) at offset 12.
fn build_xts256_volume(key_hash: [u8; 32]) -> (Vec<u8>, [u8; 512]) {
    let salt = [0x3du8; 16];
    let mut xts_key = [0u8; 64];
    for (i, b) in xts_key.iter_mut().enumerate() {
        *b = 0x46u8.wrapping_add(i as u8);
    }
    let vmk = [0x4cu8; 32];

    let mut vmk_container = vec![0u8; 44];
    vmk_container[12..44].copy_from_slice(&vmk);
    let stretched = stretch_key(&key_hash, &salt);
    let vmk_ccm = aes_ccm_wrap(&stretched, &[0x5d; 12], &vmk_container);

    // 0x8005: 64-byte XTS key at offset 12.
    let mut fvek_container = vec![0u8; 76];
    fvek_container[12..76].copy_from_slice(&xts_key);
    let fvek_ccm = aes_ccm_wrap(&vmk, &[0x6e; 12], &fvek_container);

    let mut stretch_data = vec![0u8; 4];
    stretch_data.extend_from_slice(&salt);
    let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
    let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

    let mut vmk_data = vec![0u8; 28];
    vmk_data[26..28].copy_from_slice(&PROTECTION_RECOVERY.to_le_bytes());
    vmk_data.extend_from_slice(&stretch_entry);
    vmk_data.extend_from_slice(&vmk_ccm_entry);
    let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

    let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

    let mut vh_data = Vec::new();
    vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    vh_data.extend_from_slice(&512u64.to_le_bytes());
    let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

    let mut entries = Vec::new();
    entries.extend_from_slice(&vh_entry);
    entries.extend_from_slice(&vmk_entry);
    entries.extend_from_slice(&fvek_entry);
    let metadata_size = 48 + entries.len();

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

    let mb = META_BLOCK_OFFSET as usize;
    image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
    image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
    image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
    image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
    image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
    image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
    image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
    image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8005u16.to_le_bytes());
    image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

    let mut plain = [0u8; 512];
    for (i, b) in plain.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x17;
    }
    let ct = SectorCipher::new_xts256(xts_key).encrypt_sector(&plain, RELOCATED_OFFSET);
    let ro = RELOCATED_OFFSET as usize;
    image[ro..ro + 512].copy_from_slice(&ct);

    (image, plain)
}

#[test]
fn unlock_and_read_synthetic_xts256_volume() {
    // Tier-3 self-consistency for the 0x8005 pipeline; the Tier-2 proof is
    // oracle_m8005.rs.
    let rk = "444444-444444-444444-444444-444444-444444-444444-444444";
    let key_hash = crate::crypto::recovery_key_hash(rk).unwrap();
    let (image, plain) = build_xts256_volume(key_hash);
    let mut vol = BitLockerVolume::unlock_with_recovery_password(Cursor::new(image), rk).unwrap();
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, plain);
    assert_eq!(vol.metadata().encryption_method, 0x8005);
}

#[test]
fn seek_variants_and_eof() {
    use std::io::{Read as _, Seek as _};
    let (image, _) = build_volume("test-pw");
    let mut vol = BitLockerVolume::unlock_with_password(Cursor::new(image), "test-pw").unwrap();

    assert_eq!(vol.seek(SeekFrom::End(0)).unwrap(), IMAGE_SIZE as u64);
    let mut b = [0u8; 16];
    assert_eq!(vol.read(&mut b).unwrap(), 0); // read at EOF

    assert_eq!(vol.seek(SeekFrom::Start(10)).unwrap(), 10);
    assert_eq!(vol.seek(SeekFrom::Current(5)).unwrap(), 15);
    assert!(vol.seek(SeekFrom::Current(-100)).is_err()); // before start
}

/// A metadata-only image (no key material) with configurable cipher,
/// protectors, header offsets, and whether the block is actually present.
fn meta_only_image(
    method: u16,
    protectors: &[u16],
    header_offsets: [u64; 3],
    block_at: Option<usize>,
) -> Vec<u8> {
    let mut entries = Vec::new();
    for p in protectors {
        let mut d = vec![0u8; 28];
        d[26..28].copy_from_slice(&p.to_le_bytes());
        entries.extend(entry(0x0002, 0x0008, &d));
    }
    let metadata_size = 48 + entries.len();
    let mut image = vec![0u8; 0x2000];
    image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    image[3..11].copy_from_slice(b"MSWIN4.1");
    image[12] = 0x02;
    image[440..448].copy_from_slice(&header_offsets[0].to_le_bytes());
    image[448..456].copy_from_slice(&header_offsets[1].to_le_bytes());
    image[456..464].copy_from_slice(&header_offsets[2].to_le_bytes());
    if let Some(mb) = block_at {
        image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
        image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
        image[mb + 32..mb + 40].copy_from_slice(&(mb as u64).to_le_bytes());
        image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
        image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&method.to_le_bytes());
        image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);
    }
    image
}

#[test]
fn diffuser256_requires_key_material() {
    // Method 0x8001 is enabled, but incomplete key material must fail.
    let img = meta_only_image(0x8001, &[0x2000], [0x1000, 0, 0], Some(0x1000));
    let res = BitLockerVolume::unlock_with_password(Cursor::new(img), "x");
    assert!(matches!(res, Err(BdeError::MissingKeyMaterial { .. })));
}

#[test]
fn unrecognized_method_errors() {
    // Not a 0x800x BDE cipher at all — a distinct "unsupported" error.
    let img = meta_only_image(0x1234, &[0x2000], [0x1000, 0, 0], Some(0x1000));
    let res = BitLockerVolume::unlock_with_password(Cursor::new(img), "x");
    assert!(matches!(
        res,
        Err(BdeError::UnsupportedEncryptionMethod { method: 0x1234 })
    ));
}

#[test]
fn no_password_protector_errors() {
    // Recovery-only volume — no password protector to unlock with.
    let img = meta_only_image(0x8000, &[0x0800], [0x1000, 0, 0], Some(0x1000));
    let res = BitLockerVolume::unlock_with_password(Cursor::new(img), "x");
    assert!(matches!(
        res,
        Err(BdeError::NoUnlockProtector { protector, .. }) if protector == "password"
    ));
}

#[test]
fn no_valid_metadata_errors() {
    // Valid BitLocker header, but the block offsets point to non-FVE bytes.
    let img = meta_only_image(0x8000, &[0x2000], [0x1000, 0x1200, 0x1400], None);
    let err = BitLockerVolume::read_metadata(&mut Cursor::new(img)).unwrap_err();
    assert!(matches!(err, BdeError::NoValidMetadata { .. }));
}

#[test]
fn read_metadata_skips_zero_first_offset() {
    // First offset zero (skipped), second valid — covers the `continue`.
    let img = meta_only_image(0x8000, &[0x2000], [0, 0x1000, 0], Some(0x1000));
    let meta = BitLockerVolume::read_metadata(&mut Cursor::new(img)).unwrap();
    assert_eq!(meta.encryption_method, 0x8000);
}

/// A reader that returns a valid header once, then a transient `Interrupted`,
/// then a hard error — to exercise the I/O-error arms of `read_available`.
struct FlakyReader {
    header: Vec<u8>,
    phase: usize,
}

impl Read for FlakyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.phase += 1;
        match self.phase {
            1 => {
                let n = buf.len().min(self.header.len());
                buf[..n].copy_from_slice(&self.header[..n]);
                Ok(n)
            }
            2 => Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "eintr",
            )),
            _ => Err(std::io::Error::other("boom")),
        }
    }
}

impl Seek for FlakyReader {
    fn seek(&mut self, _pos: SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
}

#[test]
fn unreadable_metadata_copies_report_no_valid_metadata() {
    let mut header = vec![0u8; 512];
    header[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    header[3..11].copy_from_slice(b"MSWIN4.1");
    header[12] = 0x02;
    header[440..448].copy_from_slice(&0x1000u64.to_le_bytes());
    let mut reader = FlakyReader { header, phase: 0 };
    let res = BitLockerVolume::read_metadata(&mut reader);
    assert!(matches!(res, Err(BdeError::NoValidMetadata { .. })));
}

#[test]
fn wrong_password_fails_authentication() {
    let (image, _) = build_volume("test-pw");
    let res = BitLockerVolume::unlock_with_password(Cursor::new(image), "wrong");
    assert!(matches!(
        res,
        Err(BdeError::AuthenticationFailed { what }) if what == "volume master key"
    ));
}

#[test]
fn read_and_seek_traits() {
    use std::io::{Read as _, Seek as _};
    let (image, plain) = build_volume("test-pw");
    let mut vol = BitLockerVolume::unlock_with_password(Cursor::new(image), "test-pw").unwrap();
    vol.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = [0u8; 256];
    vol.read_exact(&mut buf).unwrap();
    assert_eq!(&buf[..], &plain[..256]);
    assert_eq!(vol.volume_size(), IMAGE_SIZE as u64);
}

#[test]
fn non_bitlocker_reader_errors() {
    let r = BitLockerVolume::unlock_with_password(Cursor::new(vec![0u8; 1024]), "x");
    assert!(matches!(r, Err(BdeError::NotBitLocker { .. })));
}

#[test]
fn read_metadata_reports_protectors() {
    let (image, _) = build_volume("test-pw");
    let meta = BitLockerVolume::read_metadata(&mut Cursor::new(image)).unwrap();
    assert_eq!(meta.protector_types(), vec![PROTECTION_PASSWORD]);
}
#[test]
fn multiple_password_protectors_try_authenticated_alternatives() {
    let (image, plain) = build_volume("test-pw");
    let mut metadata = BitLockerVolume::read_metadata(&mut Cursor::new(&image)).unwrap();
    let mut wrong = metadata.vmk_entries().next().unwrap().clone();
    // Alter the final authentication-protected byte of the first VMK record.
    *wrong.data.last_mut().unwrap() ^= 1;
    metadata.entries.insert(0, wrong);
    let cipher = derive_cipher(
        &metadata,
        SectorCipherKind::Cbc128Diffuser,
        PROTECTION_PASSWORD,
        "password",
        &VmkUnwrap::Stretch(password_hash("test-pw")),
    )
    .unwrap();
    let start = RELOCATED_OFFSET as usize;
    assert_eq!(
        cipher.decrypt_sector(&image[start..start + 512], RELOCATED_OFFSET),
        plain
    );
}

#[test]
fn truncated_ciphertext_and_overflow_are_not_decrypted_as_zeroes() {
    let (image, _) = build_volume("test-pw");
    let mut volume = BitLockerVolume::unlock_with_password(Cursor::new(image), "test-pw").unwrap();
    assert!(volume.read_at(u64::MAX, &mut [0; 2]).is_err());
    volume
        .reader
        .get_mut()
        .truncate(RELOCATED_OFFSET as usize + 100);
    assert!(volume.read_at(0, &mut [0; 512]).is_err());
}
