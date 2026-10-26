//! Extended attributes: the values of the xattrs btree.
//!
//! Provenance (docs/clean-room.md): the xattrs btree is documented (S1,
//! 11.3) as holding extended attributes, snapshot-aware. Each attribute is
//! one key of type `xattr` (11, S1's key type order) at
//! `inode:hash:snapshot`. The value layout was found by hexdump of the aged
//! fixture against the reference lister's `name:value` output and the
//! mount's own listing (S3, S4, S8):
//!
//! ```text
//! namespace u8, name length u8, value length le16, name (without its
//! namespace prefix), value, zero padding to a whole u64
//! ```

use crate::bkey::{key_type, Bkey};
use crate::error::{Error, Result};

/// An xattr's offset in its inode's run of the xattrs btree: SipHash-2-4
/// keyed `(hash_seed, 0)` over the namespace byte then the name, shifted
/// right by one, except that when that message is longer than 8 bytes and
/// not a whole number of 8-byte words, its final partial word is a zero
/// byte followed by all but the last of its bytes, so the message's last
/// byte is not hashed. INFERRED (#77) from one xattr of every name length
/// from 1 to 24 set through the reference mount, and checked against every
/// xattr of the write study's xattr images and all 45 of `aged`
/// (tests/oracle_xattr_slots.rs, docs/clean-room.md).
pub fn name_slot(hash_seed: u64, namespace: u8, name: &[u8]) -> u64 {
    let mut msg = Vec::with_capacity(1 + name.len());
    msg.push(namespace);
    msg.extend_from_slice(name);
    let (whole, tail) = (msg.len() / 8 * 8, msg.len() % 8);
    if msg.len() > 8 && tail != 0 {
        msg.insert(whole, 0);
        msg.pop();
    }
    crate::siphash::siphash24(hash_seed, 0, &msg) >> 1
}

/// The slot of an xattr on an inode hashed with `hash_type` (#106):
/// SipHash as [`name_slot`]; crc32c as the dirents' crc32c hash
/// (`inode::name_hash`) over the namespace byte then the name, with no
/// rearranged final word. OBSERVED (S8): every xattr of the write study's
/// xcollide images, 1 to 24 bytes, sits at that hash, and the four names
/// that collide under it take its slot and the three after it, in the
/// order they were set (tests/oracle_xattr_slots.rs). Any other hash type
/// has no known slot.
pub fn slot(hash_type: u8, hash_seed: u64, namespace: u8, name: &[u8]) -> Option<u64> {
    match hash_type {
        crate::inode::HASH_TYPE_SIPHASH => Some(name_slot(hash_seed, namespace, name)),
        crate::inode::HASH_TYPE_CRC32C => {
            let mut msg = Vec::with_capacity(1 + name.len());
            msg.push(namespace);
            msg.extend_from_slice(name);
            crate::inode::name_hash(hash_type, hash_seed, &msg)
        }
        _ => None,
    }
}

/// One extended attribute: its full name, namespace prefix included
/// (`user.greeting`), and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xattr {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// Namespace numbers and the prefix a name carries in each. INFERRED from
/// the aged fixture: `user.*` attributes store 0 and `trusted.*` store 3.
/// Others (security, the POSIX ACLs) have not been seen and are refused
/// rather than guessed.
const NAMESPACES: &[(u8, &str)] = &[(0, "user."), (3, "trusted.")];

impl Xattr {
    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::XATTR {
            return Err(Error::Unsupported(format!("xattr key type {}", k.key_type)));
        }
        let v = &k.value;
        if v.len() < 4 {
            return Err(Error::Corrupt("xattr value shorter than its header".into()));
        }
        let (ns, name_len, val_len) = (
            v[0],
            v[1] as usize,
            u16::from_le_bytes([v[2], v[3]]) as usize,
        );
        let end = 4 + name_len + val_len;
        if end > v.len() {
            return Err(Error::Corrupt(format!(
                "xattr of {name_len}+{val_len} bytes in a {}-byte value",
                v.len()
            )));
        }
        let prefix = NAMESPACES
            .iter()
            .find(|(n, _)| *n == ns)
            .map(|(_, p)| *p)
            .ok_or_else(|| Error::Unsupported(format!("xattr namespace {ns}")))?;
        let mut name = prefix.as_bytes().to_vec();
        name.extend_from_slice(&v[4..4 + name_len]);
        Ok(Xattr {
            name,
            value: v[4 + name_len..end].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bkey::Bpos;

    #[test]
    fn name_slots_are_the_offsets_the_lister_showed() {
        // The write study's xattr images: the seed, then the offset the
        // lister printed (user namespace, 0).
        for (seed, name, offset) in [
            // A 7-byte message: hashed as it is.
            (
                0xc4a6_d082_99d7_882e_u64,
                &b"kkkkkk"[..],
                6_920_644_051_664_158_609,
            ),
            // 9 bytes: the last is not hashed.
            (
                0x4048_1ff5_c1b3_7408,
                b"greeting",
                5_470_492_547_012_498_347,
            ),
            // 15 bytes: a zero, then "rector" without the final "y".
            (
                0xa7cf_829b_18ef_4f8a,
                b"on-a-directory",
                8_914_961_747_488_599_387,
            ),
            // 16 bytes, two whole words: hashed as it is.
            (
                0xc4a6_d082_99d7_882e,
                b"kkkkkkkkkkkkkkk",
                6_418_509_822_616_553_518,
            ),
        ] {
            assert_eq!(
                name_slot(seed, 0, name),
                offset,
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }

    fn key(value: Vec<u8>) -> Bkey {
        Bkey {
            key_type: key_type::XATTR,
            version_hi: 0,
            version_lo: 0,
            size: 0,
            pos: Bpos::default(),
            value,
        }
    }

    #[test]
    fn a_user_attribute_decodes_with_its_prefix() {
        // Bytes as the aged fixture stores user.greeting = "hello".
        let mut v = vec![0, 8, 5, 0];
        v.extend_from_slice(b"greetinghello");
        v.resize(24, 0);
        let x = Xattr::from_key(&key(v)).unwrap();
        assert_eq!(x.name, b"user.greeting");
        assert_eq!(x.value, b"hello");
    }

    #[test]
    fn a_length_past_the_value_is_corrupt_and_an_unknown_namespace_is_refused() {
        assert!(matches!(
            Xattr::from_key(&key(vec![0, 8, 200, 0, 0, 0, 0, 0])),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            Xattr::from_key(&key(vec![9, 1, 1, 0, b'a', b'b', 0, 0])),
            Err(Error::Unsupported(_))
        ));
    }
}
