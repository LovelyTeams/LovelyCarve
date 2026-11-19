// Media streaming IPC: materialise a video / audio node inside a disk image to
// a per-session temp file (backend `fs_media_temp`) and resolve the asset-
// protocol URL for it. The in-memory `fsRead` Blob path is capped at 16 MiB and
// cannot seek; the asset protocol serves the temp file with HTTP Range support,
// so clips of any size stream and scrub.
//
// This module is part of the lib/ipc service boundary, so it is allowed to
// import `@tauri-apps/api/core` (the only place besides client.ts that does).
import { convertFileSrc } from '@tauri-apps/api/core';
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** Materialise a media node to a temp file and return its absolute path. Streams
 *  a full copy (long for large clips) so it is cancellable via `taskId`
 *  (`taskCancel`). `ext` hints the temp file's extension so the asset protocol
 *  content-types it correctly — pass the extension derived from the effective /
 *  identified MIME so a renamed clip still decodes. */
export function fsMediaTemp(args: {
  imageId: ImageId;
  nodeId: NodeId;
  ext?: string;
  taskId: string;
}): Promise<string> {
  return invokeCmd<string>('fs_media_temp', args);
}

/** Asset-protocol URL for a materialised temp file path, for a `<video>` /
 *  `<audio>` `src`. `convertFileSrc` builds the platform-correct origin
 *  (`https://asset.localhost/…` on Windows, `asset://localhost/…` elsewhere). */
export function mediaStreamUrl(path: string): string {
  return convertFileSrc(path);
}

/** Release the preview lease after the element/viewer stops using its URL. */
export function fsMediaRelease(taskId: string): Promise<void> {
  return invokeCmd<void>('fs_media_release', { taskId });
}
export function newPreviewTaskId(): string { return crypto.randomUUID(); }
