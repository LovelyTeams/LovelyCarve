//! codec implementation.
use super::*;

/// The one `bincode` config used for every v2 frame: little-endian, var-int.
/// Encode and decode MUST go through the same config, so route both here — a
/// divergence would silently mis-decode rather than error. `+ Copy` is load-
/// bearing: [`read_entry_frames`] passes the value by-value once per frame, so
/// the opaque type must stay copyable.
#[inline]
pub(crate) fn snapshot_bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
}

/// Write the snapshot header as a single JSON line + `\n`. Identical framing in
/// every schema version so [`load_snapshot_header`] never needs to change.
/// Shared by all four builders.
pub fn write_snapshot_header<W: Write>(w: &mut W, header: &SnapshotHeader) -> anyhow::Result<()> {
    writeln!(w, "{}", serde_json::to_string(header)?)?;
    Ok(())
}

/// Append one length-prefixed bincode frame for `entry`: `[u32 LE len][bytes]`.
/// `scratch` is cleared and reused across the millions of entries a build emits
/// so per-entry encoding allocates nothing. Shared by all four builders.
pub fn write_snapshot_entry<W: Write>(
    w: &mut W,
    entry: &SnapshotEntry,
    scratch: &mut Vec<u8>,
) -> anyhow::Result<()> {
    scratch.clear();
    bincode::serde::encode_into_std_write(entry, scratch, snapshot_bincode_config())?;
    let len: u32 = scratch.len().try_into().map_err(|_| {
        anyhow::anyhow!(
            "snapshot entry too large to frame ({} bytes)",
            scratch.len()
        )
    })?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(scratch.as_slice())?;
    Ok(())
}

/// Read just the header line of a `.dirz` file — used to list snapshots
/// without paying to parse every entry.
pub fn load_snapshot_header(path: &Path) -> anyhow::Result<SnapshotHeader> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    let mut reader = BufReader::new(decoder);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let header: SnapshotHeader = serde_json::from_str(first.trim_end())?;
    if header.schema_version > SNAPSHOT_SCHEMA {
        anyhow::bail!(
            "snapshot schema {} is newer than supported {SNAPSHOT_SCHEMA}",
            header.schema_version
        );
    }
    Ok(header)
}

/// 旧版快照体里 LVM 逻辑卷节点存的是「LV {卷名} · {文件系统}」的展示名，
/// 现行命名只留卷名。载入时就地归一化，老案件不必重建快照就能显示新名。
/// LVM 卷名的合法字符集（字母数字和 `+_.-`）不含空格，真实卷名不可能以
/// "LV " 开头或含 " · "，按前后缀剥离不会误伤。
pub(super) fn normalize_legacy_lv_name(node: &mut FsNode) {
    if !node.id.starts_with("lvm:") {
        return;
    }
    if let Some(rest) = node.name.strip_prefix("LV ") {
        let name = rest.rsplit_once(" · ").map_or(rest, |(n, _)| n);
        if !name.is_empty() {
            node.name = name.to_string();
        }
    }
}

/// Outcome of reading a v2 frame's 4-byte length prefix.
pub(super) enum FrameStart {
    /// Stream ended cleanly at a frame boundary — no more entries.
    Clean,
    /// A frame of this many bytes follows.
    Length(u32),
    /// Stream ended mid-prefix — the file is truncated / corrupt.
    Torn,
}

/// Read the next 4-byte little-endian frame length, distinguishing a clean
/// end-of-stream (0 bytes available at a frame boundary) from a torn tail
/// (1–3 bytes). `read_exact` can't tell these apart — both surface as
/// `UnexpectedEof` — and conflating them would let a truncated snapshot load as
/// "valid but short", a forensic-integrity bug.
pub(super) fn read_frame_len<R: std::io::Read>(
    r: &mut R,
    buf: &mut [u8; 4],
) -> std::io::Result<FrameStart> {
    let mut filled = 0usize;
    while filled < 4 {
        match r.read(&mut buf[filled..]) {
            Ok(0) => {
                return Ok(if filled == 0 {
                    FrameStart::Clean
                } else {
                    FrameStart::Torn
                });
            }
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(FrameStart::Length(u32::from_le_bytes(*buf)))
}

/// Read the length-prefixed bincode entry frames of a v2 body to EOF, reusing
/// one decode buffer. Errors on a truncated / torn tail rather than returning a
/// short snapshot. Each decoded entry is normalized before it reaches the
/// visitor so stream-indexed snapshots match [`VolumeSnapshot::new`].
pub(super) fn read_entry_frames<R: std::io::Read>(
    reader: &mut R,
    mut visit: impl FnMut(SnapshotEntry) -> anyhow::Result<()>,
) -> anyhow::Result<usize> {
    let mut count = 0usize;
    let mut len_buf = [0u8; 4];
    let mut body: Vec<u8> = Vec::new();
    loop {
        match read_frame_len(reader, &mut len_buf)? {
            FrameStart::Clean => break,
            FrameStart::Torn => {
                anyhow::bail!("snapshot truncated: partial frame length prefix")
            }
            FrameStart::Length(len) => {
                let len = len as usize;
                body.resize(len, 0);
                reader.read_exact(&mut body).map_err(|e| {
                    anyhow::anyhow!("snapshot truncated: frame body ({len} bytes): {e}")
                })?;
                let (mut entry, _consumed): (SnapshotEntry, usize) =
                    bincode::serde::decode_from_slice(&body, snapshot_bincode_config())?;
                normalize_legacy_lv_name(&mut entry.node);
                visit(entry)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

pub(super) fn open_snapshot_body(
    path: &Path,
) -> anyhow::Result<(SnapshotHeader, impl std::io::Read)> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    let mut reader = BufReader::with_capacity(64 * 1024, decoder);

    let mut first = String::new();
    if reader.read_line(&mut first)? == 0 {
        anyhow::bail!("snapshot file is empty");
    }
    let header: SnapshotHeader = serde_json::from_str(first.trim_end())?;
    if header.schema_version > SNAPSHOT_SCHEMA {
        anyhow::bail!(
            "snapshot schema {} is newer than supported {SNAPSHOT_SCHEMA}",
            header.schema_version
        );
    }
    if header.schema_version != SNAPSHOT_SCHEMA {
        anyhow::bail!(
            "snapshot schema {} is no longer supported (rebuild required)",
            header.schema_version
        );
    }
    Ok((header, reader))
}

/// Stream the entries of a `.dirz` file without materializing the whole
/// snapshot body. Used by the SQLite snapshot indexer so opening or reusing a
/// large case snapshot does not build a long-lived `Vec<SnapshotEntry>`.
pub fn visit_snapshot_entries(
    path: &Path,
    visit: impl FnMut(SnapshotEntry) -> anyhow::Result<()>,
) -> anyhow::Result<(SnapshotHeader, usize)> {
    visit_snapshot_entries_checked(path, |_| Ok(()), visit)
}

/// Stream the entries of a `.dirz` file after validating its header.
///
/// The header is read from the same decompression stream that is later used for
/// entry frames, so callers that need to check the image or volume identity do
/// not have to open and decompress the snapshot once for the header and again
/// for the body.
pub fn visit_snapshot_entries_checked(
    path: &Path,
    validate_header: impl FnOnce(&SnapshotHeader) -> anyhow::Result<()>,
    visit: impl FnMut(SnapshotEntry) -> anyhow::Result<()>,
) -> anyhow::Result<(SnapshotHeader, usize)> {
    let (header, mut reader) = open_snapshot_body(path)?;
    validate_header(&header)?;
    let count = read_entry_frames(&mut reader, visit)?;
    Ok((header, count))
}

/// Read a whole `.dirz` file into an indexed [`VolumeSnapshot`].
///
/// v2 layout: a JSON header line (read with `read_line`, leaving the same
/// `BufReader` positioned at the body) followed by length-prefixed bincode
/// frames. We must NOT use `reader.lines()` here — `Lines` takes the reader by
/// value and buffers past the header newline, which would swallow the binary
/// body. Schema-1 JSON-lines bodies are no longer parsed (the caller falls back
/// to a live walk / rebuild).
pub fn load_snapshot(path: &Path) -> anyhow::Result<VolumeSnapshot> {
    let (header, mut reader) = open_snapshot_body(path)?;
    let mut entries = Vec::new();
    read_entry_frames(&mut reader, |entry| {
        entries.push(entry);
        Ok(())
    })?;
    Ok(VolumeSnapshot::new(header, entries))
}
