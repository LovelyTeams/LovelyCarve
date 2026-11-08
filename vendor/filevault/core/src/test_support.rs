//! Shared `#[cfg(test)]` synthetic-image builder.
//!
//! Builds a whole synthetic CoreStorage image (header + plaintext metadata +
//! AES-XTS-encrypted metadata + encrypted logical volume) so tests exercise the
//! full unlock/decrypt pipeline hermetically, WITHOUT the env-gated Tier-1
//! oracle. This is Tier-3 scaffolding *under* the oracle (which is Tier-1): it
//! specifies the wiring behaviour; the real-data decrypt proves correctness.
//!
//! Used by both the `lib` unit tests and the `vfs` adapter tests, so the builder
//! lives here once rather than being duplicated per module.

use aes_kw::KekAes128;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::Hmac;
use pbkdf2::pbkdf2;
use sha2::{Digest, Sha256};

pub const BLOCK: usize = 4096;
pub const PASSWORD: &str = "synthetic-pass";
pub const SALT: [u8; 16] = [0xAB; 16];
pub const ITERS: u32 = 1000;
pub const KEK: [u8; 16] = [0x10; 16];
pub const VMK: [u8; 16] = [0x20; 16];
pub const KEY1: [u8; 16] = [0x30; 16]; // metadata XTS key1
pub const PV_ID: [u8; 16] = [0x40; 16]; // metadata XTS key2
pub const FAMILY: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];
pub const LV_PHYS_BLOCK: u64 = 16; // LV base = 16 * 4096 = 0x10000
pub const LV_BLOCKS: u32 = 4; // 4 * 4096 = 16384 bytes

fn wrap(kek: &[u8; 16], key: &[u8; 16]) -> [u8; 24] {
    let k = KekAes128::from(*kek);
    let mut out = [0u8; 24];
    k.wrap(key, &mut out).unwrap();
    out
}

fn set_block_type(block: &mut [u8], t: u16) {
    block[10..12].copy_from_slice(&t.to_le_bytes());
}

/// Build a decrypted metadata region: block 0 = context plist (0x0013),
/// block 1 = 0x001a familyUUID+size, block 2 = 0x0305 segment descriptor.
fn build_decrypted_metadata(iterations: u32, status: &str) -> Vec<u8> {
    let passphrase_key = {
        let mut pk = [0u8; 16];
        pbkdf2::<Hmac<Sha256>>(PASSWORD.as_bytes(), &SALT, iterations, &mut pk).unwrap();
        pk
    };
    let wrapped_kek = wrap(&passphrase_key, &KEK);
    let wrapped_vmk = wrap(&KEK, &VMK);

    // PassphraseWrappedKEKStruct (284 bytes): salt@8, wrappedKEK@32, iters@168.
    let mut pw = vec![0u8; 284];
    pw[8..24].copy_from_slice(&SALT);
    pw[32..56].copy_from_slice(&wrapped_kek);
    pw[168..172].copy_from_slice(&iterations.to_le_bytes());
    // KEKWrappedVolumeKeyStruct (256): wrappedVMK@8.
    let mut kekw = vec![0u8; 256];
    kekw[8..32].copy_from_slice(&wrapped_vmk);

    let plist = format!(
        "<dict ID=\"0\"><key>CryptoUsers</key><array ID=\"2\"><dict ID=\"3\">\
         <key>PassphraseWrappedKEKStruct</key><data ID=\"4\">{}</data>\
         <key>UserType</key><integer size=\"32\" ID=\"5\">0x10000001</integer>\
         <key>BlockAlgorithm</key><string ID=\"6\">AES-XTS</string>\
         <key>KEKWrappedVolumeKeyStruct</key><data ID=\"7\">{}</data>\
         </dict></array>\
         <key>ConversionStatus</key><string ID=\"8\">{status}</string></dict>",
        B64.encode(&pw),
        B64.encode(&kekw),
    );

    const OBJECT: usize = 8192;
    let mut meta = vec![0u8; OBJECT * 3];
    // Block 0: context plist.
    set_block_type(&mut meta[0..OBJECT], 0x0019);
    meta[24..32].copy_from_slice(&10u64.to_le_bytes());
    let body = plist.as_bytes();
    meta[104..108].copy_from_slice(&(body.len() as u32).to_le_bytes());
    meta[108..112].copy_from_slice(&(body.len() as u32).to_le_bytes());
    meta[112..116].copy_from_slice(&184u32.to_le_bytes());
    meta[116..120].copy_from_slice(&(body.len() as u32).to_le_bytes());
    meta[184..184 + body.len()].copy_from_slice(body);

    // Block 1: 0x001a familyUUID + name + uuid + size.
    set_block_type(&mut meta[OBJECT..2 * OBJECT], 0x001a);
    meta[OBJECT + 24..OBJECT + 32].copy_from_slice(&11u64.to_le_bytes());
    meta[OBJECT + 64..OBJECT + 72].copy_from_slice(&10u64.to_le_bytes());
    meta[OBJECT + 72..OBJECT + 80].copy_from_slice(&8u64.to_le_bytes());
    let fam_uuid = uuid::Uuid::from_bytes(FAMILY).hyphenated().to_string();
    let lv_plist = format!(
        "<dict ID=\"0\">\
         <key>com.apple.corestorage.lv.familyUUID</key><string ID=\"1\">{fam_uuid}</string>\
         <key>com.apple.corestorage.lv.name</key><string ID=\"2\">SynthLV</string>\
         <key>com.apple.corestorage.lv.uuid</key><string ID=\"3\">00000000-0000-0000-0000-000000000001</string>\
         <key>com.apple.corestorage.lv.size</key><integer size=\"64\" ID=\"4\">0x4000</integer></dict>"
    );
    let lb = lv_plist.as_bytes();
    meta[OBJECT + 120..OBJECT + 124].copy_from_slice(&(lb.len() as u32).to_le_bytes());
    meta[OBJECT + 124..OBJECT + 128].copy_from_slice(&(lb.len() as u32).to_le_bytes());
    meta[OBJECT + 128..OBJECT + 132].copy_from_slice(&184u32.to_le_bytes());
    meta[OBJECT + 132..OBJECT + 136].copy_from_slice(&(lb.len() as u32).to_le_bytes());
    meta[OBJECT + 184..OBJECT + 184 + lb.len()].copy_from_slice(lb);

    // Block 2: 0x0305 segment descriptor (1 entry).
    let base = 2 * OBJECT;
    set_block_type(&mut meta[base..base + OBJECT], 0x0305);
    meta[base + 24..base + 32].copy_from_slice(&8u64.to_le_bytes());
    let payload = base + 64;
    meta[payload..payload + 4].copy_from_slice(&1u32.to_le_bytes());
    let entry = payload + 8;
    meta[entry + 8..entry + 16].copy_from_slice(&0u64.to_le_bytes()); // logical block
    meta[entry + 16..entry + 20].copy_from_slice(&LV_BLOCKS.to_le_bytes());
    let phys = LV_PHYS_BLOCK;
    meta[entry + 32..entry + 40].copy_from_slice(&phys.to_le_bytes());
    for object in meta.as_chunks_mut::<OBJECT>().0 {
        seal_object(object);
    }
    meta
}

/// Build a whole synthetic CoreStorage image (header + plaintext metadata +
/// encrypted metadata + encrypted LV) unlockable by [`PASSWORD`]. The LV
/// plaintext is the byte ramp `(i & 0xff)`.
#[must_use]
pub fn build_image() -> Vec<u8> {
    build_image_with_parameters(ITERS, "Complete")
}

/// Construct a checksum-valid image with controlled KDF and conversion status.
#[must_use]
pub fn build_image_with_parameters(iterations: u32, status: &str) -> Vec<u8> {
    // Layout (blocks): 0 header-block region, plaintext meta region at block 1,
    // encrypted meta at block 8, LV at block 16.
    let enc_meta_block: u64 = 8;
    let plaintext_meta_block: u64 = 1;

    // Decrypt-then-encrypt the metadata for the encrypted region.
    let plain_meta = build_decrypted_metadata(iterations, status);
    let mut enc_meta = plain_meta.clone();
    crate::xts::encrypt_units(&mut enc_meta, &KEY1, &PV_ID, 8192, 0);

    // Plaintext metadata region (one block): 0x0011 pointer.
    let meta_size = 8192u32; // region = 1 block
    let mut region = vec![0u8; 8192];
    set_block_type(&mut region, 0x0011);
    region[64..68].copy_from_slice(&meta_size.to_le_bytes()); // metadata_size@payload 0
                                                              // Descriptor offset (region-relative). Put descriptor at payload 200.
    let desc_off = 200u32;
    region[64 + 156..64 + 160].copy_from_slice(&desc_off.to_le_bytes());
    let d = desc_off as usize;
    region[d + 8..d + 16].copy_from_slice(&(plain_meta.len() as u64 / BLOCK as u64).to_le_bytes()); // count
    region[d + 32..d + 40].copy_from_slice(&enc_meta_block.to_le_bytes()); // primary block

    seal_object(&mut region);

    // LV plaintext -> encrypt with VMK/tweak_key.
    let tweak_key = {
        let mut h = Sha256::new();
        h.update(VMK);
        h.update(FAMILY);
        let dg = h.finalize();
        let mut tk = [0u8; 16];
        tk.copy_from_slice(&dg[..16]);
        tk
    };
    let lv_len = (LV_BLOCKS as usize) * BLOCK;
    let lv_plain: Vec<u8> = (0..lv_len).map(|i| (i & 0xff) as u8).collect();
    let mut lv_cipher = lv_plain.clone();
    crate::xts::encrypt_units(&mut lv_cipher, &VMK, &tweak_key, 512, 0);

    // Assemble the image.
    let lv_block: u64 = LV_PHYS_BLOCK;
    let total = (lv_block as usize + LV_BLOCKS as usize) * BLOCK;
    let mut image = vec![0u8; total];

    // Header at block 0.
    image[8..10].copy_from_slice(&1u16.to_le_bytes());
    image[10..12].copy_from_slice(&0x10u16.to_le_bytes());
    image[90..94].copy_from_slice(&1u32.to_le_bytes());
    image[94..96].copy_from_slice(&1u16.to_le_bytes());
    image[168..172].copy_from_slice(&16u32.to_le_bytes());
    image[88] = b'C';
    image[89] = b'S';
    image[48..52].copy_from_slice(&512u32.to_le_bytes());
    image[96..100].copy_from_slice(&(BLOCK as u32).to_le_bytes());
    image[64..72].copy_from_slice(&(total as u64).to_le_bytes());
    image[172..176].copy_from_slice(&2u32.to_le_bytes());
    image[104..112].copy_from_slice(&plaintext_meta_block.to_le_bytes());
    image[176..192].copy_from_slice(&KEY1);
    image[304..320].copy_from_slice(&PV_ID);

    // Plaintext metadata region at block 1.
    let po = plaintext_meta_block as usize * BLOCK;
    image[po..po + region.len()].copy_from_slice(&region);
    // Encrypted metadata at block 8.
    let eo = enc_meta_block as usize * BLOCK;
    image[eo..eo + enc_meta.len()].copy_from_slice(&enc_meta);
    // LV at block 16.
    let lo = lv_block as usize * BLOCK;
    image[lo..lo + lv_cipher.len()].copy_from_slice(&lv_cipher);

    image[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let crc = crate::storage_metadata::checksum(&image[8..512]);
    image[..4].copy_from_slice(&crc.to_le_bytes());
    image
}

pub fn seal_object(object: &mut [u8]) {
    object[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    object[8..10].copy_from_slice(&1u16.to_le_bytes());
    let size = object.len() as u32;
    object[48..52].copy_from_slice(&size.to_le_bytes());
    object[16..24].copy_from_slice(&1u64.to_le_bytes());
    let crc = crate::storage_metadata::checksum(&object[8..]);
    object[..4].copy_from_slice(&crc.to_le_bytes());
}
