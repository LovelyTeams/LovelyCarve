<template>
  <!-- 标签列按最长标签取宽，长标签不折行；可复制的值带复制按钮。 -->
  <dl class="kdbx-details">
    <div v-for="row in rows" :key="row.key" class="detail-row">
      <dt :title="row.label">{{ row.label }}</dt>
      <dd :class="{ mono: row.mono, warning: row.warning }">
        <span class="detail-value">{{ display(row.value) }}</span>
        <button v-if="row.copyable && present(row.value)" type="button" class="ui-icon-btn sm"
          :title="copiedKey === row.key ? tr('已复制') : tr('复制{0}', { 0: row.label })" :aria-label="tr('复制{0}', { 0: row.label })"
          @click="copy(row.key, String(row.value))">
          <Check v-if="copiedKey === row.key" :size="13" class="ui-copied" /><Copy v-else :size="13" />
        </button>
      </dd>
    </div>
  </dl>
  <span class="sr-only" role="status">{{ copiedKey === null ? '' : tr("已复制") }}</span>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { watch } from 'vue';
import { Check, Copy } from 'lucide-vue-next';
import { useCopyFeedback } from '../../composables/useCopyFeedback';
import type { KdbxDetail } from '../../model/kdbx';

const props = defineProps<{ rows: KdbxDetail[] }>();
const { copiedKey, copy, reset } = useCopyFeedback();

function present(value: KdbxDetail['value']): boolean {
  return value !== null && value !== '';
}

function display(value: KdbxDetail['value']): string {
  return present(value) ? String(value) : '—';
}

watch(() => props.rows, reset);
</script>

<style scoped>
.kdbx-details {
  display: grid;
  grid-template-columns: minmax(72px, max-content) minmax(0, 1fr);
  column-gap: 16px;
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
}

.detail-row {
  display: contents;
}

.detail-row dt,
.detail-row dd {
  padding: 5px 0;
}

.detail-row dt {
  max-width: 160px;
  overflow: hidden;
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.detail-row dd {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  min-width: 0;
  margin: 0;
  color: var(--text-primary);
}

.detail-row dd.mono {
  font-family: var(--font-mono);
}

.detail-row dd.warning {
  color: var(--sev-warn-fg);
}

.detail-value {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
  user-select: text;
}
</style>
