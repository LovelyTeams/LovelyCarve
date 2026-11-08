/// The physical address of the primary superblock.
pub const PRIMARY_SUPERBLOCK_ADDR: u64 = 0x10000;

/// The physical addresses of superblocks.
pub const SUPERBLOCK_ADDRS: [u64; 3] = [PRIMARY_SUPERBLOCK_ADDR, 0x4000000, 0x4000000000];

pub const MAGIC: u64 = 0x4D5F53665248425F;

/// Corresponds to `BTRFS_CSUM_SIZE`.
pub const CSUM_SIZE: usize = 32;

/// Corresponds to `BTRFS_FSID_SIZE`.
pub const FSID_SIZE: usize = 16;

/// Corresponds to `BTRFS_LABEL_SIZE`.
pub const LABEL_SIZE: usize = 256;

/// Corresponds to `BTRFS_UUID_SIZE`.
pub const UUID_SIZE: usize = 16;

/// Corresponds to `BTRFS_SYSTEM_CHUNK_ARRAY_SIZE`.
pub const MAX_SYSTEM_CHUNK_ARRAY_SIZE: usize = 2048;

/// Corresponds to `BTRFS_NUM_BACKUP_ROOTS`.
pub const NUM_BACKUP_ROOTS: usize = 4;
