<template>
  <div class="ui-empty evidence-load" role="status" aria-live="polite">
    <Loader2 v-if="running" :size="32" class="spin" />
    <FileArchive v-else :size="32" />
    <p class="ui-empty-title">{{ name }}</p>
    <p class="ui-empty-desc">{{ entry?.error || message || tr('等待加载') }}</p>
    <div v-if="running" class="ui-progress" :class="{ indeterminate: percent === null }"
      role="progressbar" :aria-label="tr('检材加载')" :aria-valuenow="percent ?? undefined" aria-valuemin="0" aria-valuemax="100">
      <span :style="percent !== null ? { width: `${percent}%` } : undefined" />
    </div>
    <p v-if="running" class="ui-empty-desc">{{ tr('解压完成后自动加载内容，可切换标签页继续操作。') }}</p>
    <div class="ui-empty-actions">
      <button v-if="running" class="ui-btn" :disabled="cancelling" @click="cancelTask">{{ cancelling ? tr('正在取消，请稍候…') : tr('取消') }}</button>
      <button class="ui-btn" @click="emit('tasks')">{{ tr('任务中心') }}</button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';
import { FileArchive, Loader2 } from 'lucide-vue-next';
import { builtinMessage, cancelTaskEntry, useTaskEntries } from '../../features/tasks/api';
import { tr } from '../../composables/useLocale';
import { notifyError } from '../../composables/useToast';
const props = defineProps<{ taskId: string; path: string }>();
const emit = defineEmits<{ tasks: [] }>();
const { entries } = useTaskEntries();
/** The backend lists the load under the task id the tab started it with. */
const entry = computed(() => entries.value.find(item => item.id === props.taskId));
const name = computed(() => props.path.split(/[\\/]/).pop() || props.path);
const running = computed(() => !entry.value || ['queued', 'running'].includes(entry.value.status));
const message = computed(() => entry.value?.source === 'builtin' ? builtinMessage(entry.value, tr) : '');
const percent = computed(() => {
  const amount = entry.value?.progress;
  return amount?.total ? Math.min(100, Math.round(amount.done / amount.total * 100)) : null;
});
const requested = ref(false);
const cancelling = computed(() => requested.value || Boolean(entry.value?.cancelling));
async function cancelTask() {
  requested.value = true;
  try { await cancelTaskEntry(props.taskId); }
  catch (error) {
    requested.value = false;
    notifyError(tr('取消任务失败'), String(error));
  }
}
</script>

<style scoped>
.evidence-load { height: 100%; padding: 32px; }
.evidence-load .ui-progress { width: min(400px, 100%); }
.evidence-load .ui-empty-desc { overflow-wrap: anywhere; }
</style>
