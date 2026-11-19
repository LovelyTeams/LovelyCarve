import type { ImageInfo } from '../../../lib/ipc/image';
import { evidenceContext } from './evidenceContext';
import type { ImageId } from '../../../lib/ipc/ids';

// Inactive tab snapshots receive the same updated metadata when restored.
const updates = new Map<string, { context: unknown; info: ImageInfo }>();
export function rememberUnlockedImageInfo(info: ImageInfo) {
  updates.set(info.id, { context: evidenceContext(), info });
}
export function latestUnlockedImageInfo(info: ImageInfo | null): ImageInfo | null {
  if (!info) return null;
  const update = updates.get(info.id);
  return update && update.context === evidenceContext() ? update.info : info;
}
export function forgetUnlockedImageInfo(imageId: ImageId) { updates.delete(imageId); }
export function clearUnlockedImageInfo() { updates.clear(); }
