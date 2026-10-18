<template>
  <Transition name="ui-modal">
    <div
      v-if="visible"
      class="ui-modal-overlay task-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="plugin-task-title"
      @click.self="emit('close')"
    >
      <div ref="dialogEl" class="ui-modal task-modal" tabindex="-1" @keydown.esc.stop="emit('close')">
        <header class="ui-modal-header task-head">
          <span class="ui-modal-icon"><Puzzle :size="18" /></span>
          <div class="ui-modal-heading">
            <h2 id="plugin-task-title" class="ui-modal-title">{{ tr("插件分析任务") }}</h2>
            <p class="ui-modal-subtitle task-subtitle">
              {{ tr("为") }} <span class="mono">{{ fileName }}</span> {{ tr("选择一个 function。任务进入案件任务表后由调度器派发，可在「任务中心」跟踪进度、取消或重启。") }}
            </p>
          </div>
          <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div class="ui-modal-body task-body">
          <label class="ui-check toggle">
            <input v-model="showInternal" type="checkbox" />
            {{ tr("显示内部 function") }}
          </label>

          <div v-if="loading && candidates.length === 0" class="ui-empty state-box">
            <span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span>
            <p class="ui-empty-desc">{{ tr("正在读取 function 注册表…") }}</p>
          </div>
          <div v-else-if="candidates.length === 0" class="ui-empty state-box">
            <span class="ui-empty-icon"><CircleAlert :size="24" /></span>
            <p class="ui-empty-title">{{ tr("没有可用的插件分析能力") }}</p>
            <p class="ui-empty-desc">{{ tr("请将提供文件分析的插件放入应用数据目录的 plugins 文件夹。") }}</p>
          </div>

          <div v-else class="function-options">
            <button
              v-for="fn in candidates"
              :key="fn.function"
              type="button"
              class="ui-choice option"
              :disabled="submitting !== null"
              :title="fn.function"
              @click="submit(fn)"
            >
              <span class="ui-choice-text">
                <span class="ui-choice-title">{{ fn.displayName }}</span>
                <span class="option-desc mono">{{ fn.function }} · v{{ fn.pluginVersion }}</span>
                <span class="option-chips">
                  <span class="ui-badge sm mono">{{ fn.trigger }}</span>
                  <span v-if="fn.dependence" class="ui-badge sm mono">← {{ fn.dependence }}</span>
                  <span v-if="fn.internal" class="ui-badge sm mono warn">internal</span>
                  <span v-if="fn.singleInstance" class="ui-badge sm mono">singleinstance</span>
                  <span v-if="fn.extensions.length > 0" class="ui-badge sm mono info">{{ fn.extensions.join(' ') }}</span>
                </span>
              </span>
              <Loader2 v-if="submitting === fn.function" :size="16" class="spin ui-choice-arrow" />
              <ChevronRight v-else :size="18" class="ui-choice-arrow" />
            </button>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import { ChevronRight, CircleAlert, Loader2, Puzzle, X } from 'lucide-vue-next';
import type { SchedFunction } from '../../../lib/ipc/tasks';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { useTaskCenter } from '../composables/useTaskCenter';

const props = defineProps<{
  visible: boolean;
  imageId: string;
  nodeId: string | null;
  fileName: string;
}>();

const emit = defineEmits<{
  close: [];
  submitted: [taskId: number];
}>();

const { functions, loading, refresh, create } = useTaskCenter();
const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.visible, dialogEl);
const showInternal = ref(false);
const submitting = ref<string | null>(null);

watch(
  () => props.visible,
  visible => {
    if (visible) {
      submitting.value = null;
      void refresh();
    }
  },
);

const candidates = computed(() =>
  functions.value.filter(fn => showInternal.value || !fn.internal),
);

async function submit(fn: SchedFunction) {
  if (!props.nodeId || submitting.value) return;
  submitting.value = fn.function;
  try {
    const task = await create({
      function: fn.function,
      imageId: props.imageId,
      nodeId: props.nodeId,
      fileName: props.fileName,
    });
    if (task) {
      emit('submitted', task.id);
      emit('close');
    }
  } finally {
    submitting.value = null;
  }
}
</script>

<style scoped>
.task-overlay {
  z-index: 9999;
}

.task-modal {
  width: min(540px, 100%);
  max-height: min(80vh, 660px);
}

.task-head {
  align-items: flex-start;
}

.task-subtitle {
  line-height: 1.6;
  word-break: break-all;
}

.task-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.toggle {
  align-self: flex-start;
  color: var(--text-secondary);
}

.state-box {
  border: 1px dashed var(--border-color);
  border-radius: var(--radius-md);
}

.function-options {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.option {
  padding: 12px 14px;
}

.option-desc {
  font-size: 11px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.option-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  margin-top: 3px;
}
</style>
