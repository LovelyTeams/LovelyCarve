// 加密卷解锁的纯模型：哪些格式、哪些区域可以解锁，以及每种格式接受的凭据。
// BitLocker、LUKS、VeraCrypt、FileVault 走同一个解锁命令，区别只在这里。
import { sourceText, type Translator } from '../../../i18n/translate';
import type { EncryptionScheme, VeracryptHeader, VeracryptPrf } from '../../../lib/ipc/encrypted';
import type { DiskPartition, ImageInfo } from '../../../lib/ipc/image';

/** Every scheme, in the order the dialog offers them. */
export const ENCRYPTION_SCHEMES: readonly EncryptionScheme[] = ['bitlocker', 'luks', 'veracrypt', 'filevault'];

export function schemeLabel(scheme: EncryptionScheme): string {
  switch (scheme) {
    case 'bitlocker': return 'BitLocker';
    case 'luks': return 'LUKS1 / LUKS2';
    case 'veracrypt': return 'VeraCrypt';
    case 'filevault': return 'FileVault 2 / CoreStorage';
  }
}

/** The product name alone, for badges and messages. */
export function schemeShortLabel(scheme: EncryptionScheme): string {
  switch (scheme) {
    case 'bitlocker': return 'BitLocker';
    case 'luks': return 'LUKS';
    case 'veracrypt': return 'VeraCrypt';
    case 'filevault': return 'FileVault';
  }
}

/** The scheme a partition's encryption header names; VeraCrypt has no header to name. */
export function detectedScheme(partition: DiskPartition): EncryptionScheme | undefined {
  switch (partition.kind) {
    case 'bitlocker':
    case 'luks':
    case 'filevault': return partition.kind;
    default: return undefined;
  }
}

/** The scheme `partitionId` was unlocked with this session, if it was. */
export function unlockedScheme(info: ImageInfo, partitionId: number): EncryptionScheme | undefined {
  return info.unlocked_partitions?.find(unlocked => unlocked.id === partitionId)?.scheme;
}

/** A partition whose detected encryption is still locked. */
export function isLockedEncrypted(info: ImageInfo, partition: DiskPartition): boolean {
  return detectedScheme(partition) !== undefined && unlockedScheme(info, partition.id) === undefined;
}

/** The region an unlock reads: one partition, or the whole image without `partitionId`. */
export interface UnlockRegion {
  partitionId?: number;
  bytes: number;
}

/** A VeraCrypt volume is a whole number of sectors, so a region of any other
 *  size cannot be one. */
export function isWholeSectors(info: Pick<ImageInfo, 'sector_size'>, bytes: number): boolean {
  return info.sector_size > 0 && bytes > 0 && bytes % info.sector_size === 0;
}

/** An unlocked whole-image container appears as a partition covering the image. */
function wholeImageUnlocked(info: ImageInfo): boolean {
  return info.partitions.some(partition => unlockedScheme(info, partition.id) !== undefined
    && partition.start_lba === 0 && partition.length_lba * info.sector_size >= info.size_bytes);
}

/** Regions `scheme` can unlock. A partition whose header names a scheme opens
 *  only with that scheme; BitLocker always has its header. The containers may
 *  also span the whole image, and VeraCrypt only regions of whole sectors.
 *  Partitions already unlocked are left out. */
export function unlockRegions(info: ImageInfo, scheme: EncryptionScheme): UnlockRegion[] {
  const partitions = info.partitions
    .filter(partition => unlockedScheme(info, partition.id) === undefined)
    .filter(partition => {
      const detected = detectedScheme(partition);
      return scheme === 'bitlocker' ? detected === 'bitlocker' : detected === undefined || detected === scheme;
    })
    .map(partition => ({ partitionId: partition.id, bytes: partition.length_lba * info.sector_size }));
  if (scheme === 'bitlocker') return partitions;
  const regions = wholeImageUnlocked(info) ? partitions : [{ bytes: info.size_bytes }, ...partitions];
  return scheme === 'veracrypt' ? regions.filter(region => isWholeSectors(info, region.bytes)) : regions;
}

/** Schemes the dialog offers: those with a region to unlock, and for a fixed
 *  partition those that can open it. */
export function availableSchemes(info: ImageInfo, partitionId: number | null): EncryptionScheme[] {
  return ENCRYPTION_SCHEMES.filter(scheme => {
    const regions = unlockRegions(info, scheme);
    return partitionId === null ? regions.length > 0 : regions.some(region => region.partitionId === partitionId);
  });
}

/** Where the dialog starts: the fixed partition's scheme, else the first locked
 *  detected partition, else the whole image with the first container scheme. */
export function initialSelection(info: ImageInfo, partitionId: number | null): { scheme: EncryptionScheme; partitionId?: number } | null {
  const schemes = availableSchemes(info, partitionId);
  if (!schemes.length) return null;
  const pick = (scheme: EncryptionScheme | undefined) => scheme && schemes.includes(scheme) ? scheme : schemes[0]!;
  if (partitionId !== null) {
    const partition = info.partitions.find(candidate => candidate.id === partitionId);
    return { scheme: pick(partition && detectedScheme(partition)), partitionId };
  }
  const locked = info.partitions.find(partition => isLockedEncrypted(info, partition));
  if (locked) return { scheme: pick(detectedScheme(locked)), partitionId: locked.id };
  const scheme = pick('luks');
  const region = unlockRegions(info, scheme)[0];
  return { scheme, ...(region?.partitionId === undefined ? {} : { partitionId: region.partitionId }) };
}

/** How the dialog asks for a credential. BitLocker's other keys are the BEK
 *  startup key, a VMK, the FVEK with its TWEAK, and the clear key. */
export type CredentialMode = 'password' | 'recovery' | 'masterKey' | 'memory' | 'otherKey';

export function credentialModes(scheme: EncryptionScheme): CredentialMode[] {
  switch (scheme) {
    case 'bitlocker': return ['password', 'recovery', 'memory', 'otherKey'];
    case 'luks':
    case 'filevault': return ['password', 'masterKey'];
    case 'veracrypt': return ['password'];
  }
}

/** VeraCrypt header choices, in the order the dialog offers them. */
export const VERACRYPT_HEADERS: readonly VeracryptHeader[] = ['auto', 'normal', 'hidden', 'backup', 'system'];

export function veracryptHeaderLabel(header: VeracryptHeader, tr: Translator = sourceText): string {
  switch (header) {
    case 'auto': return tr("自动");
    case 'normal': return tr("标准卷");
    case 'hidden': return tr("隐藏卷");
    case 'backup': return tr("备份卷头");
    case 'system': return tr("系统加密");
  }
}

/** A VeraCrypt key derivation choice; `auto` tries every one. */
export type VeracryptPrfChoice = VeracryptPrf | 'auto';

export const VERACRYPT_PRFS: readonly VeracryptPrfChoice[] = ['auto', 'sha512', 'sha256', 'blake2s', 'whirlpool', 'streebog', 'argon2id', 'ripemd160'];

export function veracryptPrfLabel(prf: VeracryptPrfChoice, tr: Translator = sourceText): string {
  switch (prf) {
    case 'auto': return tr("自动");
    case 'sha512': return 'SHA-512';
    case 'sha256': return 'SHA-256';
    case 'blake2s': return 'BLAKE2s-256';
    case 'whirlpool': return 'Whirlpool';
    case 'streebog': return 'Streebog';
    case 'argon2id': return 'Argon2id';
    case 'ripemd160': return 'RIPEMD-160';
  }
}

/** The largest PIM VeraCrypt accepts: pre-boot authentication allows less. */
export function veracryptPimLimit(header: VeracryptHeader): number {
  return header === 'system' ? 65_535 : 2_147_468;
}

/** Whether the key derivation can open the header: system encryption never
 *  uses Argon2id. */
export function veracryptPrfFits(header: VeracryptHeader, prf: VeracryptPrfChoice): boolean {
  return !(header === 'system' && prf === 'argon2id');
}

export function credentialModeLabel(mode: CredentialMode, tr: Translator = sourceText): string {
  switch (mode) {
    case 'password': return tr("密码");
    case 'recovery': return tr("恢复密钥");
    case 'masterKey': return tr("主密钥");
    case 'memory': return tr("内存镜像");
    case 'otherKey': return tr("其他密钥");
  }
}
