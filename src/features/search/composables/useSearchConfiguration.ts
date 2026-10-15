
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onScopeDispose, reactive, ref, shallowRef, watch } from 'vue';
import { fsChildren } from '../../../lib/ipc/fs/query';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { GlobalSearchOptions, SearchCodePage } from '../../../lib/ipc/search';
import { createSearchDraft, searchConfigurationError, buildSearchOptions } from '../model/searchConfiguration';
import { parseSearchPatterns } from '../model/searchResults';
interface SearchConfigurationOptions { imageId: () => string; onSubmit: (options: GlobalSearchOptions) => void; }
export function useSearchConfiguration(options: SearchConfigurationOptions) {
  let generation = 0;
  let disposed = false;
  const current = (captured: number) => !disposed && captured === generation;
  // ── 新建搜索任务对话框 ─────────────────────────────────────────────
  const keywordInput = ref<HTMLTextAreaElement | null>(null);
  const dialogVisible = ref(false);
  const starting = ref(false);
  const loadingPartitions = ref(false);
  const partitionOptions = shallowRef<{ id: string; label: string; title: string }[]>([]);

  const dialog = reactive(createSearchDraft());

  const scopeSummary = computed(() => {
    if (dialog.scopeMode === 'tags') return dialog.tags.length ? tr("标签：{0}", { 0: dialog.tags.map(tag => tr(tag)).join(', ') }) : tr("全部文件（未限制标签）");
    if (dialog.scopeMode === 'tree_nodes') {
      const paths = parseSearchPatterns(dialog.treeNodesText);
      return paths.length ? tr("{0} 个目录或文件", { 0: paths.length }) : tr("全部文件（未限制目录）");
    }
    return dialog.partitions.length ? tr("已选 {0} 个分区", { 0: dialog.partitions.length }) : tr("全部分区");
  });
  const advancedSummary = computed(() => [
    tr("{0} 种编码", { 0: dialog.codepages.length }),
    dialog.searchDeleted ? tr("含已删除文件") : tr("不含已删除文件"),
    ...(dialog.searchEmptySpace ? [tr("含未分配空间")] : []),
    ...(dialog.regexesText.trim() ? [tr("含正则")] : []),
    ...(dialog.calcHash ? [tr("计算哈希")] : []),
    ...(dialog.matchCase ? [tr("区分大小写")] : []),
    ...(dialog.stopAfterFirst ? [tr("每文件首命中")] : []),
  ].join(' · '));
  const dialogError = computed(() => searchConfigurationError(dialog, tr));

  function toggleCodePage(page: SearchCodePage) {
    const index = dialog.codepages.indexOf(page);
    if (index >= 0) dialog.codepages.splice(index, 1);
    else dialog.codepages.push(page);
  }

  async function openCreateDialog() {
    dialogVisible.value = true;
    const captured = generation;
    void nextTick(() => { if (current(captured) && dialogVisible.value) keywordInput.value?.focus(); });
    if (partitionOptions.value.length === 0 && !loadingPartitions.value) {
      loadingPartitions.value = true;
      try {
        const children = await fsChildren({ imageId: options.imageId(), parentId: 'root' });
        if (!current(captured)) return;
        partitionOptions.value = children
          .filter(node => node.id.startsWith('part:'))
          .map((node: FsNode) => ({
            id: node.id,
            label: node.name,
            title: `${node.name}（${node.id}）`,
          }));
      } catch {
        if (current(captured)) partitionOptions.value = [];
      } finally {
        if (current(captured)) loadingPartitions.value = false;
      }
    }
  }

  function closeCreateDialog() {
    if (starting.value) return;
    dialogVisible.value = false;
  }

  function submitCreateDialog() {
    if (dialogError.value || disposed) return;
    const configured = buildSearchOptions(dialog);
    starting.value = true;
    try { dialogVisible.value = false; options.onSubmit(configured); }
    finally { starting.value = false; }
  }
  watch(options.imageId, () => {
    generation += 1;
    partitionOptions.value = [];
    loadingPartitions.value = false;
    if (dialogVisible.value) void openCreateDialog();
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation += 1; });
  const elements = { keyword: keywordInput };
  return { dialog, dialogVisible, starting, loadingPartitions, partitionOptions, scopeSummary, advancedSummary, dialogError, elements, toggleCodePage, openCreateDialog, closeCreateDialog, submitCreateDialog };
}
