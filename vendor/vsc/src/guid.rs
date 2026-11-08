//! VSS identifier GUID and canonical GUID formatting.
//!
//! GUID → string is a solved primitive, so we reuse the `uuid` crate rather than
//! hand-roll a mixed-endian formatter (which `dpapi-forensic` and
//! `winevt-binxml` each do separately — a fleet cleanup opportunity). VSS/Windows
//! GUIDs store the first three fields little-endian and the last eight big-endian,
//! which is exactly [`uuid::Uuid::from_bytes_le`].

/// The VSS identifier GUID `3808876b-c176-4e48-b7ae-04046e6cc752`, in its
/// on-disk (mixed-endian) byte layout. Present at offset 0 of the VSS volume
/// header, every catalog block header, and every store block header.
pub const VSS_IDENTIFIER: [u8; 16] = [
    0x6b, 0x87, 0x08, 0x38, // Data1 = 0x3808876b (LE)
    0x76, 0xc1, // Data2 = 0xc176 (LE)
    0x48, 0x4e, // Data3 = 0x4e48 (LE)
    0xb7, 0xae, 0x04, 0x04, 0x6e, 0x6c, 0xc7, 0x52, // Data4 (big-endian)
];

/// Render a 16-byte Microsoft/VSS GUID in canonical `8-4-4-4-12` form.
///
/// Delegates to [`uuid::Uuid::from_bytes_le`], whose little-endian-first-three-
/// fields layout matches the on-disk Windows GUID encoding; the result matches
/// the rendering libvshadow/`pyvshadow` produce for store identifiers.
#[must_use]
pub fn format_guid(raw: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes_le(*raw).to_string()
}
