<template>
  <div class="ui-empty evidence-load" role="status" aria-live="polite">
    <Loader2 v-if="running" :size="32" class="spin" />
    <FileArchive v-else :size="32" />
    <p class="ui-empty-title">{{ name }}</p>
    <p class="ui-empty-desc">{{ task?.error || task?.message || tr('等待加载') }}</p>
    <div v-if="running" class="ui-progress" :class="{ indeterminate: task?.progress == null }"
      role="progressbar" :aria-label="tr('证据加载')" :aria-valuenow="task?.progress ?? undefined" aria-valuemin="0" aria-valuemax="100">
      <span :style="task?.progress != null ? { width: `${task.progress}%` } : undefined" />
    </div>
    <p v-if="running" class="ui-empty-desc">{{ tr('解压完成后自动加载内容，可切换标签页继续操作。') }}</p>
    <div class="ui-empty-actions">
      <button v-if="running" class="ui-btn" :disabled="task?.cancelling" @click="cancelTask">{{ task?.cancelling ? tr('正在取消，请稍候…') : tr('取消') }}</button>
      <button class="ui-btn" @click="emit('tasks')">{{ tr('任务中心') }}</button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { FileArchive, Loader2 } from 'lucide-vue-next';
import { useBuiltinTasks } from '../../features/tasks/api';
import { tr } from '../../composables/useLocale';
import { notifyError } from '../../composables/useToast';
const props = defineProps<{ taskId: string; path: string }>();
const emit = defineEmits<{ tasks: [] }>();
const builtin = useBuiltinTasks();
const task = computed(() => builtin.tasks.value.find(task => task.id === `imageLoad:${props.taskId}`));
const name = computed(() => props.path.split(/[\\/]/).pop() || props.path);
const running = computed(() => !task.value || ['queued', 'running'].includes(task.value.status));
async function cancelTask() {
  try { await builtin.cancel(`imageLoad:${props.taskId}`); }
  catch (error) { notifyError(tr('取消任务失败'), String(error)); }
}
</script>

<style scoped>
.evidence-load { height: 100%; padding: 32px; }
.evidence-load .ui-progress { width: min(400px, 100%); }
.evidence-load .ui-empty-desc { overflow-wrap: anywhere; }
</style>
