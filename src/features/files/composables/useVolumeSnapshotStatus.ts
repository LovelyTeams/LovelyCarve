
import { tr } from '../../../composables/useLocale';
import { computed, type Ref } from 'vue';
import { useCaseSnapshots } from '../../cases/api';

interface StackFrame {
  id: string;
}

export interface SnapshotStatus {
  state: 'building' | 'ready' | 'stale';
  label: string;
}

export function volumeKeyOfId(id: string): string {
  if (id.startsWith('part:')) return id;
  const part = id.match(/(?:^|:)part:(\d+)(?::|$)/);
  if (part) return `part:${part[1]}`;
  const lvm = id.match(/(?:^|:)lvm:(\d+):/);
  if (lvm) return `part:${lvm[1]}`;
  return 'root';
}

export function useVolumeSnapshotStatus(options: {
  imageId: () => string;
  stack: Ref<StackFrame[]>;
}) {
  const { activeCase, snapshots, buildProgress } = useCaseSnapshots();

  // Partition roots key by their `part:N` id (one snapshot per partition covers
  // the whole volume, incl. LVM logical volumes inside it); docker / folder
  // evidence keys by `root`. Flat mode can jump from root to a deep fs node, so
  // derive from id shape instead of relying on stack prefixes.
  const volumeKey = computed<string | null>(() => {
    const frame = options.stack.value[1];
    if (!frame) return null;
    return volumeKeyOfId(frame.id);
  });

  const snapshotStatus = computed<SnapshotStatus | null>(() => {
    const key = volumeKey.value;
    if (!key) return null;
    const imageId = options.imageId();
    const building = buildProgress.value.get(`${imageId}:${key}`);
    if (
      building &&
      (building.phase === 'walking' || building.phase === 'started' || building.phase === 'indexing')
    ) {
      return { state: 'building', get label() { return tr("快照构建中 · {0} 条", { 0: building.entries.toLocaleString() }); } };
    }
    const snap = snapshots.value.find(s => s.image_id === imageId && s.volume_key === key);
    if (!snap) return null;
    if (snap.stale) return { state: 'stale', get label() { return tr("快照已过期"); } };
    const count = snap.entry_count.toLocaleString();
    return {
      state: 'ready',
      label: snap.truncated ? tr("快照就绪 · {0} 条（截断）", { 0: count }) : tr("快照就绪 · {0} 条", { 0: count }),
    };
  });

  return {
    activeCase,
    volumeKey,
    snapshotStatus,
  };
}
