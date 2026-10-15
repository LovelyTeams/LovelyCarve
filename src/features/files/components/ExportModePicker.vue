<template>
  <Teleport to="body">
    <Transition name="ui-modal">
      <div
        v-if="open"
        class="ui-modal-overlay export-mode-overlay"
        @click.self="$emit('choose', null)"
      >
        <div
          ref="dialogEl"
          class="ui-modal export-mode-modal"
          role="dialog"
          aria-modal="true"
          aria-labelledby="export-mode-title"
          tabindex="-1"
          @keydown.esc.stop="$emit('choose', null)"
        >
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><FolderDown :size="18" /></span>
            <div class="ui-modal-heading">
              <h2 id="export-mode-title" class="ui-modal-title">{{ tr("导出文件夹") }}</h2>
              <p class="ui-modal-subtitle">{{ tr("选择导出后的组织方式") }}</p>
            </div>
            <button class="ui-icon-btn" type="button" :aria-label="tr('取消')" :title="tr('取消')" @click="$emit('choose', null)">
              <X :size="16" />
            </button>
          </header>
          <div class="ui-modal-body export-mode-body">
            <button class="ui-choice" type="button" @click="$emit('choose', 'tree')">
              <span class="ui-choice-icon"><FolderTree :size="20" /></span>
              <span class="ui-choice-text">
                <span class="ui-choice-title">{{ tr("原始目录结构") }}</span>
                <span class="ui-choice-desc">{{ tr("保留嵌套目录，按原样写入磁盘") }}</span>
              </span>
              <ChevronRight :size="18" class="ui-choice-arrow" />
            </button>
            <button class="ui-choice" type="button" @click="$emit('choose', 'zip')">
              <span class="ui-choice-icon"><FileArchive :size="20" /></span>
              <span class="ui-choice-text">
                <span class="ui-choice-title">{{ tr("ZIP 压缩包") }}</span>
                <span class="ui-choice-desc">{{ tr("Deflate 压缩，打包为单个 .zip 文件") }}</span>
              </span>
              <ChevronRight :size="18" class="ui-choice-arrow" />
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ref } from 'vue';
import { ChevronRight, FileArchive, FolderDown, FolderTree, X } from 'lucide-vue-next';
import { useDialogFocus } from '../../../composables/useDialogFocus';

type ExportDirMode = 'tree' | 'zip';

const props = defineProps<{ open: boolean }>();

defineEmits<{
  choose: [mode: ExportDirMode | null];
}>();

const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.open, dialogEl);
</script>

<style scoped>
.export-mode-overlay {
  z-index: 9100;
}

.export-mode-modal {
  width: min(460px, calc(100vw - 32px));
}

.export-mode-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
</style>
