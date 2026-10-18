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
            <p class="ui-modal-subtitle">{{ tr("选择检材来源后会登记到当前案件") }}</p>
          </div>
          <button class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div class="ui-modal-body add-options">
          <button class="ui-choice" @click="choose('image')">
            <span class="ui-choice-icon"><HardDrive :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("镜像文件") }}</span>
              <span class="ui-choice-desc">{{ tr("RAW / E01 / VMDK / VHD(X) / ISO / DMG / LDBK 等磁盘镜像") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('folder')">
            <span class="ui-choice-icon"><FolderOpen :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("文件夹") }}</span>
              <span class="ui-choice-desc">{{ tr("本机目录、已解包数据集或 iTunes / Finder 的 iOS 备份") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('bcp')">
            <span class="ui-choice-icon"><Database :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("BCP 文件") }}</span>
              <span class="ui-choice-desc">{{ tr("打开网安数据交换 BCP，按索引浏览分类与记录") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('raid')">
            <span class="ui-choice-icon"><Layers :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("RAID 磁盘") }}</span>
              <span class="ui-choice-desc">{{ tr("逐块放入成员盘，自动检测阵列；确认后再打开重组镜像") }}</span>
            </span>
            <ChevronRight :size="18" class="ui-choice-arrow" />
          </button>

          <button class="ui-choice" @click="choose('physical_disk')">
            <span class="ui-choice-icon"><HardDriveDownload :size="20" /></span>
            <span class="ui-choice-text">
              <span class="ui-choice-title">{{ tr("物理磁盘") }}</span>
              <span class="ui-choice-desc">{{ tr("直接读取本机块设备；Windows 通常需要管理员权限") }}</span>
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
import { ChevronRight, Database, FolderOpen, HardDrive, HardDriveDownload, Layers, Plus, X } from 'lucide-vue-next';

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
