<template>
  <dl class="file-detail-fields">
    <div v-for="(field, index) in fields" :key="field.key ?? field.label" class="field-row">
      <dt :title="field.key || field.label">{{ field.label }}</dt>
      <dd :class="{ mono: field.mono, warning: field.warning }">
        <span class="field-value">{{ displayValue(field.value) }}</span>
        <button v-if="field.copyable && hasValue(field.value)" class="ui-icon-btn sm" type="button"
          :title="copiedIndex === index ? tr('已复制') : tr('复制{0}', { 0: field.label })" :aria-label="tr('复制{0}', { 0: field.label })"
          @click="copyValue(field, index)">
          <Check v-if="copiedIndex === index" :size="13" /><Copy v-else :size="13" />
        </button>
      </dd>
    </div>
  </dl>
  <span class="sr-only" role="status">{{ copiedIndex === null ? '' : tr("已复制") }}</span>
</template>

<script lang="ts">
export interface FileDetailField {
  key?: string;
  label: string;
  value: string | number | boolean | null | undefined;
  mono?: boolean;
  copyable?: boolean;
  warning?: boolean;
}
</script>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { onBeforeUnmount, ref, watch } from 'vue';
import { Check, Copy } from 'lucide-vue-next';
import { notifyError } from '../../composables/useToast';

const props = defineProps<{ fields: FileDetailField[] }>();
const copiedIndex = ref<number | null>(null);
let resetTimer: ReturnType<typeof setTimeout> | undefined;
let copyRequest = 0;
function hasValue(value: FileDetailField['value']) { return value !== null && value !== undefined && value !== ''; }
function displayValue(value: FileDetailField['value']) { return hasValue(value) ? String(value) : '—'; }
function resetCopy() { copyRequest++; clearTimeout(resetTimer); copiedIndex.value = null; }
watch(() => props.fields, resetCopy);
onBeforeUnmount(resetCopy);
async function copyValue(field: FileDetailField, index: number) {
  const request = ++copyRequest;
  try {
    await navigator.clipboard.writeText(String(field.value));
    if (request !== copyRequest) return;
    clearTimeout(resetTimer);
    copiedIndex.value = index;
    resetTimer = setTimeout(() => { copiedIndex.value = null; }, 1500);
  } catch (error) {
    if (request === copyRequest) notifyError(tr("复制失败"), String(error));
  }
}
</script>

<style scoped>
.file-detail-fields { margin: 0; }
.field-row { display: grid; grid-template-columns: 76px minmax(0, 1fr); gap: 12px; padding: 6px 0; font-size: 12px; line-height: 1.7; }
.field-row dt { color: var(--text-secondary); overflow-wrap: anywhere; }
.field-row dd { display: flex; align-items: flex-start; gap: 6px; min-width: 0; margin: 0; color: var(--text-primary); }
.field-value { flex: 1; min-width: 0; overflow-wrap: anywhere; white-space: pre-wrap; user-select: text; }
.field-row dd.mono { font: 12px/1.8 var(--font-mono); }
.field-row dd.warning { color: var(--sev-warn-fg); }
</style>
