//! Bounded decoding of Windows STORAGE_DEVICE_DESCRIPTOR responses.

/// Serial bytes reported by a physical drive, with an optional ATA word-order
/// interpretation. Neither interpretation is an inferred member identity.
#[derive(Debug, Clone)]
pub struct DriveSerial {
    reported: String,
    word_swapped: Option<String>,
}

impl DriveSerial {
    /// The device-reported ASCII serial, with surrounding whitespace trimmed.
    pub fn as_str(&self) -> &str {
        &self.reported
    }

    /// Some ATA drivers expose IDENTIFY string words without swapping their
    /// bytes. Keep this alternative separate: only the RAID metadata can
    /// establish whether it identifies a member unambiguously.
    pub(crate) fn word_swapped(&self) -> Option<&str> {
        self.word_swapped.as_deref()
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug)]
pub(crate) struct DeviceDescriptor {
    pub model: Option<String>,
    pub is_removable: bool,
    pub serial: Option<DriveSerial>,
}

/// Offsets are relative to the descriptor and must point past its fixed
/// fields. Only bytes inside both Size and the actual IOCTL response count;
/// zero-filled allocation capacity must not supply a missing terminator.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn parse(buf: &[u8]) -> Option<DeviceDescriptor> {
    const FIXED_SIZE: usize = 36;
    if buf.len() < FIXED_SIZE {
        return None;
    }
    let read_u32 = |offset: usize| {
        buf.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let size = read_u32(4)? as usize;
    if size < FIXED_SIZE || size > buf.len() {
        return None;
    }
    let read_cstr = |offset: u32| -> Option<&[u8]> {
        let start = offset as usize;
        if start < FIXED_SIZE || start >= size {
            return None;
        }
        let tail = &buf[start..size];
        let end = tail.iter().position(|&byte| byte == 0)?;
        Some(&tail[..end])
    };
    let read_text = |offset| {
        read_cstr(offset)
            .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let vendor = read_text(read_u32(12)?);
    let product = read_text(read_u32(16)?);
    let model = match (vendor, product) {
        (Some(vendor), Some(product)) => Some(format!("{vendor} {product}")),
        (Some(text), None) | (None, Some(text)) => Some(text),
        (None, None) => None,
    };
    let serial = read_cstr(read_u32(24)?).and_then(|bytes| {
        // A serial is identity evidence: never use lossy UTF-8 replacement or
        // remove interior bytes to manufacture an apparently valid serial.
        let text = std::str::from_utf8(bytes).ok()?.trim_ascii();
        if text.is_empty()
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            return None;
        }
        let bus_type = read_u32(28)?;
        let word_swapped = if matches!(bus_type, 2 | 3 | 11) && bytes.len().is_multiple_of(2) {
            // ATAPI / ATA / SATA only. Swap before trimming, since leading
            // padding is part of the word alignment.
            let mut swapped = bytes.to_vec();
            for word in swapped.as_chunks_mut::<2>().0 {
                word.swap(0, 1);
            }
            String::from_utf8(swapped)
                .ok()
                .map(|text| text.trim_ascii().to_string())
                .filter(|swapped| !swapped.is_empty() && swapped != text)
        } else {
            None
        };
        Some(DriveSerial {
            reported: text.to_string(),
            word_swapped,
        })
    });
    Some(DeviceDescriptor {
        model,
        is_removable: buf[10] != 0,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(serial: &[u8], bus: u32) -> Vec<u8> {
        let mut buf = vec![0; 64];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes());
        buf[10] = 1;
        buf[12..16].copy_from_slice(&36u32.to_le_bytes());
        buf[16..20].copy_from_slice(&44u32.to_le_bytes());
        buf[24..28].copy_from_slice(&64u32.to_le_bytes());
        buf[28..32].copy_from_slice(&bus.to_le_bytes());
        buf[36..44].copy_from_slice(b"Vendor \0");
        buf[44..52].copy_from_slice(b" Model \0");
        buf.extend_from_slice(serial);
        buf.push(0);
        let size = buf.len() as u32;
        buf[4..8].copy_from_slice(&size.to_le_bytes());
        buf
    }

    #[test]
    fn serial_is_trimmed_and_model_and_removability_are_preserved() {
        let descriptor = parse(&fixture(b"  WD-123456789  ", 17)).unwrap();
        assert_eq!(descriptor.model.as_deref(), Some("Vendor Model"));
        assert!(descriptor.is_removable);
        let serial = descriptor.serial.unwrap();
        assert_eq!(serial.as_str(), "WD-123456789");
        assert_eq!(serial.word_swapped(), None); // NVMe is not ATA.
    }

    #[test]
    fn ata_word_order_keeps_padding_until_after_swapping() {
        // A 20-byte ATA string whose decoded serial has odd length and a
        // leading pad byte. Trimming first would change every word boundary.
        let mut bytes = b" WD-12345678        ".to_vec();
        assert_eq!(bytes.len(), 20);
        for word in bytes.as_chunks_mut::<2>().0 {
            word.swap(0, 1);
        }
        for bus in [2, 3, 11] {
            let descriptor = parse(&fixture(&bytes, bus)).unwrap();
            let serial = descriptor.serial.unwrap();
            assert_eq!(serial.as_str(), "W -D21436587");
            assert_eq!(serial.word_swapped(), Some("WD-12345678"));
        }
    }

    #[test]
    fn serial_offsets_must_be_inside_the_returned_descriptor() {
        for offset in [0, 1, 24, 35, 256, u32::MAX] {
            let mut buf = fixture(b"SERIAL", 11);
            buf[24..28].copy_from_slice(&offset.to_le_bytes());
            assert!(parse(&buf).unwrap().serial.is_none(), "offset {offset}");
        }
        let mut buf = fixture(b"SERIAL", 11);
        let size = buf.len() as u32;
        buf[24..28].copy_from_slice(&size.to_le_bytes());
        assert!(parse(&buf).unwrap().serial.is_none());
        buf[24..28].copy_from_slice(&64u32.to_le_bytes());
        buf.pop(); // Missing NUL; allocation slack must not become evidence.
        assert!(parse(&buf).is_none());
        let size = buf.len() as u32;
        buf[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(parse(&buf).unwrap().serial.is_none());
        buf.resize(1024, 0);
        assert!(parse(&buf).unwrap().serial.is_none());
    }

    #[test]
    fn malformed_headers_and_invalid_serials_are_not_identity_evidence() {
        for len in 0..36 {
            assert!(parse(&vec![0; len]).is_none());
        }
        for size in [0u32, 35, u32::MAX] {
            let mut buf = fixture(b"SERIAL", 11);
            buf[4..8].copy_from_slice(&size.to_le_bytes());
            assert!(parse(&buf).is_none());
        }
        for serial in [
            b"".as_slice(),
            b"    ",
            b"SN\xff",
            b"SN\x01",
            b"SN\t123",
            b"\xc2\xa0SN",
        ] {
            assert!(parse(&fixture(serial, 11)).unwrap().serial.is_none());
        }
    }
}
