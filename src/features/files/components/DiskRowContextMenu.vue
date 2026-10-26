<template>
  <UiContextMenu
    v-model:open="open"
    :x="x"
    :y="y"
    :entries="entries"
    :label="tr('文件操作')"
    :heading="heading"
    :subheading="subheading"
  >
    <template #heading-icon>
      <ListChecks v-if="scope === 'checked'" :size="15" class="heading-icon" aria-hidden="true" />
      <FileNodeIcon v-else-if="node" :id="node.id" :kind="node.kind" :name="node.name" :size="15" />
    </template>
    <template #before>
      <div class="row-menu-tags" role="group" :aria-label="tr('标签')">
        <TagIcon :size="13" class="row-menu-tags-icon" aria-hidden="true" />
        <button
          v-for="tag in FILE_TAGS"
          :key="tag"
          type="button"
          role="menuitem"
          class="tag-pill mono"
          :class="`tag-${TAG_STYLE[tag]}`"
          :disabled="summary.count === 0"
          :title="summary.count > 1 ? tr('给 {0} 项打「{1}」标签', { 0: summary.count, 1: tr(tag) }) : tr('打「{0}」标签', { 0: tr(tag) })"
          @click="pickTag(tag)"
        >{{ tr(tag) }}</button>
        <button
          type="button"
          role="menuitem"
          class="tag-pill tag-clear mono"
          :disabled="summary.count === 0"
          :title="tr('清除标签')"
          :aria-label="tr('清除标签')"
          @click="pickTag(null)"
        >
          <X :size="11" />
        </button>
      </div>
    </template>
  </UiContextMenu>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import {
  ClipboardCopy,
  ClipboardList,
  Copy,
  Eye,
  FileDown,
  Fingerprint,
  FolderDown,
  FolderOpen,
  FolderSearch,
  HardDriveDownload,
  HardDriveUpload,
  Info,
  ListChecks,
  Puzzle,
  ScanSearch,
  Tag as TagIcon,
  X,
} from 'lucide-vue-next';
import UiContextMenu, { type UiContextMenuEntry } from '../../../components/ui/UiContextMenu.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { compactMenuEntries } from '../../../utils/menuPlacement';
import type { HashAlgo } from '../../../lib/ipc/fs/derived';
import { formatBytes, kindLabel, type DiskFsNode } from '../model/model';
import { FILE_TAGS, TAG_STYLE, type FileTag } from '../model/tags';
import type { FileMenuScope } from '../composables/useFileActions';

/** The menu of a file-list row or treemap tile. Its heading names what the
 *  batch actions act on: the row itself, or every checked row when it was
 *  opened on a checked one. */
const props = defineProps<{
  x: number;
  y: number;
  node: DiskFsNode | null;
  scope: FileMenuScope;
  summary: { count: number; files: number; dirs: number; bytes: number };
  indexOnly: boolean;
  canReturnToOriginalDir: boolean;
  canPluginTask: boolean;
  mount: { available: boolean; busy: boolean; mounted: boolean; label: string };
  hashOptions: { label: string; algos: HashAlgo[] }[];
}>();
const open = defineModel<boolean>('open', { required: true });
const emit = defineEmits<{
  'open-node': [];
  'show-detail': [];
  'return-to-original-dir': [];
  'toggle-mount': [];
  'export-file': [];
  'export-dir': [];
  'compute-hash': [algos: HashAlgo[]];
  'identify-type': [];
  'plugin-task': [];
  'copy-names': [];
  'copy-paths': [];
  'copy-forensic': [];
  'tag-pick': [tag: FileTag | null];
}>();

const heading = computed(() => {
  if (props.scope === 'checked') return tr('已勾选 {0} 项', { 0: props.summary.count.toLocaleString() });
  return props.node?.name ?? '';
});

const subheading = computed(() => {
  const node = props.node;
  if (props.scope === 'checked') {
    const { files, dirs, bytes } = props.summary;
    return [
      files > 0 ? tr('{0} 个文件', { 0: files.toLocaleString() }) : '',
      dirs > 0 ? tr('{0} 个文件夹', { 0: dirs.toLocaleString() }) : '',
      files > 0 ? formatBytes(bytes) : '',
    ].filter(Boolean).join(' · ');
  }
  if (!node) return '';
  return [kindLabel(node, tr), node.kind === 'file' ? formatBytes(node.size) : '', node.deleted ? tr('已删除') : '']
    .filter(Boolean).join(' · ');
});

function pickTag(tag: FileTag | null) {
  open.value = false;
  emit('tag-pick', tag);
}

const entries = computed<UiContextMenuEntry[]>(() => {
  const node = props.node;
  if (!node) return [];
  const checked = props.scope === 'checked';
  const { count, files, dirs } = props.summary;
  const offline = props.indexOnly;
  const offlineTitle = offline ? tr('离线索引 · 需恢复源镜像') : undefined;
  const manyFiles = checked && files > 1;
  const many = checked && count > 1;
  const list: UiContextMenuEntry[] = [];

  if (node.kind === 'dir') {
    list.push({ id: 'open', label: tr('打开'), icon: FolderOpen, hint: 'Enter', action: () => emit('open-node') });
    list.push({ id: 'detail', label: tr('查看详情'), icon: Info, action: () => emit('show-detail') });
  } else {
    list.push({ id: 'preview', label: tr('预览'), icon: Eye, hint: 'Enter', action: () => emit('show-detail') });
  }
  if (props.canReturnToOriginalDir) {
    list.push({
      id: 'reveal', label: tr('回到原始目录'), icon: FolderSearch, title: tr('跳到该条目的所在目录，并清除当前筛选'),
      action: () => emit('return-to-original-dir'),
    });
  }

  list.push({ kind: 'separator' });
  if (files > 0) {
    list.push({
      id: 'export-file', label: checked ? tr('导出勾选文件（{0}）', { 0: files.toLocaleString() }) : tr('导出文件'),
      icon: FileDown, disabled: offline, title: offlineTitle, action: () => emit('export-file'),
    });
  }
  if (dirs > 0) {
    list.push({
      id: 'export-dir', label: checked ? tr('导出勾选文件夹（{0}）', { 0: dirs.toLocaleString() }) : tr('导出文件夹'),
      icon: FolderDown, disabled: offline, title: offlineTitle, action: () => emit('export-dir'),
    });
  }
  if (props.mount.available) {
    list.push({
      id: 'mount', label: props.mount.label, icon: props.mount.mounted ? HardDriveUpload : HardDriveDownload,
      busy: props.mount.busy, disabled: props.mount.busy || offline, title: offlineTitle, action: () => emit('toggle-mount'),
    });
  }

  list.push({ kind: 'separator' });
  if (files > 0) {
    list.push({
      kind: 'submenu', id: 'hash', icon: Fingerprint, disabled: offline, title: offlineTitle,
      label: manyFiles ? tr('计算勾选文件哈希（{0}）', { 0: files.toLocaleString() }) : tr('计算哈希'),
      items: props.hashOptions.map((option, index) => ({
        id: `hash:${index}`, label: tr(option.label), action: () => emit('compute-hash', option.algos),
      })),
    });
    list.push({
      id: 'identify', icon: ScanSearch, disabled: offline,
      label: manyFiles ? tr('识别勾选文件类型（{0}）', { 0: files.toLocaleString() }) : tr('识别文件类型'),
      title: offlineTitle ?? tr('Magika 按文件内容识别真实类型，不依赖扩展名，可发现修改扩展名伪装的文件'),
      action: () => emit('identify-type'),
    });
  }
  if (node.kind === 'file') {
    list.push({
      id: 'plugin', label: tr('插件分析任务…'), icon: Puzzle, disabled: offline || !props.canPluginTask,
      title: offlineTitle ?? (props.canPluginTask
        ? tr('提交到任务中心，由已启用的 LPP 插件 function 分析该文件；声明了依赖链的 function 会自动接续')
        : tr('插件分析任务一次分析一个文件')),
      action: () => emit('plugin-task'),
    });
  }

  list.push({ kind: 'separator' });
  const shown = count.toLocaleString();
  list.push({ id: 'copy-names', label: many ? tr('复制名称（{0}）', { 0: shown }) : tr('复制名称'), icon: Copy, action: () => emit('copy-names') });
  list.push({ id: 'copy-paths', label: many ? tr('复制完整路径（{0}）', { 0: shown }) : tr('复制完整路径'), icon: ClipboardCopy, action: () => emit('copy-paths') });
  list.push({ id: 'copy-forensic', label: many ? tr('复制取证信息（{0}）', { 0: shown }) : tr('复制取证信息'), icon: ClipboardList, action: () => emit('copy-forensic') });
  return compactMenuEntries(list);
});
</script>

<style scoped>
.row-menu-tags {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
}

.row-menu-tags-icon,
.heading-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}
</style>

<style scoped src="../styles/file-tags.css"></style>
