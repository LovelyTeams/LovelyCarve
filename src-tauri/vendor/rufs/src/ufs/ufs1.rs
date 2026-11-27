//! Read the 4.4BSD UFS1 inode layout into the shared inode representation.
use super::*;

pub(super) fn read_inode<R: Backend>(
    file: &mut Decoder<BlockReader<R>>,
    offset: u64,
    inline_limit: i32,
) -> IoResult<Inode> {
    file.seek(offset)?;
    let mode: u16 = file.decode()?;
    let nlink = file.decode()?;
    let ignored = file.decode()?;
    let size: u64 = file.decode()?;
    let atime = i64::from(file.decode::<i32>()?);
    let atimensec: u32 = file.decode()?;
    let mtime = i64::from(file.decode::<i32>()?);
    let mtimensec: u32 = file.decode()?;
    let ctime = i64::from(file.decode::<i32>()?);
    let ctimensec: u32 = file.decode()?;
    if [atimensec, mtimensec, ctimensec]
        .into_iter()
        .any(|ns| ns >= 1_000_000_000)
    {
        iobail!(
            ErrorKind::InvalidData,
            "UFS1 invalid inode nanosecond timestamp"
        );
    }
    // Symlinks and block pointers share the same 60 raw bytes.
    let mut address_bytes = [0; (UFS_NDADDR + UFS_NIADDR) * UFS1_POINTER_SIZE];
    file.read(&mut address_bytes)?;
    let flags = file.decode()?;
    let blocks = u64::from(file.decode::<u32>()?);
    let generation = file.decode()?;
    let uid = file.decode()?;
    let gid = file.decode()?;
    let modrev = file.decode()?;
    let data = if mode & S_IFMT == S_IFLNK && size < inline_limit.max(0) as u64 {
        if size > address_bytes.len() as u64 || blocks != 0 {
            iobail!(
                ErrorKind::InvalidData,
                "UFS1 invalid inline symlink size or allocation"
            );
        }
        let mut bytes = [0u8; UFS_SLLEN];
        bytes[..address_bytes.len()].copy_from_slice(&address_bytes);
        InodeData::Shortlink(bytes)
    } else {
        let mut addresses = Decoder::new(Cursor::new(address_bytes), file.config());
        let direct = addresses.decode::<[i32; UFS_NDADDR]>()?.map(i64::from);
        let indirect = addresses.decode::<[i32; UFS_NIADDR]>()?.map(i64::from);
        InodeData::Blocks(InodeBlocks { direct, indirect })
    };
    Ok(Inode {
        mode,
        nlink,
        uid,
        gid,
        blksize: 0,
        size,
        blocks,
        atime,
        mtime,
        ctime,
        birthtime: 0,
        mtimensec,
        atimensec,
        ctimensec,
        birthnsec: 0,
        r#gen: generation,
        kernflags: 0,
        flags,
        extsize: 0,
        extb: [0; UFS_NXADDR],
        data,
        modrev,
        ignored,
        ckhash: 0,
        spare: [0; 2],
    })
}
