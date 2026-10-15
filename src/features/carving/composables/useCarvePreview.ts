
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { carvePreview, type CarveArtifact, type CarveScanSnapshot } from '../../../lib/ipc/carve';
export function useCarvePreview(options: { imageId: () => string; scan: () => CarveScanSnapshot | null; artifact: () => CarveArtifact | null }) {
  const MAX_IMAGE_PREVIEW_BYTES = 8 * 1024 ** 2;
  const previewLoading = ref(false); const previewUrl = ref<string | null>(null);
  let previewRequest = 0; let disposed = false;
  const previewMessage = computed(() => {
    const artifact = options.artifact();
    if (!artifact) return '';
    if (artifact.category !== 'image') return tr("此格式暂不提供内嵌渲染，可恢复后使用原类型视图复核。");
    if (artifact.length > MAX_IMAGE_PREVIEW_BYTES) return tr("图像超过 8 MB 预览上限，可恢复后查看。");
    return tr("无法渲染该图像，结构可能不完整。");
  });

  function revokePreview() {
    if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
    previewUrl.value = null;
  }

  async function loadPreview(artifact: CarveArtifact | null) {
    const request = ++previewRequest;
    revokePreview();
    const scan = options.scan();
    if (!artifact || artifact.category !== 'image' || artifact.length > MAX_IMAGE_PREVIEW_BYTES || !scan) {
      previewLoading.value = false;
      return;
    }
    previewLoading.value = true;
    try {
      const bytes = await carvePreview({
        scanId: scan.scan_id,
        artifactId: artifact.id,
        maxBytes: artifact.length,
      });
      if (disposed || request !== previewRequest) return;
      const mime = {
        jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', gif: 'image/gif', bmp: 'image/bmp',
      }[artifact.extension.toLocaleLowerCase()] ?? 'application/octet-stream';
      previewUrl.value = URL.createObjectURL(new Blob([bytes], { type: mime }));
    } catch (error) {
      if (!disposed && request === previewRequest) console.error('carve preview failed:', error);
    } finally {
      if (!disposed && request === previewRequest) previewLoading.value = false;
    }
  }


  watch([options.imageId, () => options.scan()?.scan_id, options.artifact], () => { void loadPreview(options.artifact()); }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; previewRequest++; revokePreview(); });
  return { previewLoading, previewUrl, previewMessage };
}
