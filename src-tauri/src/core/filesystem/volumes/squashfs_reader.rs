//! Version selection for the shared SquashFS tree adapter.

use std::io::SeekFrom;

use anyhow::{bail, Result};
use backhand::{kind, BufReadSeek, FilesystemReaderTrait};

pub(super) fn open_reader(
    mut reader: impl BufReadSeek + 'static,
) -> Result<Box<dyn FilesystemReaderTrait>> {
    let mut header = [0; 32];
    reader.read_exact(&mut header)?;
    reader.seek(SeekFrom::Start(0))?;
    let big = matches!(&header[..4], b"sqsh" | b"tqsh");
    let version = if big {
        u16::from_be_bytes([header[28], header[29]])
    } else {
        u16::from_le_bytes([header[28], header[29]])
    };
    let minor = if big {
        u16::from_be_bytes([header[30], header[31]])
    } else {
        u16::from_le_bytes([header[30], header[31]])
    };
    anyhow::ensure!(minor == 0, "unsupported SquashFS version {version}.{minor}");
    let selected = match (&header[..4], version) {
        (b"hsqs", 4) => kind::LE_V4_0,
        (b"sqsh", 4) => kind::BE_V4_0,
        (b"hsqs", 3) => kind::LE_V3_0,
        (b"sqsh", 3) => kind::BE_V3_0,
        (b"hsqt", 3) => kind::LE_V3_0_DDWRT,
        (b"tqsh", 3) => kind::BE_V3_0_DDWRT,
        _ => bail!(
            "unsupported SquashFS magic/version: {:?} v{version}",
            &header[..4]
        ),
    };
    let kind = kind::Kind::from_const(selected).map_err(anyhow::Error::msg)?;
    Ok(backhand::create_squashfs_from_kind(reader, 0, kind)?)
}
