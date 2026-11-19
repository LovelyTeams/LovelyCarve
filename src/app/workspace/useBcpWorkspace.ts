import { computed, reactive, ref, watch, type Ref } from 'vue';
import { bcpCatalog, type BcpCatalog } from '../../lib/ipc/bcp';
import type { ImageInfo } from '../../lib/ipc/image';
import type { ImageId } from '../../lib/ipc/ids';

/** BCP navigation metadata follows the active folder-backed evidence. */
export function useBcpWorkspace(imageId: Ref<ImageId | null>, imageInfo: Ref<ImageInfo | null>) {
  const bcpCatalogData = ref<BcpCatalog | null>(null);
  const bcpCatalogError = ref('');
  const preferredBcpFiles = reactive(new Map<string, string>());
  const bcpPreferredFile = computed(() => preferredBcpFiles.get(imageInfo.value?.path ?? '') ?? '');
  let bcpCatalogRequest = 0;
  watch(() => [imageId.value, imageInfo.value?.source_format], async () => {
    const request = ++bcpCatalogRequest;
    bcpCatalogData.value = null;
    bcpCatalogError.value = '';
    if (!imageId.value || imageInfo.value?.source_format !== 'folder') return;
    try {
      const result = await bcpCatalog(imageId.value);
      if (request !== bcpCatalogRequest) return;
      bcpCatalogData.value = result;
    } catch (cause) {
      if (request === bcpCatalogRequest) bcpCatalogError.value = String(cause);
    }
  });
  return { bcpCatalogData, bcpCatalogError, preferredBcpFiles, bcpPreferredFile };
}
