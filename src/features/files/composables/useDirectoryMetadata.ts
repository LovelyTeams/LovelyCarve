import { onScopeDispose, reactive } from 'vue';
import { fsChildCounts, fsSubtreeEmptyDirs } from '../../../lib/ipc/fs/query';
import { containerFsChildrenPage as fsChildrenPage } from './containerReads';
import { isHostDockerContainerRootId, type DiskFsNode as FsNode } from '../model/model';

/** Snapshot-first counts and bounded live probes; synthetic Docker roots stay lazy. */
export function useDirectoryMetadata(options: { imageId: () => string; generation: () => number }) {
  const dirCounts = reactive<Record<string, number>>({});
  const emptySubtree = reactive<Record<string, boolean>>({});
  const pending = new Set<string>();
  let disposed = false;
  onScopeDispose(() => { disposed = true; });
  const current = (generation: number) => !disposed && generation === options.generation();

  async function fetchDirCounts(nodes: FsNode[], gen: number) {
    const imageId = options.imageId();
    const allDirs = nodes.filter(n => n.kind === 'dir');
    if (allDirs.length === 0) return;

    // Fast path: ask the active case's volume snapshot for every subdir's child
    // count in one call. A snapshot hit returns the exact count without any
    // filesystem walk, so a whole directory's badges come back at once with no
    // per-subdir fan-out and no probe cap.
    let covered: Record<string, number> = {};
    try {
      covered = await fsChildCounts({
        imageId: imageId,
        parentIds: allDirs.map(d => d.id),
      });
      if (!current(gen)) return;
      for (const [id, count] of Object.entries(covered)) {
        dirCounts[id] = count;
      }
    } catch {
      // No active case / command unavailable — fall through to live probing.
    }

    // Live fallback only for the subdirs the snapshot did NOT cover (no case,
    // still building, or beyond the truncation cap). Each probe loads a full
    // child list, so a directory with thousands of uncovered subdirs would fan
    // out thousands of fsChildren IPCs — cap the probe volume; dirs past the cap
    // simply render without a count badge.
    // A container root is expensive: probing its count composes every image
    // layer. Keep that work lazy so it starts only when the user enters the
    // container and the dedicated loading bar can describe the real wait.
    const uncovered = allDirs.filter(
      d => covered[d.id] == null && !isHostDockerContainerRootId(d.id),
    );
    if (uncovered.length === 0) return;
    const MAX_DIR_COUNT_PROBES = 600;
    const dirs = uncovered.slice(0, MAX_DIR_COUNT_PROBES);
    const concurrency = 6;
    for (let i = 0; i < dirs.length; i += concurrency) {
      if (!current(gen)) return;
      const batch = dirs.slice(i, i + concurrency);
      await Promise.allSettled(
        batch.map(async d => {
          try {
            const page = await fsChildrenPage({ imageId: imageId, parentId: d.id, pageSize: 1 });
            if (!current(gen)) return;
            if (page.total != null) dirCounts[d.id] = page.total;
          } catch {
            // best-effort; leave count blank if a partition/volume errors
          }
        }),
      );
    }
  }

  /// Background-fill `emptySubtree` for the directories in `nodes` from the volume
  /// snapshot: which have a subtree containing no files (only empty dirs, at any
  /// depth). Snapshot-only — dirs the snapshot can't decide are left unset so the
  /// filter falls back to the direct child-count (`dirCounts===0`). Mirrors the
  /// directory tree's fetch so both panels hide the same dirs. Only invoked when
  /// the "hide empty directories" toggle is on.
  async function fetchEmptySubtrees(nodes: FsNode[], gen: number) {
    // Container roots are navigable/mountable synthetic entries rather than
    // ordinary directories. They must remain visible even before their costly
    // overlay has been composed, so do not ask the snapshot to classify them.
    const imageId = options.imageId();
    const dirs = nodes.filter(
      n => n.kind === 'dir' && !isHostDockerContainerRootId(n.id),
    );
    if (dirs.length === 0) return;
    try {
      const result = await fsSubtreeEmptyDirs({
        imageId: imageId,
        parentIds: dirs.map(d => d.id),
      });
      if (!current(gen)) return;
      for (const [id, empty] of Object.entries(result)) emptySubtree[id] = empty;
    } catch {
      // No snapshot / command unavailable — leave unset; the filter falls back
      // to dirCounts===0 (directly-empty) for these dirs.
    }
  }

  function clear() {
    pending.clear();
    for (const key of Object.keys(dirCounts)) delete dirCounts[key];
    for (const key of Object.keys(emptySubtree)) delete emptySubtree[key];
  }

  function hydrateVisible(nodes: FsNode[]) {
    const missing = nodes.filter(node => node.kind === 'dir' && dirCounts[node.id] == null && !pending.has(node.id));
    const generation = options.generation();
    for (const node of missing) pending.add(node.id);
    void fetchDirCounts(missing, generation).finally(() => {
      if (current(generation)) for (const node of missing) pending.delete(node.id);
    });
  }

  return { dirCounts, emptySubtree, clear, hydrateVisible, fetchEmptySubtrees };
}
