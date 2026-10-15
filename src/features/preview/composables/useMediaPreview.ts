
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, ref } from 'vue';
import { fsRead } from '../../../lib/ipc/fs/read';
import { mediaStreamUrl } from '../../../lib/ipc/media';
import { createPreviewRequest } from './previewRequest';
import { MEDIA_CAP, STREAM_MIME_EXT } from '../model/preview';

/** One content viewer owns its Blob URL and cancellable stream-cache lease. */
export function useMediaPreview(options: {
  imageId: () => string;
  nodeId: () => string;
  nodeSize: () => number;
  category: () => string;
  mime: () => string;
  extension: () => string;
  isBlobMedia: () => boolean;
  isStreamMedia: () => boolean;
}) {
  const mediaUrl = ref<string | null>(null), mediaLoading = ref(false), mediaError = ref<string | null>(null);
  const streamUrl = ref<string | null>(null), streamLoading = ref(false), streamError = ref<string | null>(null);
  const streamExt = computed(() => STREAM_MIME_EXT[options.mime()] || options.extension().toLowerCase() || 'bin');
  let streamRequest: ReturnType<typeof createPreviewRequest> | null = null;
  let generation = 0;
  let disposed = false;

  function resetMedia() {
    generation++;
    if (mediaUrl.value) URL.revokeObjectURL(mediaUrl.value);
    mediaUrl.value = null; mediaError.value = null; mediaLoading.value = false;
    streamRequest?.cancel(); streamRequest = null;
    streamUrl.value = null; streamError.value = null; streamLoading.value = false;
  }
  async function loadMedia() {
    if (disposed || !options.isBlobMedia() || mediaUrl.value || mediaLoading.value || options.nodeSize() === 0 || options.nodeSize() > MEDIA_CAP) return;
    const imageId = options.imageId(), nodeId = options.nodeId(), mime = options.mime(), requestGeneration = generation;
    const current = () => !disposed && requestGeneration === generation && imageId === options.imageId() && nodeId === options.nodeId();
    mediaLoading.value = true; mediaError.value = null;
    try {
      const out = await fsRead({ imageId, nodeId });
      if (!current()) return;
      mediaUrl.value = URL.createObjectURL(new Blob([new Uint8Array(out)], { type: mime }));
    } catch (error) { if (current()) mediaError.value = String(error); }
    finally { if (current()) mediaLoading.value = false; }
  }
  async function loadStreamMedia() {
    if (disposed || !options.isStreamMedia() || streamUrl.value || streamLoading.value || options.nodeSize() === 0) return;
    const imageId = options.imageId(), nodeId = options.nodeId();
    const request = createPreviewRequest(); streamRequest = request;
    streamLoading.value = true; streamError.value = null;
    try {
      const path = await request.materialize({ imageId, nodeId, ext: streamExt.value });
      if (!path || disposed || streamRequest !== request) return;
      streamUrl.value = mediaStreamUrl(path);
    } catch (error) { if (!disposed && streamRequest === request) streamError.value = String(error); }
    finally { if (!disposed && streamRequest === request) streamLoading.value = false; }
  }
  function onMediaError() {
    if (!streamUrl.value || streamError.value) return;
    streamError.value = options.category() === 'video'
      ? tr("此视频的编码内核无法解码（如 H.265/HEVC），请导出后用外部播放器查看。")
      : tr("此音频的编码内核无法解码，请导出后用外部播放器查看。");
  }
  onScopeDispose(() => { disposed = true; resetMedia(); });
  return { mediaUrl, mediaLoading, mediaError, streamUrl, streamLoading, streamError, loadMedia, loadStreamMedia, resetMedia, onMediaError };
}
