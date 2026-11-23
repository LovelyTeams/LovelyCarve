import { describe, expect, it } from 'vitest';
import type { DiskPartition, ImageInfo } from '../../../lib/ipc/image';
import { availableSchemes, credentialModes, initialSelection, isLockedEncrypted, isWholeSectors, unlockRegions } from './encryptedUnlock';

function partition(id: number, kind: string, start_lba: number, length_lba: number): DiskPartition {
  return { id, kind, label: null, start_lba, length_lba } as DiskPartition;
}
function image(partitions: DiskPartition[], size_bytes = 1024 * 512, extra: Partial<ImageInfo> = {}): ImageInfo {
  return { id: 'disk', path: 'disk.dd', source_format: 'raw', size_bytes, sector_size: 512, table_scheme: null, partitions, disk_id: null, on_disk_bytes: null, modified_unix_ms: null, created_unix_ms: null, ewf: null, ...extra } as ImageInfo;
}

describe('encrypted unlock regions', () => {
  it('offers BitLocker only its detected locked partitions', () => {
    const info = image([partition(1, 'ntfs', 0, 100), partition(2, 'bitlocker', 100, 100), partition(3, 'bitlocker', 200, 100)],
      1024 * 512, { unlocked_partitions: [{ id: 3, scheme: 'bitlocker' }] });
    expect(unlockRegions(info, 'bitlocker')).toEqual([{ partitionId: 2, bytes: 100 * 512 }]);
    expect(isLockedEncrypted(info, info.partitions[1]!)).toBe(true);
    expect(isLockedEncrypted(info, info.partitions[2]!)).toBe(false);
  });

  it('keeps a detected partition to its own scheme and lets the containers span the whole image', () => {
    const info = image([partition(1, 'ntfs', 0, 100), partition(2, 'luks', 100, 100), partition(3, 'bitlocker', 200, 100)]);
    expect(unlockRegions(info, 'luks').map(region => region.partitionId)).toEqual([undefined, 1, 2]);
    expect(unlockRegions(info, 'filevault').map(region => region.partitionId)).toEqual([undefined, 1]);
    expect(availableSchemes(info, 2)).toEqual(['luks']);
    expect(availableSchemes(info, 3)).toEqual(['bitlocker']);
    expect(availableSchemes(info, 1)).toEqual(['luks', 'veracrypt', 'filevault']);
  });

  it('only offers VeraCrypt regions that are a whole number of sectors', () => {
    expect(isWholeSectors({ sector_size: 512 }, 1024)).toBe(true);
    expect(isWholeSectors({ sector_size: 512 }, 1000)).toBe(false);
    expect(isWholeSectors({ sector_size: 4096 }, 512)).toBe(false);
    expect(isWholeSectors({ sector_size: 512 }, 0)).toBe(false);
    // A loose container file whose size is not sector aligned cannot be VeraCrypt.
    const ragged = image([], 196_708);
    expect(unlockRegions(ragged, 'veracrypt')).toEqual([]);
    expect(availableSchemes(ragged, null)).toEqual(['luks', 'filevault']);
    const aligned = image([], 196_608);
    expect(unlockRegions(aligned, 'veracrypt')).toEqual([{ bytes: 196_608 }]);
    // 4Kn evidence counts its own sector size.
    const fourKn = image([partition(1, 'unknown', 0, 10)], 10 * 4096 + 512, { sector_size: 4096 });
    expect(unlockRegions(fourKn, 'veracrypt')).toEqual([{ partitionId: 1, bytes: 10 * 4096 }]);
  });

  it('drops a whole-image container once it is unlocked in place', () => {
    const info = image([partition(0, 'fat12', 0, 384)], 384 * 512, { unlocked_partitions: [{ id: 0, scheme: 'veracrypt' }] });
    expect(unlockRegions(info, 'veracrypt')).toEqual([]);
    expect(availableSchemes(info, null)).toEqual([]);
    expect(initialSelection(info, null)).toBeNull();
  });
});

describe('encrypted unlock defaults and credentials', () => {
  it('starts from the fixed partition, then a locked detected one, then the whole image', () => {
    const info = image([partition(1, 'ntfs', 0, 100), partition(2, 'bitlocker', 100, 100), partition(4, 'luks', 200, 100)]);
    expect(initialSelection(info, 4)).toEqual({ scheme: 'luks', partitionId: 4 });
    expect(initialSelection(info, 1)).toEqual({ scheme: 'luks', partitionId: 1 });
    expect(initialSelection(info, null)).toEqual({ scheme: 'bitlocker', partitionId: 2 });
    expect(initialSelection(image([partition(1, 'ntfs', 0, 100)]), null)).toEqual({ scheme: 'luks' });
  });

  it('gives each scheme the credentials its format accepts', () => {
    expect(credentialModes('bitlocker')).toEqual(['password', 'recovery', 'memory', 'otherKey']);
    expect(credentialModes('luks')).toEqual(['password', 'masterKey']);
    expect(credentialModes('filevault')).toEqual(['password', 'masterKey']);
    expect(credentialModes('veracrypt')).toEqual(['password']);
  });
});
