<template>
  <footer class="preview-footer">
    <span v-if="hasSelection" class="sel-info mono">
      {{ tr("已选") }} {{ selCount }} {{ tr("字节 · 0x") }}{{ selMin.toString(16).toUpperCase() }}–0x{{ selMax.toString(16).toUpperCase() }}
    </span>
    <div v-if="totalPages > 1" class="footer-pager">
      <span class="footer-label">{{ tr("分页大小：") }}</span>
      <UiSelect v-model="pageSize" class="sm mono page-size" :options="PAGE_SIZE_OPTIONS" :aria-label="tr('分页大小')" />
      <span class="footer-label">{{ tr("当前页：") }}</span>
      <input class="ui-input sm mono page-input" type="number" min="1" :max="totalPages" v-model.number="page" />
      <span class="footer-divider">/ {{ totalPages }}</span>
      <button class="ui-icon-btn sm bordered" :aria-label="tr('上一页')" :disabled="page <= 1" @click="page -= 1"><ChevronLeft :size="14" /></button>
      <span class="ui-count accent page-current">{{ page }}</span>
      <button class="ui-icon-btn sm bordered" :aria-label="tr('下一页')" :disabled="page >= totalPages" @click="page += 1"><ChevronRight :size="14" /></button>
    </div>
  </footer>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ChevronLeft, ChevronRight } from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import { PREVIEW_PAGE_SIZES as PAGE_SIZE_OPTIONS } from '../model/preview';

defineProps<{ totalPages: number; hasSelection: boolean; selCount: number; selMin: number; selMax: number }>();
const page = defineModel<number>('page', { required: true });
const pageSize = defineModel<number>('pageSize', { required: true });
</script>

<style scoped>
.mono { font-family: var(--font-mono); font-size: 12px; }
.preview-footer {
  display: flex;
  flex-wrap: wrap;
  flex-shrink: 0;
  align-items: center;
  gap: 8px;
  min-height: 40px;
  padding: 6px 12px 6px 16px;
  border-top: 1px solid var(--border-color);
  background: var(--surface-primary);
  font-size: 11px;
  color: var(--text-secondary);
}

.footer-label {
  font-size: 12px;
  white-space: nowrap;
}

/* 选区读数靠左，分页控件成组靠右。 */
.sel-info {
  color: var(--text-secondary);
  overflow-wrap: anywhere;
}

.footer-pager {
  margin-left: auto;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.page-size {
  width: auto;
}

.page-input {
  width: 64px;
}

.footer-divider {
  font-family: var(--font-mono);
  color: var(--text-secondary);
}

.page-current {
  min-width: 26px;
  height: 22px;
}
</style>
