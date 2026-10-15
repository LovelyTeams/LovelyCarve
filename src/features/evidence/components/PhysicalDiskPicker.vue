<template>
  <Transition name="ui-modal">
    <div
      v-if="visible"
      class="ui-modal-overlay disk-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="physical-disk-title"
      @click.self="emit('close')"
    >
      <div class="ui-modal disk-modal">
        <header class="ui-modal-header">
          <span class="ui-modal-icon"><HardDrive :size="18" /></span>
          <div class="ui-modal-heading">
            <h2 id="physical-disk-title" class="ui-modal-title">{{ tr("打开物理磁盘") }}</h2>
            <p class="ui-modal-subtitle">{{ tr("选择要作为证据加载的本机物理磁盘") }}</p>
          </div>
          <button class="ui-icon-btn" :title="tr('刷新')" :aria-label="tr('刷新磁盘列表')" :disabled="loading" @click="refresh">
            <RotateCw :size="16" :class="{ spin: loading }" />
          </button>
          <button class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div class="ui-modal-body">
          <!-- 加载中 -->
          <div v-if="loading" class="ui-empty disk-state">
            <span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span>
            <p class="ui-empty-desc">{{ tr("正在枚举物理磁盘…") }}</p>
          </div>

          <!-- 失败 -->
          <div v-else-if="error" class="ui-alert danger">
            <AlertTriangle :size="16" />
            <span>{{ error }}</span>
          </div>

          <!-- 空列表：多半是没有管理员权限 -->
          <div v-else-if="disks.length === 0" class="ui-empty disk-state">
            <span class="ui-empty-icon"><ShieldAlert :size="24" /></span>
            <p class="ui-empty-title">{{ tr("未发现可访问的物理磁盘") }}</p>
            <p class="ui-empty-desc">{{ tr("打开物理磁盘需") }}<strong>{{ tr("以管理员身份运行") }}</strong>{{ tr("本程序，请提权后重试。") }}</p>
          </div>

          <!-- 磁盘列表 -->
          <div v-else class="disk-list">
            <button
              v-for="disk in disks"
              :key="disk.path"
              class="ui-choice"
              @click="choose(disk)"
            >
              <span class="ui-choice-icon"><HardDrive :size="20" /></span>
              <span class="ui-choice-text">
                <span class="disk-row-title">
                  <span class="ui-choice-title">{{ disk.display }}</span>
                  <span v-if="disk.is_removable" class="ui-badge sm info">{{ tr("可移动") }}</span>
                </span>
                <span class="ui-choice-desc disk-row-meta">
                  <span>{{ formatBytes(disk.size_bytes) }}</span>
                  <span v-if="disk.model" class="disk-model">{{ disk.model }}</span>
                </span>
                <span class="disk-path mono">{{ disk.path }}</span>
              </span>
              <ChevronRight :size="18" class="ui-choice-arrow" />
            </button>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { onUnmounted, ref, watch } from 'vue';
import {
  AlertTriangle,
  ChevronRight,
  HardDrive,
  Loader2,
  RotateCw,
  ShieldAlert,
  X,
} from 'lucide-vue-next';
import { physicalDisksList, type PhysicalDiskInfo } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';

const props = defineProps<{ visible: boolean }>();
const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'select', path: string): void;
}>();

const disks = ref<PhysicalDiskInfo[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

async function refresh() {
  if (loading.value) return;
  loading.value = true;
  error.value = null;
  try {
    disks.value = await physicalDisksList();
  } catch (e) {
    error.value = tr("枚举物理磁盘失败：{0}", { 0: e instanceof Error ? e.message : String(e) });
    disks.value = [];
  } finally {
    loading.value = false;
  }
}

function choose(disk: PhysicalDiskInfo) {
  emit('select', disk.path);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

// 打开时拉取一次磁盘列表并挂上 Escape 关闭；关闭时清理监听。镜像
// AddImageDialog.vue 的轻量「按需挂载」写法，避免全局监听泄漏。
watch(
  () => props.visible,
  visible => {
    if (visible) {
      document.addEventListener('keydown', onKeydown);
      void refresh();
    } else {
      document.removeEventListener('keydown', onKeydown);
    }
  },
);
onUnmounted(() => document.removeEventListener('keydown', onKeydown));
</script>

<style scoped>
.disk-overlay {
  z-index: 9999;
}

.disk-modal {
  width: min(540px, 100%);
  max-height: 80vh;
}

.disk-state strong {
  color: var(--text-primary);
}

.disk-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.disk-row-title {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.disk-row-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.disk-model {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.disk-path {
  font-size: 11px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
