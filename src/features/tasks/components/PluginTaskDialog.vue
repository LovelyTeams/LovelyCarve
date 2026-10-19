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
              {{ tr("为 {0} 选择一个文件分析器。任务由调度器派发，可在「任务中心」跟踪进度、取消或重新分析。", { 0: fileName }) }}
            </p>
          </div>
          <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div v-if="chosen" class="ui-modal-body task-body">
          <button class="ui-btn link sm back" type="button" :disabled="submitting !== null" @click="chosen = null"><ChevronLeft :size="14" />{{ tr("返回分析器列表") }}</button>
          <h3 class="chosen-title">{{ functionName(chosen) }}</h3>
          <AnalyzerOptionsForm v-model="options" :fn="chosen" :disabled="submitting !== null" />
        </div>
        <div v-else class="ui-modal-body task-body">
          <label v-if="internalCount" class="ui-check toggle">
            <input v-model="showInternal" type="checkbox" />
            {{ tr("显示内部分析器") }}
          </label>

          <div v-if="loading && candidates.length === 0" class="ui-empty state-box">
            <span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span>
            <p class="ui-empty-desc">{{ tr("正在读取分析器…") }}</p>
          </div>
          <div v-else-if="candidates.length === 0" class="ui-empty state-box">
            <span class="ui-empty-icon"><CircleAlert :size="24" /></span>
            <p class="ui-empty-title">{{ tr("没有可用的文件分析器") }}</p>
            <p class="ui-empty-desc">{{ tr("安装提供文件分析的插件包后，会显示在这里。") }}</p>
          </div>

          <div v-else class="function-options">
            <button
              v-for="fn in candidates"
              :key="fn.function"
              type="button"
              class="ui-choice option"
              :disabled="submitting !== null"
              :title="fn.function"
              @click="choose(fn)"
            >
              <span class="ui-choice-text">
                <span class="ui-choice-title">{{ functionName(fn) }}</span>
                <span class="option-desc mono">{{ fn.function }} · v{{ fn.pluginVersion }}</span>
                <span class="option-chips">
                  <span v-if="matchesFile(fn)" class="ui-badge sm success">{{ tr("匹配此文件") }}</span>
                  <span v-if="fn.options.length" class="ui-badge sm">{{ tr("{0} 个选项", { 0: fn.options.length }) }}</span>
                  <span v-if="fn.internal" class="ui-badge sm warn">{{ tr("内部") }}</span>
                  <span v-if="fn.extensions.length > 0" class="ui-badge sm mono info">{{ fn.extensions.join(' ') }}</span>
                </span>
              </span>
              <Loader2 v-if="submitting === fn.function" :size="16" class="spin ui-choice-arrow" />
              <ChevronRight v-else :size="18" class="ui-choice-arrow" />
            </button>
          </div>
        </div>
        <footer v-if="chosen" class="ui-modal-footer">
          <button class="ui-btn ghost" type="button" :disabled="submitting !== null" @click="emit('close')">{{ tr("取消") }}</button>
          <button class="ui-btn primary" type="button" :disabled="submitting !== null" @click="submit(chosen)"><Loader2 v-if="submitting" :size="13" class="spin" /><Play v-else :size="13" />{{ tr("开始分析") }}</button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

/** File analysis: the file-scope analyzers of every package; those declaring
 *  the file's extension come first. An analyzer with options asks for them
 *  before the task is created. */
import { computed, ref, shallowRef, watch } from 'vue';
import { ChevronLeft, ChevronRight, CircleAlert, Loader2, Play, Puzzle, X } from 'lucide-vue-next';
import type { SchedFunction } from '../../../lib/ipc/tasks';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { useTaskCenter } from '../composables/useTaskCenter';
import { usePluginCatalog } from '../composables/usePluginCatalog';
import AnalyzerOptionsForm from './AnalyzerOptionsForm.vue';

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
const catalog = usePluginCatalog();
const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.visible, dialogEl);
const showInternal = ref(false);
const submitting = ref<string | null>(null);
const chosen = shallowRef<SchedFunction | null>(null);
const options = ref<Record<string, unknown>>({});

watch(
  () => props.visible,
  visible => {
    if (visible) {
      submitting.value = null;
      chosen.value = null;
      void refresh();
    }
  },
);

const extension = computed(() => {
  const dot = props.fileName.lastIndexOf('.');
  return dot > 0 ? props.fileName.slice(dot + 1).toLowerCase() : '';
});

function matchesFile(fn: SchedFunction): boolean {
  return !!extension.value && fn.extensions.some(item => item.replace(/^\./, '').toLowerCase() === extension.value);
}

const fileFunctions = computed(() => functions.value.filter(fn => fn.scope === 'files'));
const internalCount = computed(() => fileFunctions.value.filter(fn => fn.internal).length);
const candidates = computed(() =>
  fileFunctions.value
    .filter(fn => showInternal.value || !fn.internal)
    .sort((left, right) => Number(matchesFile(right)) - Number(matchesFile(left))),
);

function functionName(fn: SchedFunction): string {
  return catalog.text(fn.pluginId, pluginTextKey.analyzer(fn.analyzer), fn.displayName);
}

function choose(fn: SchedFunction) {
  if (fn.options.length) {
    options.value = {};
    chosen.value = fn;
  } else {
    void submit(fn);
  }
}

async function submit(fn: SchedFunction) {
  if (!props.nodeId || submitting.value) return;
  submitting.value = fn.function;
  try {
    const task = await create({
      function: fn.function,
      imageId: props.imageId,
      nodeId: props.nodeId,
      fileName: props.fileName,
      options: Object.keys(options.value).length && chosen.value === fn ? options.value : null,
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

.back {
  align-self: flex-start;
}

.chosen-title {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
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
