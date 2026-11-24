//! The serializable outer header, readable without a key.

use serde::Serialize;

use kdbx::{Header, Kdf, Variant};

#[derive(Debug, Serialize)]
pub struct KdbxKdfReport {
    pub name: &'static str,
    pub uuid: String,
    /// AES-KDF.
    pub rounds: Option<u64>,
    /// The AES-KDF seed or the Argon2 salt, hex.
    pub salt: String,
    /// Argon2, in bytes.
    pub memory: Option<u64>,
    pub iterations: Option<u64>,
    pub parallelism: Option<u32>,
    pub version: Option<u32>,
    pub secret_key: bool,
    pub associated_data: bool,
}

#[derive(Debug, Serialize)]
pub struct KdbxVariantItem {
    pub key: String,
    pub kind: &'static str,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct KdbxHeaderField {
    pub id: u8,
    pub name: Option<&'static str>,
    pub offset: usize,
    pub length: usize,
}

/// The outer header.
#[derive(Debug, Serialize)]
pub struct KdbxHeaderReport {
    pub file_size: u64,
    pub version: String,
    pub pre_release: bool,
    pub cipher: &'static str,
    pub cipher_uuid: String,
    pub compression: &'static str,
    pub kdf: KdbxKdfReport,
    pub master_seed: String,
    pub encryption_iv: String,
    /// KDBX 3: the inner stream, its key and the stream start bytes.
    pub inner_stream: Option<&'static str>,
    pub protected_stream_key: Option<String>,
    pub stream_start_bytes: Option<String>,
    pub comment: Option<String>,
    pub public_custom_data: Vec<KdbxVariantItem>,
    pub length: usize,
    pub sha256: String,
    /// KDBX 4: whether the SHA-256 stored after the header matches.
    pub sha256_valid: Option<bool>,
    pub fields: Vec<KdbxHeaderField>,
    pub warnings: Vec<String>,
}

fn variant(item: &(String, Variant)) -> KdbxVariantItem {
    let (kind, value) = match &item.1 {
        Variant::UInt32(value) => ("UInt32", value.to_string()),
        Variant::UInt64(value) => ("UInt64", value.to_string()),
        Variant::Bool(value) => ("Bool", value.to_string()),
        Variant::Int32(value) => ("Int32", value.to_string()),
        Variant::Int64(value) => ("Int64", value.to_string()),
        Variant::String(value) => ("String", value.clone()),
        Variant::Bytes(value) => ("ByteArray", hex::encode(value)),
        Variant::Unknown { data, .. } => ("unknown", hex::encode(data)),
    };
    KdbxVariantItem {
        key: item.0.clone(),
        kind,
        value,
    }
}

fn kdf(kdf: &Kdf) -> KdbxKdfReport {
    let mut report = KdbxKdfReport {
        name: kdf.name(),
        uuid: hex::encode(kdf.uuid()),
        rounds: None,
        salt: hex::encode(kdf.salt()),
        memory: None,
        iterations: None,
        parallelism: None,
        version: None,
        secret_key: false,
        associated_data: false,
    };
    match kdf {
        Kdf::AesKdf { rounds, .. } => report.rounds = Some(*rounds),
        Kdf::Argon2(parameters) => {
            report.memory = Some(parameters.memory);
            report.iterations = Some(parameters.iterations);
            report.parallelism = Some(parameters.parallelism);
            report.version = Some(parameters.version);
            report.secret_key = parameters.secret.is_some();
            report.associated_data = parameters.associated_data.is_some();
        }
        Kdf::Unknown { .. } => {}
    }
    report
}

pub fn header_report(header: &Header, file_size: u64) -> KdbxHeaderReport {
    KdbxHeaderReport {
        file_size,
        version: header.version.to_string(),
        pre_release: header.pre_release,
        cipher: header.cipher.name(),
        cipher_uuid: hex::encode(header.cipher.uuid()),
        compression: header.compression.name(),
        kdf: kdf(&header.kdf),
        master_seed: hex::encode(&header.master_seed),
        encryption_iv: hex::encode(&header.encryption_iv),
        inner_stream: header.inner_stream.map(|stream| stream.name()),
        protected_stream_key: header.protected_stream_key.as_deref().map(hex::encode),
        stream_start_bytes: header.stream_start_bytes.as_deref().map(hex::encode),
        comment: header
            .comment
            .as_deref()
            .map(|comment| String::from_utf8_lossy(comment).into_owned()),
        public_custom_data: header
            .public_custom_data
            .iter()
            .flat_map(|dictionary| dictionary.items.iter().map(variant))
            .collect(),
        length: header.length,
        sha256: hex::encode(header.sha256),
        sha256_valid: header.sha256_matches(),
        fields: header
            .fields
            .iter()
            .map(|field| KdbxHeaderField {
                id: field.id,
                name: field.name(header.version),
                offset: field.offset,
                length: field.data.len(),
            })
            .collect(),
        warnings: header.warnings.clone(),
    }
}
