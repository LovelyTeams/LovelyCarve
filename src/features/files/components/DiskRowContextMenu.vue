<template>
  <Teleport to="body">
    <div
      v-if="visible"
      class="ui-menu disk-row-menu"
      :style="{ left: `${x}px`, top: `${y}px` }"
      role="menu"
      @click.stop
      @contextmenu.prevent
    >
      <div class="disk-row-menu-tags">
        <TagIcon :size="13" class="menu-tags-icon" />
        <button
          v-for="tag in FILE_TAGS"
          :key="tag"
          class="tag-pill mono"
          :class="`tag-${TAG_STYLE[tag]}`"
          :disabled="copyCount === 0"
          :title="copyCount > 1 ? tr('给勾选的 {0} 项打「{1}」标签', { 0: copyCount, 1: tr(tag) }) : tr('打「{0}」标签', { 0: tr(tag) })"
          @click="$emit('tag-pick', tag)"
        >{{ tr(tag) }}</button>
        <button
          class="tag-pill tag-clear mono"
          :disabled="copyCount === 0"
          :title="tr('清除标签')"
          @click="$emit('tag-pick', null)"
        >
          <X :size="11" />
        </button>
      </div>
      <div class="ui-menu-sep" role="separator"></div>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!hasNode"
        @click="$emit('show-detail')"
      >
        <Info :size="13" />
        <span>{{ tr("查看详情") }}</span>
      </button>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!canReturnToOriginalDir"
        :title="tr('跳到该条目的所在目录，并清除当前筛选')"
        @click="$emit('return-to-original-dir')"
      >
        <FolderOpen :size="13" />
        <span>{{ tr("回到原始目录") }}</span>
      </button>
      <button
        v-if="canMount"
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="mountBusy"
        @click="$emit('toggle-mount')"
      >
        <Loader2 v-if="mountBusy" :size="13" class="spin" />
        <HardDriveUpload v-else-if="mounted" :size="13" />
        <HardDriveDownload v-else :size="13" />
        <span>{{ mountLabel }}</span>
      </button>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!canExportFile"
        @click="$emit('export-file')"
      >
        <FileDown :size="13" />
        <span>{{ checkedFileCount > 0 ? tr("导出勾选文件（{0}）", { 0: checkedFileCount }) : tr("导出文件") }}</span>
      </button>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!canExportDir"
        @click="$emit('export-dir')"
      >
        <FolderDown :size="13" />
        <span>{{ checkedDirCount > 0 ? tr("导出勾选文件夹（{0}）", { 0: checkedDirCount }) : tr("导出文件夹") }}</span>
      </button>
      <div
        class="disk-row-submenu-wrap"
        @mouseenter="canHash && $emit('update:hashMenuOpen', true)"
        @mouseleave="$emit('update:hashMenuOpen', false)"
      >
        <button
          class="ui-menu-item disk-row-menu-item"
          role="menuitem"
          aria-haspopup="menu"
          :aria-expanded="hashMenuOpen"
          :disabled="!canHash"
          @click="canHash && $emit('update:hashMenuOpen', !hashMenuOpen)"
        >
          <Fingerprint :size="13" />
          <span>{{ checkedFileCount > 1 ? tr("计算勾选文件哈希（{0}）", { 0: checkedFileCount }) : tr("计算哈希") }}</span>
          <ChevronRight :size="12" class="submenu-caret" />
        </button>
        <div v-if="hashMenuOpen && canHash" class="ui-menu disk-row-submenu" :class="{ flip: hashMenuFlip }" role="menu">
          <button
            v-for="option in hashOptions"
            :key="option.label"
            class="ui-menu-item disk-row-menu-item"
            role="menuitem"
            @click="$emit('compute-hash', option.algos)"
          >
            <span class="mono">{{ tr(option.label) }}</span>
          </button>
        </div>
      </div>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!canIdentify"
        :title="tr('Magika 按文件内容识别真实类型，不依赖扩展名，可发现修改扩展名伪装的文件')"
        @click="$emit('identify-type')"
      >
        <ScanSearch :size="13" />
        <span>{{ checkedFileCount > 1 ? tr("识别勾选文件类型（{0}）", { 0: checkedFileCount }) : tr("识别文件类型") }}</span>
      </button>
      <button
        class="ui-menu-item disk-row-menu-item"
        role="menuitem"
        :disabled="!canPluginTask"
        :title="tr('提交到任务中心，由已启用的 LPP 插件 function 分析该文件；声明了依赖链的 function 会自动接续')"
        @click="$emit('plugin-task')"
      >
        <Puzzle :size="13" />
        <span>{{ tr("插件分析任务…") }}</span>
      </button>
      <div class="ui-menu-sep" role="separator"></div>
      <button class="ui-menu-item disk-row-menu-item" role="menuitem" :disabled="copyCount === 0" @click="$emit('copy-names')">
        <Copy :size="13" />
        <span>{{ copyCount > 1 ? tr("复制名称（{0}）", { 0: copyCount }) : tr("复制名称") }}</span>
      </button>
      <button class="ui-menu-item disk-row-menu-item" role="menuitem" :disabled="copyCount === 0" @click="$emit('copy-paths')">
        <ClipboardCopy :size="13" />
        <span>{{ copyCount > 1 ? tr("复制完整路径（{0}）", { 0: copyCount }) : tr("复制完整路径") }}</span>
      </button>
      <button class="ui-menu-item disk-row-menu-item" role="menuitem" :disabled="copyCount === 0" @click="$emit('copy-forensic')">
        <ClipboardList :size="13" />
        <span>{{ copyCount > 1 ? tr("复制取证信息（{0}）", { 0: copyCount }) : tr("复制取证信息") }}</span>
      </button>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import {
  ChevronRight,
  ClipboardCopy,
  ClipboardList,
  Copy,
  FileDown,
  Fingerprint,
  FolderDown,
  FolderOpen,
  HardDriveDownload,
  HardDriveUpload,
  Info,
  Loader2,
  Puzzle,
  ScanSearch,
  Tag as TagIcon,
  X,
} from 'lucide-vue-next';
import type { HashAlgo } from '../../../lib/ipc/fs/derived';
import { FILE_TAGS, TAG_STYLE, type FileTag } from '../model/tags';

withDefaults(defineProps<{
  visible: boolean;
  x: number;
  y: number;
  hasNode: boolean;
  copyCount: number;
  checkedFileCount: number;
  checkedDirCount: number;
  canExportFile: boolean;
  canExportDir: boolean;
  canHash: boolean;
  canIdentify: boolean;
  canPluginTask: boolean;
  canReturnToOriginalDir: boolean;
  canMount: boolean;
  mountBusy: boolean;
  mounted: boolean;
  mountLabel: string;
  hashMenuOpen: boolean;
  hashMenuFlip: boolean;
  hashOptions: { label: string; algos: HashAlgo[] }[];
}>(), {
  canReturnToOriginalDir: false,
  canPluginTask: false,
});

defineEmits<{
  'tag-pick': [tag: FileTag | null];
  'show-detail': [];
  'return-to-original-dir': [];
  'toggle-mount': [];
  'export-file': [];
  'export-dir': [];
  'update:hashMenuOpen': [open: boolean];
  'compute-hash': [algos: HashAlgo[]];
  'identify-type': [];
  'plugin-task': [];
  'copy-names': [];
  'copy-paths': [];
  'copy-forensic': [];
}>();
</script>

<style scoped>
/* Teleport preserves this component's scope; surface/item visuals use ui-menu. */
.disk-row-menu {
  position: fixed;
  z-index: 9000;
  min-width: 196px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.disk-row-submenu-wrap {
  position: relative;
  display: flex;
  flex-direction: column;
}

.submenu-caret {
  margin-left: auto;
}

.disk-row-submenu {
  position: absolute;
  left: calc(100% - 2px);
  top: -5px;
  z-index: 1;
  min-width: 110px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.disk-row-submenu.flip {
  left: auto;
  right: calc(100% - 2px);
}

.disk-row-menu-tags {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px 5px;
}

.menu-tags-icon {
  color: var(--text-secondary);
  flex-shrink: 0;
}
</style>

<style scoped src="../styles/file-tags.css"></style>
