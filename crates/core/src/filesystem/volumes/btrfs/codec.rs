//! On-disk item decoding and compression codecs.
use super::*;

// =============================================================================
// Payload parsers.
// =============================================================================

pub(super) fn parse_sys_chunk_array(bytes: &[u8]) -> anyhow::Result<Vec<ChunkMapping>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + KEY_RECORD_SIZE + 48 <= bytes.len() {
        let key = Key::ref_from_bytes(&bytes[pos..pos + KEY_RECORD_SIZE])
            .map_err(|_| anyhow!("btrfs: invalid sys_chunk_array key"))?;
        if key.key_type != KEY_CHUNK_ITEM {
            break;
        }
        pos += KEY_RECORD_SIZE;
        let chunk = Chunk::ref_from_bytes(&bytes[pos..pos + 48])
            .map_err(|_| anyhow!("btrfs: invalid sys_chunk_array chunk"))?;
        let logical = key.offset.get();
        let length = chunk.length.get();
        let num_stripes = chunk.num_stripes.get() as usize;
        pos += 48;
        if num_stripes == 0 {
            break;
        }
        let stripes_total = num_stripes * 32;
        if pos + stripes_total > bytes.len() {
            break;
        }
        let stripe = Stripe::ref_from_bytes(&bytes[pos..pos + 32])
            .map_err(|_| anyhow!("btrfs: invalid sys_chunk_array stripe"))?;
        out.push(ChunkMapping {
            logical,
            length,
            physical: stripe.offset.get(),
        });
        pos += stripes_total;
    }
    Ok(out)
}

pub(super) fn parse_chunk_item(key: &Key, data: &[u8]) -> Option<ChunkMapping> {
    if data.len() < 48 + 32 {
        return None;
    }
    let chunk = Chunk::ref_from_bytes(&data[..48]).ok()?;
    let stripe = Stripe::ref_from_bytes(&data[48..80]).ok()?;
    Some(ChunkMapping {
        logical: key.offset.get(),
        length: chunk.length.get(),
        physical: stripe.offset.get(),
    })
}

pub(super) fn parse_root_item(data: &[u8]) -> Option<(u64, u8)> {
    if data.len() < 439 {
        return None;
    }
    let item = RootItem::ref_from_bytes(&data[..439]).ok()?;
    Some((item.bytenr.get(), item.level))
}

/// Parse the trailing name out of a `ROOT_REF` (or `ROOT_BACKREF`) leaf
/// payload. The on-disk layout is `dirid(8) sequence(8) name_len(2)
/// name[name_len]`.
pub(super) fn parse_root_ref_name(data: &[u8]) -> Option<String> {
    if data.len() < 18 {
        return None;
    }
    let name_len = u16::from_le_bytes(data[16..18].try_into().ok()?) as usize;
    if 18 + name_len > data.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&data[18..18 + name_len]).into_owned())
}

/// Parse a `btrfs_inode_item` leaf payload (160 fixed bytes). `None` on a
/// short or malformed payload — callers skip the record, matching
/// `read_inode`'s historical lenience.
pub(super) fn parse_inode_item(objectid: u64, data: &[u8]) -> Option<BtrfsInode> {
    if data.len() < 160 {
        return None;
    }
    let item = InodeItem::ref_from_bytes(&data[..160]).ok()?;
    Some(BtrfsInode {
        objectid,
        mode: item.mode.get(),
        size: item.size.get(),
        atime: item.atime.timestamp.get(),
        ctime: item.ctime.timestamp.get(),
        mtime: item.mtime.timestamp.get(),
        otime: item.otime.timestamp.get(),
        flags: item.flags.get(),
    })
}

pub(super) fn parse_dir_item(data: &[u8]) -> Option<DirEntry> {
    if data.len() < 30 {
        return None;
    }
    let location_objectid = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let location_key_type = data[8];
    let _location_offset = u64::from_le_bytes(data[9..17].try_into().ok()?);
    let _transid = u64::from_le_bytes(data[17..25].try_into().ok()?);
    let _data_len = u16::from_le_bytes(data[25..27].try_into().ok()?);
    let name_len = u16::from_le_bytes(data[27..29].try_into().ok()?) as usize;
    let ftype = data[29];
    if 30 + name_len > data.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&data[30..30 + name_len]).into_owned();
    Some(DirEntry {
        name,
        location_objectid,
        location_key_type,
        ftype,
    })
}

pub(super) fn parse_file_extent(file_offset: u64, data: &[u8]) -> Option<FileExtent> {
    if data.len() < 21 {
        return None;
    }
    let _generation = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let ram_bytes = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let compression = data[16];
    let _encryption = data[17];
    let _other_encoding = u16::from_le_bytes(data[18..20].try_into().ok()?);
    let ext_type = data[20];

    if ext_type == EXTENT_INLINE {
        return Some(FileExtent {
            file_offset,
            kind: ExtentKind::Inline {
                ram_bytes,
                compression,
                data: data[21..].to_vec(),
            },
        });
    }

    if data.len() < FILE_EXTENT_REGULAR_FIXED {
        return None;
    }
    let disk_bytenr = u64::from_le_bytes(data[21..29].try_into().ok()?);
    let disk_num_bytes = u64::from_le_bytes(data[29..37].try_into().ok()?);
    let offset = u64::from_le_bytes(data[37..45].try_into().ok()?);
    let num_bytes = u64::from_le_bytes(data[45..53].try_into().ok()?);

    Some(FileExtent {
        file_offset,
        kind: ExtentKind::Regular {
            ext_type,
            disk_bytenr,
            disk_num_bytes,
            offset,
            num_bytes,
            ram_bytes,
            compression,
        },
    })
}

pub(super) fn decompress(
    comp: u8,
    data: &[u8],
    ram_bytes: usize,
    sector_size: usize,
) -> anyhow::Result<Vec<u8>> {
    match comp {
        COMPRESS_NONE => Ok(data.to_vec()),
        COMPRESS_ZLIB => {
            let mut out = Vec::with_capacity(ram_bytes);
            let mut decoder = flate2::read::ZlibDecoder::new(data);
            decoder
                .read_to_end(&mut out)
                .context("btrfs: zlib decompression failed")?;
            Ok(out)
        }
        COMPRESS_ZSTD => {
            let mut out = Vec::with_capacity(ram_bytes);
            let mut decoder = zstd::stream::read::Decoder::new(data)
                .context("btrfs: zstd decoder init failed")?;
            decoder
                .read_to_end(&mut out)
                .context("btrfs: zstd decompression failed")?;
            Ok(out)
        }
        COMPRESS_LZO => decompress_lzo(data, ram_bytes, sector_size),
        other => bail!("btrfs: unknown compression code {other}"),
    }
}

pub(super) fn decompress_lzo(
    data: &[u8],
    ram_bytes: usize,
    sector_size: usize,
) -> anyhow::Result<Vec<u8>> {
    if ram_bytes == 0 {
        return Ok(Vec::new());
    }
    if !(4096..=LZO_MAX_SECTOR_SIZE).contains(&sector_size) || !sector_size.is_power_of_two() {
        bail!("btrfs: invalid LZO sector size {sector_size}");
    }
    if data.len() < LZO_LEN_SIZE {
        bail!("btrfs: LZO extent is missing total length header");
    }

    let total_len = read_lzo_len(data, 0)?;
    if total_len < LZO_LEN_SIZE * 2 {
        bail!("btrfs: LZO total length {total_len} is too small");
    }
    if total_len > data.len() {
        bail!(
            "btrfs: LZO extent truncated: header says {total_len} bytes, have {}",
            data.len()
        );
    }

    let mut cursor = LZO_LEN_SIZE;
    let mut out = Vec::with_capacity(ram_bytes);
    let max_segment_len = sector_size + sector_size / 16 + 64 + 3;
    while cursor < total_len && out.len() < ram_bytes {
        if total_len - cursor < LZO_LEN_SIZE {
            bail!("btrfs: LZO segment length header crosses extent boundary");
        }
        let segment_len = read_lzo_len(data, cursor)?;
        cursor += LZO_LEN_SIZE;
        if segment_len == 0 {
            bail!("btrfs: LZO segment has zero length");
        }
        if segment_len > max_segment_len {
            bail!("btrfs: LZO segment length {segment_len} exceeds maximum {max_segment_len}");
        }
        let segment_end = cursor
            .checked_add(segment_len)
            .ok_or_else(|| anyhow!("btrfs: LZO segment length overflow"))?;
        if segment_end > total_len {
            bail!(
                "btrfs: LZO segment overruns extent: segment ends at {segment_end}, total {total_len}"
            );
        }

        let mut page = vec![0u8; sector_size];
        let decoded = lzokay::decompress::decompress(&data[cursor..segment_end], &mut page)
            .context("btrfs: LZO decompression failed")?;
        let remaining = ram_bytes - out.len();
        out.extend_from_slice(&page[..decoded.min(remaining)]);
        cursor = segment_end;

        let bytes_left_in_sector = sector_size - (cursor % sector_size);
        if bytes_left_in_sector < LZO_LEN_SIZE {
            cursor = cursor
                .checked_add(bytes_left_in_sector)
                .ok_or_else(|| anyhow!("btrfs: LZO sector padding overflow"))?;
        }
    }

    if out.len() < ram_bytes {
        bail!(
            "btrfs: LZO decompressed {} bytes, expected {ram_bytes}",
            out.len()
        );
    }
    Ok(out)
}

pub(super) fn read_lzo_len(data: &[u8], offset: usize) -> anyhow::Result<usize> {
    let bytes = data
        .get(offset..offset + LZO_LEN_SIZE)
        .ok_or_else(|| anyhow!("btrfs: LZO length field at {offset} is truncated"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
}

pub(super) fn decode_label(buf: &[u8]) -> Option<String> {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&buf[..end])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(super) fn format_ts(ts: i64) -> Option<String> {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
}
