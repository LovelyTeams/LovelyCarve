<template>
  <Transition name="ui-modal">
    <div
      v-if="visible"
      class="ui-modal-overlay add-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="add-image-title"
      @click.self="emit('close')"
    >
      <div class="ui-modal add-modal">
        <header class="ui-modal-header">
          <span class="ui-modal-icon"><Plus :size="18" /></span>
          <div class="ui-modal-heading">
            <h2 id="add-image-title" class="ui-modal-title">{{ tr("添加检材") }}</h2>
          </div>
          <button class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div class="ui-modal-body add-options">
          <button class="ui-choice" @click="choose('image')">
            <span class="ui-choice-icon"><HardDrive :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("镜像文件") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('folder')">
            <span class="ui-choice-icon"><FolderOpen :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("文件夹") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('bcp')">
            <span class="ui-choice-icon"><Database :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("BCP 文件与 ZIP 包") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('raid')">
            <span class="ui-choice-icon"><Server :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("RAID 磁盘") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('physical_disk')">
            <span class="ui-choice-icon"><HardDriveDownload :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("物理磁盘") }}</span>
            </span>
            <span class="ui-badge warn">{{ tr("需提权") }}</span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { type AddImageKind } from '../model/types';

import { onUnmounted, watch } from 'vue';
import { ChevronRight, Database, FolderOpen, HardDrive, HardDriveDownload, Plus, Server, X } from 'lucide-vue-next';

const props = defineProps<{ visible: boolean }>();
const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'select', kind: AddImageKind): void;
}>();

function choose(kind: AddImageKind) {
  emit('select', kind);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

// Escape-to-close only while the dialog is open — mirror the lightweight
// click-outside (`@click.self`) close above without a global listener leak.
watch(
  () => props.visible,
  visible => {
    if (visible) document.addEventListener('keydown', onKeydown);
    else document.removeEventListener('keydown', onKeydown);
  },
);
onUnmounted(() => document.removeEventListener('keydown', onKeydown));
</script>

<style scoped>
.add-overlay {
  z-index: 9999;
}

.add-modal {
  width: min(480px, 100%);
}

.add-options {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
</style>
