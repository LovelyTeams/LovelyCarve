
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, type Ref } from 'vue';
import { fsPreview } from '../../../lib/ipc/fs/read';
import { fsIdentifyType, type FsTypeIdent } from '../../../lib/ipc/fs/derived';
import type { FsNode } from '../../../lib/ipc/fs/types';
import { notifyError } from '../../../composables/useToast';
import { PREVIEW_CAP, type DetailTab } from '../model/preview';

/** Prefix-read and local-identification state belong to one content-viewer instance. */
export function usePreviewSession(options: {
  imageId: () => string;
  indexOnly?: () => boolean;
  node: () => FsNode;
  activeTab: Ref<DetailTab>;
  page: Ref<number>;
  category: () => string;
  resetForNode: () => void;
  resetMedia: () => void;
  loadNativeMedia: () => void;
  ensureDecodedText: () => Promise<void>;
  identified: (payload: { nodeId: string; ident: FsTypeIdent }) => void;
}) {
  const loading = ref(false), error = ref<string | null>(null), bytes = ref<Uint8Array>(new Uint8Array());
  const localIdent = ref<FsTypeIdent | null>(null), identifying = ref(false);
  let generation = 0;
  let identifyingGeneration = 0;
  let disposed = false;
  let previousWasFile = false;
  let fileTab: DetailTab = 'raw';
  function current(requestGeneration: number, imageId: string, nodeId: string) {
    return !disposed && requestGeneration === generation && imageId === options.imageId() && nodeId === options.node().id;
  }
  async function loadPreview() {
    if (disposed) return;
    if (previousWasFile) fileTab = options.activeTab.value;
    const requestGeneration = ++generation;
    identifyingGeneration++; identifying.value = false;
    localIdent.value = null; bytes.value = new Uint8Array();
    options.resetForNode();
    loading.value = false; error.value = null; options.page.value = 1;
    const node = options.node(), imageId = options.imageId();
    previousWasFile = node.kind === 'file';
    if (options.indexOnly?.()) {
      options.activeTab.value = 'detail';
      error.value = tr("源镜像不可用，仅能查看索引中的文件属性；请恢复源镜像后重试。");
      return;
    }
    options.activeTab.value = previousWasFile ? fileTab : 'detail';
    if (node.kind !== 'file' || node.size === 0) return;
    options.loadNativeMedia();
    loading.value = true;
    try {
      const out = await fsPreview({ imageId, nodeId: node.id, maxBytes: PREVIEW_CAP });
      if (!current(requestGeneration, imageId, node.id)) return;
      bytes.value = new Uint8Array(out);
      void options.ensureDecodedText();
    } catch (reason) {
      if (!current(requestGeneration, imageId, node.id)) return;
      error.value = String(reason); bytes.value = new Uint8Array();
    } finally { if (current(requestGeneration, imageId, node.id)) loading.value = false; }
  }
  async function identifyType() {
    const node = options.node();
    if (disposed || options.indexOnly?.() || node.kind !== 'file' || identifying.value) return;
    const imageId = options.imageId(), requestGeneration = generation, identityGeneration = ++identifyingGeneration;
    const valid = () => identityGeneration === identifyingGeneration && current(requestGeneration, imageId, node.id);
    identifying.value = true;
    try {
      const ident = await fsIdentifyType({ imageId, nodeId: node.id });
      if (!valid()) return;
      localIdent.value = ident;
      options.identified({ nodeId: node.id, ident });
      options.resetMedia();
      if (options.category() !== 'unsupported') {
        options.activeTab.value = 'raw';
        options.loadNativeMedia();
      }
    } catch (reason) { if (valid()) notifyError(tr("类型识别失败"), String(reason)); }
    finally { if (valid()) identifying.value = false; }
  }
  onScopeDispose(() => { disposed = true; generation++; identifyingGeneration++; });
  return { loading, error, bytes, localIdent, identifying, loadPreview, identifyType };
}
