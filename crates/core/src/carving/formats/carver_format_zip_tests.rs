use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Clone)]
struct Bytes(Arc<Vec<u8>>);

impl CarveSource for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
        let start = (offset as usize).min(self.0.len());
        let length = destination.len().min(self.0.len() - start);
        destination[..length].copy_from_slice(&self.0[start..start + length]);
        Ok(length)
    }

    fn worker_handle(&self) -> Self {
        self.clone()
    }
}

fn carve(bytes: &[u8]) -> Option<(u64, &'static str)> {
    let source = Bytes(Arc::new(bytes.to_vec()));
    let cancellation = CancellationToken::new();
    let mut lookahead = Vec::new();
    let mut reader = CarveReader::new(
        &source,
        0,
        bytes.len() as u64,
        &bytes[..bytes.len().min(512)],
        &mut lookahead,
        &cancellation,
    );
    scan_zip(&mut reader)
        .ok()
        .flatten()
        .map(|result| (result.size, result.extension.unwrap_or("zip")))
}

#[derive(Clone, Copy, PartialEq)]
enum Sizes {
    /// Sizes in the local header.
    Header,
    /// Streamed: zero sizes, then a descriptor with its signature.
    Signed,
    /// Streamed with a descriptor that omits the signature.
    Unsigned,
    /// Streamed ZIP64: 8-byte sizes in the descriptor.
    Zip64Signed,
    /// Saturated 32-bit sizes resolved by the ZIP64 extra field.
    Zip64Header,
}

struct Member {
    name: String,
    data: Vec<u8>,
    sizes: Sizes,
}

fn member(name: &str, data: &[u8], sizes: Sizes) -> Member {
    Member {
        name: name.to_string(),
        data: data.to_vec(),
        sizes,
    }
}

fn archive(members: &[Member], apk_signing_block: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for member in members {
        let length = member.data.len() as u32;
        let streamed = matches!(
            member.sizes,
            Sizes::Signed | Sizes::Unsigned | Sizes::Zip64Signed
        );
        let (header_size, extra) = match member.sizes {
            Sizes::Header => (length, Vec::new()),
            Sizes::Signed | Sizes::Unsigned => (0, Vec::new()),
            Sizes::Zip64Signed => (0, zip64_extra(0)),
            Sizes::Zip64Header => (u32::MAX, zip64_extra(u64::from(length))),
        };
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0]);
        out.extend_from_slice(&(if streamed { 8u16 } else { 0 }).to_le_bytes());
        out.extend_from_slice(&[0u8; 10]);
        out.extend_from_slice(&header_size.to_le_bytes());
        out.extend_from_slice(&header_size.to_le_bytes());
        out.extend_from_slice(&(member.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        out.extend_from_slice(member.name.as_bytes());
        out.extend_from_slice(&extra);
        out.extend_from_slice(&member.data);
        match member.sizes {
            Sizes::Signed | Sizes::Unsigned => {
                if member.sizes == Sizes::Signed {
                    out.extend_from_slice(b"PK\x07\x08");
                }
                out.extend_from_slice(&[0u8; 4]);
                out.extend_from_slice(&length.to_le_bytes());
                out.extend_from_slice(&length.to_le_bytes());
            }
            Sizes::Zip64Signed => {
                out.extend_from_slice(b"PK\x07\x08");
                out.extend_from_slice(&[0u8; 4]);
                out.extend_from_slice(&u64::from(length).to_le_bytes());
                out.extend_from_slice(&u64::from(length).to_le_bytes());
            }
            Sizes::Header | Sizes::Zip64Header => {}
        }
    }
    if apk_signing_block {
        out.extend_from_slice(&40u64.to_le_bytes());
        out.extend_from_slice(&8u64.to_le_bytes());
        out.extend_from_slice(&0x7109_871au32.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&40u64.to_le_bytes());
        out.extend_from_slice(APK_SIGNING_BLOCK_MAGIC);
    }
    for member in members {
        out.extend_from_slice(b"PK\x01\x02");
        let mut fixed = [0u8; 42];
        fixed[24..26].copy_from_slice(&(member.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&fixed);
        out.extend_from_slice(member.name.as_bytes());
    }
    out.extend_from_slice(b"PK\x05\x06");
    let mut end = [0u8; 18];
    end[16..18].copy_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&end);
    out.extend_from_slice(b"hi!");
    out
}

fn zip64_extra(compressed: u64) -> Vec<u8> {
    let mut extra = ZIP64_EXTRA.to_le_bytes().to_vec();
    extra.extend_from_slice(&16u16.to_le_bytes());
    extra.extend_from_slice(&compressed.to_le_bytes());
    extra.extend_from_slice(&compressed.to_le_bytes());
    extra
}

fn with_slack(archive: &[u8]) -> Vec<u8> {
    let mut evidence = archive.to_vec();
    evidence.extend_from_slice(&[0x5a; 700]);
    evidence
}

#[test]
fn stored_members_carve_to_the_end_record_comment() {
    let zip = archive(
        &[
            member("a.txt", b"alpha", Sizes::Header),
            member("b.txt", b"bravo", Sizes::Header),
        ],
        false,
    );
    assert_eq!(carve(&with_slack(&zip)), Some((zip.len() as u64, "zip")));
}

#[test]
fn central_directory_longer_than_the_old_search_window() {
    let members = (0..1200)
        .map(|index| {
            member(
                &format!("com/example/generated/Class{index:05}.class"),
                b"x",
                Sizes::Header,
            )
        })
        .collect::<Vec<_>>();
    let zip = archive(&members, false);
    assert!(zip.len() > 66 * 1024);
    assert_eq!(carve(&with_slack(&zip)), Some((zip.len() as u64, "zip")));
}

#[test]
fn streamed_members_find_their_descriptors() {
    // The data contains a descriptor signature whose size does not match.
    let mut data = b"compressed-bytes".to_vec();
    data.extend_from_slice(b"PK\x07\x08\0\0\0\0\x05\0\0\0\x05\0\0\0");
    data.extend_from_slice(b"more");
    for sizes in [Sizes::Signed, Sizes::Unsigned, Sizes::Zip64Signed] {
        let zip = archive(
            &[
                member("a.bin", &data, sizes),
                member("b.bin", b"tail", sizes),
            ],
            false,
        );
        assert_eq!(
            carve(&with_slack(&zip)),
            Some((zip.len() as u64, "zip")),
            "descriptor layout"
        );
    }
}

#[test]
fn zip64_extra_field_resolves_saturated_sizes() {
    let zip = archive(&[member("big.bin", &[7u8; 300], Sizes::Zip64Header)], false);
    assert_eq!(carve(&with_slack(&zip)), Some((zip.len() as u64, "zip")));
}

#[test]
fn apk_signing_block_and_spanning_marker_are_accepted() {
    let apk = archive(
        &[
            member("AndroidManifest.xml", b"<manifest/>", Sizes::Header),
            member("classes.dex", b"dex\n035", Sizes::Signed),
        ],
        true,
    );
    assert_eq!(carve(&with_slack(&apk)), Some((apk.len() as u64, "apk")));

    let mut spanned = b"PK00".to_vec();
    spanned.extend_from_slice(&archive(&[member("a.txt", b"a", Sizes::Header)], false));
    assert_eq!(
        carve(&with_slack(&spanned)),
        Some((spanned.len() as u64, "zip"))
    );
}

/// Member names and payloads of one archive.
type Members<'a> = &'a [(&'a str, &'a [u8])];

#[test]
fn member_names_identify_package_formats() {
    let cases: [(Members, &str); 5] = [
        (&[("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0")], "jar"),
        (
            &[
                ("mimetype", b"application/vnd.oasis.opendocument.text"),
                ("content.xml", b"<x/>"),
            ],
            "odt",
        ),
        (
            &[
                ("mimetype", b"application/epub+zip"),
                ("OEBPS/a.xhtml", b"<x/>"),
            ],
            "epub",
        ),
        (
            &[
                ("pkg/__init__.py", b""),
                ("pkg-1.0.dist-info/WHEEL", b"Wheel-Version: 1.0"),
            ],
            "whl",
        ),
        (
            &[
                ("[Content_Types].xml", b"<Types/>"),
                ("_rels/.rels", b"<Relationships/>"),
                ("word/document.xml", b"<w:document/>"),
            ],
            "docx",
        ),
    ];
    for (members, expected) in cases {
        let members = members
            .iter()
            .map(|(name, data)| member(name, data, Sizes::Header))
            .collect::<Vec<_>>();
        let zip = archive(&members, false);
        assert_eq!(carve(&with_slack(&zip)), Some((zip.len() as u64, expected)));
    }
}

#[test]
fn archive_without_its_end_record_is_rejected() {
    let zip = archive(&[member("a.txt", b"alpha", Sizes::Header)], false);
    let truncated = &zip[..zip.len() - 10];
    assert_eq!(carve(truncated), None);
}

#[test]
fn a_member_inside_a_larger_archive_is_not_an_archive() {
    let zip = archive(
        &[
            member("a.txt", &[1u8; 700], Sizes::Header),
            member("b.txt", &[2u8; 700], Sizes::Header),
        ],
        false,
    );
    let second_member = 30 + 5 + 700;
    assert_eq!(&zip[second_member..second_member + 4], b"PK\x03\x04");
    assert_eq!(carve(&with_slack(&zip[second_member..])), None);
}
