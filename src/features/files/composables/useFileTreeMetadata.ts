import { containerFsChildren as fsChildren } from './containerReads';
import { fsChildCounts, fsSubtreeEmptyDirs } from '../../../lib/ipc/fs/query';
import { winDriveLetters } from '../../../lib/ipc/filesystemMetadata';
import { isHostDockerContainerRootId } from '../model/model';
interface FileTreeMetadataOptions {
  imageId: () => string; current: (generation: number) => boolean;
  publishCounts: (counts: Record<string, number>) => void;
  publishEmpty: (empty: Record<string, boolean>) => void;
  publishLetters: (letters: { id: string; letter: string }[]) => void;
}
/** Bounded best-effort probes publish only into the active tree generation. */
export function useFileTreeMetadata(options: FileTreeMetadataOptions) {
  async function fetchDriveLetters(gen: number) {
    if (!options.imageId()) return;
    try {
      const report = await winDriveLetters({ imageId: options.imageId() });
      if (!options.current(gen)) return;
      options.publishLetters(report.assignments.map(assignment => ({ id: `part:${assignment.partition_id}`, letter: report.ambiguous ? `${assignment.drive_letter}?` : assignment.drive_letter })));
    } catch {
      // 非 Windows 源、上锁的 BitLocker、或无 MountedDevices——静默跳过，
      // 浏览不受影响（与 fetchChildCounts 的尽力而为一致）。
    }
  }

  async function fetchChildCounts(ids: string[], gen: number) {
    if (ids.length === 0) return;

    let covered: Record<string, number> = {};
    try {
      covered = await fsChildCounts({ imageId: options.imageId(), parentIds: ids });
      if (!options.current(gen)) return;
      options.publishCounts(covered);
    } catch {
      // No active case / command unavailable — fall through to live probing.
    }

    // Do not compose a Docker container merely to paint its count badge. Its
    // first explicit expansion owns that work and exposes the loading bar.
    if (!options.current(gen)) return;
    const targets = ids.filter(
      id => covered[id] == null && !isHostDockerContainerRootId(id),
    );
    if (targets.length === 0) return;
    const CONCURRENCY = 6;
    for (let i = 0; i < targets.length; i += CONCURRENCY) {
      if (!options.current(gen)) return;
      const batch = targets.slice(i, i + CONCURRENCY);
      await Promise.allSettled(
        batch.map(async id => {
          try {
            const kids = await fsChildren({
              imageId: options.imageId(),
              parentId: id,
            });
            if (!options.current(gen)) return;
            options.publishCounts({ [id]: kids.length });
          } catch {
            // Best-effort; leave the badge blank if the partition is
            // locked or the FS is unsupported.
          }
        }),
      );
    }
  }

  /// Background-fill `emptySubtreeOf` for `ids` from the volume snapshot: which of
  /// these directories have a subtree containing no files (only empty dirs, at
  /// any depth). Snapshot-only — ids the snapshot can't decide are left unset so
  /// the filter falls back to the direct child-count signal. Best-effort: no
  /// snapshot / no case yet just leaves the map untouched (filter degrades to
  /// hiding only directly-empty dirs).
  async function fetchEmptySubtrees(ids: string[], gen: number) {
    // A container root is a synthetic navigation/mount entry. It remains
    // visible before overlay composition and is never classified as empty.
    const targets = ids.filter(id => !isHostDockerContainerRootId(id));
    if (targets.length === 0) return;
    try {
      const result = await fsSubtreeEmptyDirs({ imageId: options.imageId(), parentIds: targets });
      if (!options.current(gen)) return;
      options.publishEmpty(result);
    } catch {
      // No snapshot / command unavailable — leave entries unset; the filter
      // falls back to childCount===0 (directly-empty) for these dirs.
    }
  }

  return { fetchDriveLetters, fetchChildCounts, fetchEmptySubtrees };
}
