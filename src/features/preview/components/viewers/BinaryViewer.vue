<template>
  <div class="binary-viewer">
    <div v-if="loading" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("正在解析二进制结构…") }}</div>
    <div v-else-if="error" class="state-row mono error">{{ error }}</div>
    <PeViewer v-else-if="peReport" :report="peReport" />
    <ElfViewer v-else-if="elfReport" :report="elfReport" />
    <MachoViewer v-else-if="machoReport" :report="machoReport" />
    <ArchiveViewer v-else-if="archiveReport" :report="archiveReport" />
    <div v-else class="state-row mono">{{ tr("无法识别的二进制格式。") }}</div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// 二进制「原类型视图」分发器：调用 fs_binary_preview 拿带 `kind` 标签的报告，
// 按格式渲染 ELF / Mach-O / PE / ar 子视图。loading / error 在此统一处理，子视图
// 只接收已解析的报告（prop 驱动）。
import { computed, defineAsyncComponent, ref, watch } from 'vue';
import { Loader2 } from 'lucide-vue-next';
import { fsBinaryPreview, type NativeReport } from '../../../../lib/ipc/binary';

const PeViewer = defineAsyncComponent(() => import('./PeViewer.vue'));
const ElfViewer = defineAsyncComponent(() => import('./ElfViewer.vue'));
const MachoViewer = defineAsyncComponent(() => import('./MachoViewer.vue'));
const ArchiveViewer = defineAsyncComponent(() => import('./ArchiveViewer.vue'));

const props = defineProps<{
  imageId: string;
  nodeId: string;
}>();

const loading = ref(false);
const error = ref<string | null>(null);
const report = ref<NativeReport | null>(null);

// 按 kind 收窄，子视图各拿到精确类型（联合成员含全部对应字段，可直接传入）。
const peReport = computed(() => (report.value?.kind === 'pe' ? report.value : null));
const elfReport = computed(() => (report.value?.kind === 'elf' ? report.value : null));
const machoReport = computed(() => (report.value?.kind === 'macho' ? report.value : null));
const archiveReport = computed(() => (report.value?.kind === 'archive' ? report.value : null));

async function load() {
  loading.value = true;
  error.value = null;
  report.value = null;
  try {
    report.value = await fsBinaryPreview({ imageId: props.imageId, nodeId: props.nodeId });
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

watch(() => [props.imageId, props.nodeId], load, { immediate: true });
</script>

<style scoped>
.binary-viewer {
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
/* 子视图自身滚动；分发器只在 loading / error 时显示状态行。 */
.binary-viewer > :not(.state-row) {
  flex: 1;
  min-height: 0;
}
.state-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  padding: 14px 16px;
}
.state-row.error {
  color: var(--sev-critical-fg);
}
</style>
