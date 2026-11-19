import { onBeforeUnmount, ref, watch } from 'vue';
import { shadowCopyList, shadowCopyOpen, type ShadowCopyInfo, type ShadowVolumeInfo } from '../../../lib/ipc/shadow';
import type { ImageId } from '../../../lib/ipc/ids';

/** A failed open, kept with its volume so that volume's copy list shows it. */
export interface ShadowOpenError {
  partitionId: number;
  message: string;
}

/** Stable key of one copy, used for the in-flight open. */
export function shadowCopyKey(volume: ShadowVolumeInfo, copy: ShadowCopyInfo): string {
  return `${volume.partition_id}:${copy.store_id}`;
}

/**
 * Volume Shadow Copies of one image's NTFS volumes, for the partition rows of
 * the image info page. `imageId` returns null for evidence that cannot hold
 * VSS (logical evidence, or a shadow copy session itself). Answers that arrive
 * after the page moved to another image, or unmounted, are dropped.
 */
export function useShadowCopies(imageId: () => ImageId | null) {
  const volumes = ref<ShadowVolumeInfo[]>([]);
  const loadError = ref('');
  const openError = ref<ShadowOpenError | null>(null);
  /** `shadowCopyKey` of the copy being rebuilt; one open at a time. */
  const opening = ref<string | null>(null);
  let generation = 0;

  async function load() {
    const id = imageId();
    const current = ++generation;
    volumes.value = [];
    loadError.value = '';
    openError.value = null;
    opening.value = null;
    if (!id) return;
    try {
      const listed = await shadowCopyList({ imageId: id });
      if (current === generation) volumes.value = listed;
    } catch (cause) {
      if (current === generation) loadError.value = String(cause);
    }
  }

  /** Rebuild (or reuse) the session image of one copy. Resolves to its path,
   *  or null when opening failed or the page moved on. */
  async function open(volume: ShadowVolumeInfo, copy: ShadowCopyInfo): Promise<string | null> {
    const id = imageId();
    if (!id || opening.value) return null;
    const current = generation;
    opening.value = shadowCopyKey(volume, copy);
    openError.value = null;
    try {
      const image = await shadowCopyOpen({
        imageId: id,
        partitionId: volume.partition_id,
        index: copy.index,
        taskId: crypto.randomUUID(),
      });
      if (current !== generation) return null;
      copy.open_image_id = image.id;
      return image.path;
    } catch (cause) {
      if (current === generation) openError.value = { partitionId: volume.partition_id, message: String(cause) };
      return null;
    } finally {
      if (current === generation) opening.value = null;
    }
  }

  function volumeOf(partitionId: number): ShadowVolumeInfo | undefined {
    return volumes.value.find(volume => volume.partition_id === partitionId);
  }

  watch(imageId, load, { immediate: true });
  onBeforeUnmount(() => {
    generation += 1;
  });

  return { volumes, loadError, openError, opening, load, open, volumeOf };
}
