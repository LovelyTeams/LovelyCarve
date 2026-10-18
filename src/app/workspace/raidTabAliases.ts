import type { EvidenceRef } from '../../lib/ipc/cases';
import type { RaidStack } from '../../lib/ipc/raid';

/** Windows evidence paths are case-insensitive; POSIX paths keep their case. */
export function raidTabPathKey(path: string): string {
  return /^(?:[a-z]:[\\/]|\\\\)/i.test(path) || /^\/\//.test(path)
    ? path.replace(/\\/g, '/').toLowerCase()
    : path;
}

/** Use the case's recorded synthetic path; hiding sources never unloads them.
 *  A disk holding members of several arrays (NAS system, swap and data
 *  partitions) opens the largest one. */
export function raidMemberTabAliases(stack: RaidStack | null, evidence: readonly EvidenceRef[]): ReadonlyMap<string, string> {
  const aliases = new Map<string, string>();
  if (!stack) return aliases;
  const evidencePaths = new Map(evidence.map(item => [item.image_id, item.path]));
  const sourcePaths = new Map(stack.images.map(image => [image.image_id, image.path]));
  const largestFirst = [...stack.arrays].sort((a, b) => b.assembled_bytes - a.assembled_bytes);
  for (const array of largestFirst) {
    if (!array.assembled_image_id) continue;
    const targetPath = evidencePaths.get(array.assembled_image_id);
    if (!targetPath?.startsWith('assembled://')) continue;
    const memberIds = new Set<string>();
    for (const member of array.members) {
      const origin = member.origin;
      if (origin.kind === 'whole') memberIds.add(origin.image_id);
      else {
        const group = stack.volume_groups.find(item => item.vg_uuid === origin.vg_uuid);
        for (const physicalVolume of group?.members ?? []) memberIds.add(physicalVolume.image_id);
      }
    }
    for (const imageId of memberIds) {
      const path = evidencePaths.get(imageId) ?? sourcePaths.get(imageId);
      if (path && path !== targetPath && !aliases.has(raidTabPathKey(path))) aliases.set(raidTabPathKey(path), targetPath);
    }
  }
  return aliases;
}
