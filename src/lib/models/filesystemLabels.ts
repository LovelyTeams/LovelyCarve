/** Shared display names for filesystem and partition kinds returned by IPC. */
export const KIND_LABELS: Record<string, string> = {
  ext4: 'Ext4', btrfs: 'Btrfs', lvm: 'LVM2', ntfs: 'NTFS', refs: 'ReFS',
  fat32: 'FAT32', fat12: 'FAT12', fat16: 'FAT16', exfat: 'exFAT', cdfs: 'CDFS',
  squashfs: 'SquashFS', cramfs: 'CramFS', romfs: 'ROMFS', minifs: 'MINIFS', erofs: 'EROFS', f2fs: 'F2FS', qnx4: 'QNX4', qnx6: 'QNX6', udf: 'UDF', minix: 'Minix FS', jfs: 'JFS', reiserfs: 'ReiserFS', amiga: 'Amiga OFS/FFS', befs: 'BeFS', gfs2: 'GFS2', ocfs2: 'OCFS2', hfs: 'HFS', ubifs: 'UBIFS', jffs2: 'JFFS2', yaffs1: 'YAFFS1', yaffs2: 'YAFFS2', ubi: 'UBI', bitlocker: 'BitLocker', biosboot: 'BIOS Boot', apfscontainer: 'APFS',
  hfsplus: 'HFS+', hfsx: 'HFSX', luks: 'LUKS', filevault: 'FileVault 2', apm: 'APM', bsd: 'BSD', swap: 'Swap',
  extended: 'Extended', mdadm: 'MDADM', ufs: 'UFS', xfs: 'XFS', zfs: 'ZFS', unknown: '未知',
};

export function filesystemLabel(kind: string | null | undefined): string {
  return kind ? KIND_LABELS[kind.toLowerCase()] ?? kind.toUpperCase() : '未识别';
}
