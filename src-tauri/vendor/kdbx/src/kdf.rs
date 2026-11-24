//! Key derivation: AES-KDF (KDBX 3 and 4) and Argon2d / Argon2id (KDBX 4).

use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::bytes::fixed;
use crate::error::{Error, Result};
use crate::variant::{Variant, VariantDictionary};
use crate::Limits;

/// The Argon2 variant a KDBX 4 header names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Argon2Variant {
    D,
    Id,
}

/// Argon2 parameters as a KDBX 4 header stores them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argon2Parameters {
    pub variant: Argon2Variant,
    pub salt: Vec<u8>,
    pub parallelism: u32,
    /// Memory in bytes; Argon2 uses `memory / 1024` KiB blocks.
    pub memory: u64,
    pub iterations: u64,
    /// `0x10` or `0x13`.
    pub version: u32,
    /// Optional secret key (`K`).
    pub secret: Option<Vec<u8>>,
    /// Optional associated data (`A`).
    pub associated_data: Option<Vec<u8>>,
}

/// The key derivation a header names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kdf {
    /// AES-256-ECB applied `rounds` times to the composite key, then SHA-256.
    AesKdf {
        rounds: u64,
        seed: Vec<u8>,
    },
    Argon2(Argon2Parameters),
    /// A KDF UUID this reader does not implement.
    Unknown {
        uuid: [u8; 16],
    },
}

impl Kdf {
    pub const AES_KDF_UUID: [u8; 16] = [
        0xc9, 0xd9, 0xf3, 0x9a, 0x62, 0x8a, 0x44, 0x60, 0xbf, 0x74, 0x0d, 0x08, 0xc1, 0x8a, 0x4f,
        0xea,
    ];
    pub const ARGON2D_UUID: [u8; 16] = [
        0xef, 0x63, 0x6d, 0xdf, 0x8c, 0x29, 0x44, 0x4b, 0x91, 0xf7, 0xa9, 0xa4, 0x03, 0xe3, 0x0a,
        0x0c,
    ];
    pub const ARGON2ID_UUID: [u8; 16] = [
        0x9e, 0x29, 0x8b, 0x19, 0x56, 0xdb, 0x47, 0x73, 0xb2, 0x3d, 0xfc, 0x3e, 0xc6, 0xf0, 0xa1,
        0xe6,
    ];

    /// The KDF of a KDBX 4 `KdfParameters` header field.
    pub fn from_dictionary(dictionary: &VariantDictionary) -> Result<Self> {
        let uuid = match dictionary.get("$UUID") {
            Some(Variant::Bytes(bytes)) => {
                fixed::<16>(bytes).ok_or_else(|| Error::Malformed("KDF UUID length".into()))?
            }
            _ => return Err(Error::Malformed("KDF parameters without $UUID".into())),
        };
        Ok(match uuid {
            Self::AES_KDF_UUID => Self::AesKdf {
                rounds: uint64(dictionary, "R")?,
                seed: bytes(dictionary, "S")?,
            },
            Self::ARGON2D_UUID | Self::ARGON2ID_UUID => Self::Argon2(Argon2Parameters {
                variant: if uuid == Self::ARGON2D_UUID {
                    Argon2Variant::D
                } else {
                    Argon2Variant::Id
                },
                salt: bytes(dictionary, "S")?,
                parallelism: uint32(dictionary, "P")?,
                memory: uint64(dictionary, "M")?,
                iterations: uint64(dictionary, "I")?,
                version: uint32(dictionary, "V")?,
                secret: optional_bytes(dictionary, "K")?,
                associated_data: optional_bytes(dictionary, "A")?,
            }),
            uuid => Self::Unknown { uuid },
        })
    }

    pub fn uuid(&self) -> [u8; 16] {
        match self {
            Self::AesKdf { .. } => Self::AES_KDF_UUID,
            Self::Argon2(Argon2Parameters {
                variant: Argon2Variant::D,
                ..
            }) => Self::ARGON2D_UUID,
            Self::Argon2(Argon2Parameters {
                variant: Argon2Variant::Id,
                ..
            }) => Self::ARGON2ID_UUID,
            Self::Unknown { uuid } => *uuid,
        }
    }

    /// The KDF's display name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AesKdf { .. } => "AES-KDF",
            Self::Argon2(Argon2Parameters {
                variant: Argon2Variant::D,
                ..
            }) => "Argon2d",
            Self::Argon2(Argon2Parameters {
                variant: Argon2Variant::Id,
                ..
            }) => "Argon2id",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// The seed or salt (pykeepass' `database_salt`).
    pub fn salt(&self) -> &[u8] {
        match self {
            Self::AesKdf { seed, .. } => seed,
            Self::Argon2(parameters) => &parameters.salt,
            Self::Unknown { .. } => &[],
        }
    }

    /// Derive the transformed key from a composite key.
    pub fn transform(&self, composite: &[u8; 32], limits: &Limits) -> Result<Zeroizing<[u8; 32]>> {
        match self {
            Self::AesKdf { rounds, seed } => aes_kdf(composite, seed, *rounds, limits),
            Self::Argon2(parameters) => argon2(composite, parameters, limits),
            Self::Unknown { uuid } => Err(Error::Unsupported(format!(
                "key derivation {}",
                crate::bytes::hex(uuid)
            ))),
        }
    }
}

fn parameter<'a>(dictionary: &'a VariantDictionary, key: &str) -> Result<&'a Variant> {
    dictionary
        .get(key)
        .ok_or_else(|| Error::Malformed(format!("KDF parameter {key} missing")))
}

fn wrong_type(key: &str) -> Error {
    Error::Malformed(format!("KDF parameter {key} has the wrong type"))
}

fn uint32(dictionary: &VariantDictionary, key: &str) -> Result<u32> {
    match parameter(dictionary, key)? {
        Variant::UInt32(value) => Ok(*value),
        _ => Err(wrong_type(key)),
    }
}

fn uint64(dictionary: &VariantDictionary, key: &str) -> Result<u64> {
    match parameter(dictionary, key)? {
        Variant::UInt64(value) => Ok(*value),
        _ => Err(wrong_type(key)),
    }
}

fn bytes(dictionary: &VariantDictionary, key: &str) -> Result<Vec<u8>> {
    match parameter(dictionary, key)? {
        Variant::Bytes(value) => Ok(value.clone()),
        _ => Err(wrong_type(key)),
    }
}

fn optional_bytes(dictionary: &VariantDictionary, key: &str) -> Result<Option<Vec<u8>>> {
    match dictionary.get(key) {
        None => Ok(None),
        Some(Variant::Bytes(value)) => Ok(Some(value.clone())),
        Some(_) => Err(wrong_type(key)),
    }
}

/// KeePass hashes a seed or message that is not 32 bytes long.
fn key32(data: &[u8]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(fixed(data).unwrap_or_else(|| Sha256::digest(data).into()))
}

fn aes_kdf(
    composite: &[u8; 32],
    seed: &[u8],
    rounds: u64,
    limits: &Limits,
) -> Result<Zeroizing<[u8; 32]>> {
    if rounds > limits.max_aes_kdf_rounds {
        return Err(Error::Limit(format!(
            "AES-KDF rounds {rounds} exceed {}",
            limits.max_aes_kdf_rounds
        )));
    }
    let seed = key32(seed);
    let cipher = aes::Aes256::new(GenericArray::from_slice(&seed[..]));
    let mut blocks = [
        *GenericArray::from_slice(&composite[..16]),
        *GenericArray::from_slice(&composite[16..]),
    ];
    for _ in 0..rounds {
        cipher.encrypt_blocks(&mut blocks);
    }
    let digest = Sha256::new()
        .chain_update(blocks[0])
        .chain_update(blocks[1])
        .finalize();
    for block in &mut blocks {
        block.as_mut_slice().zeroize();
    }
    Ok(Zeroizing::new(digest.into()))
}

fn argon2(
    composite: &[u8; 32],
    parameters: &Argon2Parameters,
    limits: &Limits,
) -> Result<Zeroizing<[u8; 32]>> {
    if parameters.memory > limits.max_argon2_memory {
        return Err(Error::Limit(format!(
            "Argon2 memory {} bytes exceeds {}",
            parameters.memory, limits.max_argon2_memory
        )));
    }
    if parameters.memory.saturating_mul(parameters.iterations) > limits.max_argon2_work {
        return Err(Error::Limit(format!(
            "Argon2 work {} bytes × {} passes exceeds {}",
            parameters.memory, parameters.iterations, limits.max_argon2_work
        )));
    }
    let version = match parameters.version {
        0x10 => argon2::Version::V0x10,
        0x13 => argon2::Version::V0x13,
        version => {
            return Err(Error::Unsupported(format!("Argon2 version {version:#x}")));
        }
    };
    let invalid = |error: argon2::Error| Error::Malformed(format!("Argon2 parameters: {error}"));
    let memory_kib = u32::try_from(parameters.memory / 1024)
        .map_err(|_| Error::Malformed("Argon2 memory".into()))?;
    let iterations = u32::try_from(parameters.iterations)
        .map_err(|_| Error::Malformed("Argon2 iterations".into()))?;
    let mut builder = argon2::ParamsBuilder::new();
    builder
        .m_cost(memory_kib)
        .t_cost(iterations)
        .p_cost(parameters.parallelism)
        .output_len(32);
    if let Some(data) = &parameters.associated_data {
        builder.data(argon2::AssociatedData::new(data).map_err(|_| {
            Error::Unsupported(format!(
                "Argon2 associated data of {} bytes (at most {} are supported)",
                data.len(),
                argon2::AssociatedData::MAX_LEN
            ))
        })?);
    }
    let params = builder.build().map_err(invalid)?;
    let algorithm = match parameters.variant {
        Argon2Variant::D => argon2::Algorithm::Argon2d,
        Argon2Variant::Id => argon2::Algorithm::Argon2id,
    };
    let context = match &parameters.secret {
        Some(secret) => argon2::Argon2::new_with_secret(secret, algorithm, version, params.clone())
            .map_err(invalid)?,
        None => argon2::Argon2::new(algorithm, version, params.clone()),
    };
    // Allocate fallibly: a header may ask for more memory than the host has.
    let count = params.block_count();
    let mut blocks: Vec<argon2::Block> = Vec::new();
    blocks.try_reserve_exact(count).map_err(|_| {
        Error::Limit(format!(
            "Argon2 memory of {} bytes cannot be allocated",
            parameters.memory
        ))
    })?;
    blocks.resize(count, argon2::Block::default());
    let mut out = Zeroizing::new([0u8; 32]);
    context
        .hash_password_into_with_memory(composite, &parameters.salt, &mut out[..], &mut blocks)
        .map_err(invalid)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composite() -> [u8; 32] {
        std::array::from_fn(|index| index as u8)
    }

    fn hex(data: &[u8]) -> String {
        crate::bytes::hex(data)
    }

    // Expected values from Python 3.10: `cryptography` AES-256-ECB plus
    // hashlib.sha256, and argon2-cffi `hash_secret_raw`.
    #[test]
    fn aes_kdf_matches_an_independent_implementation() {
        let seed: Vec<u8> = (0..32).map(|index| 0xa0 ^ index as u8).collect();
        let kdf = Kdf::AesKdf { rounds: 1000, seed };
        let key = kdf.transform(&composite(), &Limits::default()).unwrap();
        assert_eq!(
            hex(&key[..]),
            "1d4a1269a1e2246d1aa42b8d1a5f05159b19bb0f5a99d6b83cfcf8064866287c"
        );
    }

    #[test]
    fn argon2_matches_an_independent_implementation() {
        let parameters = |variant, version| {
            Kdf::Argon2(Argon2Parameters {
                variant,
                salt: b"LovelyCarve-salt".to_vec(),
                parallelism: 2,
                memory: 64 * 1024,
                iterations: 3,
                version,
                secret: None,
                associated_data: None,
            })
        };
        let limits = Limits::default();
        let d = parameters(Argon2Variant::D, 0x13)
            .transform(&composite(), &limits)
            .unwrap();
        let id = parameters(Argon2Variant::Id, 0x13)
            .transform(&composite(), &limits)
            .unwrap();
        let d10 = parameters(Argon2Variant::D, 0x10)
            .transform(&composite(), &limits)
            .unwrap();
        assert_eq!(
            hex(&d[..]),
            "ac8b74922f6eb52700d57c7b6dac116b677ce0e0ae666366f8b784b0c5a5b79d"
        );
        assert_eq!(
            hex(&id[..]),
            "bcd5b3499f5680c15be6469aa239bc9e8de59ccaca3d4c1f6d30aaf3b1f762c3"
        );
        assert_eq!(
            hex(&d10[..]),
            "0d7c2c2422493e45e973422f3661570a7ed448f2d4fe70295d8312ec06b0d09d"
        );
    }

    #[test]
    fn limits_refuse_oversized_work_before_computing() {
        let limits = Limits {
            max_aes_kdf_rounds: 10,
            max_argon2_memory: 1 << 20,
            ..Limits::default()
        };
        let aes = Kdf::AesKdf {
            rounds: 11,
            seed: vec![0; 32],
        };
        assert!(matches!(
            aes.transform(&composite(), &limits),
            Err(Error::Limit(_))
        ));
        let argon = Kdf::Argon2(Argon2Parameters {
            variant: Argon2Variant::Id,
            salt: vec![1; 16],
            parallelism: 1,
            memory: 2 << 20,
            iterations: 1,
            version: 0x13,
            secret: None,
            associated_data: None,
        });
        assert!(matches!(
            argon.transform(&composite(), &limits),
            Err(Error::Limit(_))
        ));
    }
}
