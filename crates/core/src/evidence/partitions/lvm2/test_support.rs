//! Synthetic LVM2 fixtures. No third-party disk images or evidence bytes.
use super::header::checksum;

pub const PV_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
pub const OTHER_PV_ID: &str = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";
pub const PE_START: usize = 64 * 1024;
pub const MDA_OFFSET: usize = 4096;
pub const MDA_SIZE: usize = 32 * 1024;

pub fn text() -> String {
    format!(
        r#"
contents = "Text Format Volume Group"
version = 1
description = "comment # with {{braces}} and physical_volumes"
vg {{
    id = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
    seqno = 7
    extent_size = 8
    physical_volumes {{
        pv0 {{ id = "{OTHER_PV_ID}" device = "/dev/decoy" dev_size = 256 pe_start = 128 pe_count = 16 }}
        pv1 {{ id = "{PV_ID}" device = "/dev/evidence" dev_size = 256 pe_start = 128 pe_count = 16 }}
    }}
    logical_volumes {{
        root {{
            id = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
            segment_count = 2
            # Numeric segment names need not follow logical order.
            segment2 {{ start_extent = 0 extent_count = 2 type = "striped" stripe_count = 1 stripes = [ "pv1", 12 ] }}
            segment1 {{ start_extent = 2 extent_count = 2 type = "striped" stripe_count = 1 stripes = [ "pv1", 0 ] }}
        }}
    }}
}}
"#
    )
}

pub fn put_label(bytes: &mut [u8], sector: usize, areas: &[(usize, usize)]) {
    let size = bytes.len() as u64;
    let label = &mut bytes[sector * 512..sector * 512 + 512];
    label.fill(0);
    label[..8].copy_from_slice(b"LABELONE");
    label[8..16].copy_from_slice(&(sector as u64).to_le_bytes());
    label[20..24].copy_from_slice(&32u32.to_le_bytes());
    label[24..32].copy_from_slice(b"LVM2 001");
    label[32..64].copy_from_slice(PV_ID.as_bytes());
    label[64..72].copy_from_slice(&size.to_le_bytes());
    label[72..80].copy_from_slice(&(PE_START as u64).to_le_bytes());
    for (index, (offset, size)) in areas.iter().enumerate() {
        let start = 104 + index * 16;
        label[start..start + 8].copy_from_slice(&(*offset as u64).to_le_bytes());
        label[start + 8..start + 16].copy_from_slice(&(*size as u64).to_le_bytes());
    }
    let crc = checksum(&label[20..]);
    label[16..20].copy_from_slice(&crc.to_le_bytes());
}

pub fn put_metadata(bytes: &mut [u8], area: usize, size: usize, offset: usize, text: &[u8]) {
    let first = text.len().min(size - offset);
    bytes[area + offset..area + offset + first].copy_from_slice(&text[..first]);
    if first < text.len() {
        bytes[area + 512..area + 512 + text.len() - first].copy_from_slice(&text[first..]);
    }
    let header = &mut bytes[area..area + 512];
    header.fill(0);
    header[4..20].copy_from_slice(b" LVM2 x[5A%r0N*>");
    header[20..24].copy_from_slice(&1u32.to_le_bytes());
    header[24..32].copy_from_slice(&(area as u64).to_le_bytes());
    header[32..40].copy_from_slice(&(size as u64).to_le_bytes());
    header[40..48].copy_from_slice(&(offset as u64).to_le_bytes());
    header[48..56].copy_from_slice(&(text.len() as u64).to_le_bytes());
    header[56..60].copy_from_slice(&checksum(text).to_le_bytes());
    refresh_mda_checksum(bytes, area);
}

pub fn refresh_mda_checksum(bytes: &mut [u8], area: usize) {
    let crc = checksum(&bytes[area + 4..area + 512]);
    bytes[area..area + 4].copy_from_slice(&crc.to_le_bytes());
}

pub fn image(text: &str, sector: usize, wrap: bool) -> Vec<u8> {
    let mut bytes = vec![0; 128 * 1024];
    bytes[PE_START..PE_START + 8192].fill(0x22);
    bytes[PE_START + 12 * 4096..PE_START + 14 * 4096].fill(0x11);
    put_label(&mut bytes, sector, &[(MDA_OFFSET, MDA_SIZE)]);
    put_metadata(
        &mut bytes,
        MDA_OFFSET,
        MDA_SIZE,
        if wrap { MDA_SIZE - 64 } else { 512 },
        text.as_bytes(),
    );
    bytes
}

/// Wrap a logical stream in two reversed, non-adjacent PE runs.
pub fn fragmented(bytes: &[u8]) -> Vec<u8> {
    let size = bytes.len().next_multiple_of(8192);
    let half = size / 2;
    let first_extent = half / 4096 + 4;
    let pe_count = size / 4096 + 4;
    let pv_size = PE_START + pe_count * 4096;
    let metadata = text()
        .replace("dev_size = 256", &format!("dev_size = {}", pv_size / 512))
        .replace("pe_count = 16", &format!("pe_count = {pe_count}"))
        .replace(
            "extent_count = 2",
            &format!("extent_count = {}", half / 4096),
        )
        .replace(
            "start_extent = 2",
            &format!("start_extent = {}", half / 4096),
        )
        .replace("\"pv1\", 12", &format!("\"pv1\", {first_extent}"));
    let mut image = vec![0; pv_size];
    let mut logical = bytes.to_vec();
    logical.resize(size, 0);
    let start = PE_START + first_extent * 4096;
    image[start..start + half].copy_from_slice(&logical[..half]);
    image[PE_START..PE_START + half].copy_from_slice(&logical[half..]);
    put_label(&mut image, 1, &[(MDA_OFFSET, MDA_SIZE)]);
    put_metadata(&mut image, MDA_OFFSET, MDA_SIZE, 512, metadata.as_bytes());
    image
}
