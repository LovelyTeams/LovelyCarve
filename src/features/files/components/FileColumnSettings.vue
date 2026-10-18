<template>
  <Transition name="settings-slide">
    <aside v-if="open" class="col-settings">
      <div class="settings-panel">
        <div class="settings-search">
          <input v-model="filter" class="ui-input sm" type="text" :placeholder="tr('过滤列名…')" :aria-label="tr('过滤显示列')" />
        </div>
        <div class="settings-list">
          <label v-for="column in visibleSettingsList" :key="column.key" class="settings-item"
            :class="{ unready: column.ready === false }"
            :title="column.ready === false ? tr('该列的数据来源暂不可用，可用后自动开放') : undefined">
            <input type="checkbox" :checked="columnVisible[column.key]" :disabled="column.ready === false"
              @change="emit('toggle', column.key)" />
            <span class="settings-label">{{ tr(column.label) }}</span>
            <span v-if="column.ready === false" class="ui-badge sm info">{{ tr("暂不可用") }}</span>
          </label>
        </div>
      </div>
    </aside>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref } from 'vue';
import type { ColumnDef } from '../model/model';

const props = defineProps<{ open: boolean; columns: ColumnDef[]; columnVisible: Record<string, boolean> }>();
const emit = defineEmits<{ toggle: [key: string] }>();
const filter = ref('');
const visibleSettingsList = computed(() => {
  const keyword = filter.value.trim().toLowerCase();
  return keyword ? props.columns.filter(column => tr(column.label).toLowerCase().includes(keyword) || column.key.includes(keyword)) : props.columns;
});
</script>

<style scoped>
/* ── Settings rail ── */
/* Floats over the right of the table so opening/closing never resizes the
 *  table (no layout shift / blank gap) — it just slides in from the edge. */
.col-settings {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  z-index: 5;
  width: 240px;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  border-left: 1px solid var(--border-color);
  background: var(--surface-secondary);
  box-shadow: var(--card-shadow-hover);
}

/* Slide the settings rail in from / out to the right edge. */
.settings-slide-enter-active,
.settings-slide-leave-active {
  transition: transform var(--transition-normal), opacity var(--transition-normal);
}

.settings-slide-enter-from,
.settings-slide-leave-to {
  transform: translateX(100%);
  opacity: 0;
}

.settings-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.settings-search {
  padding: 10px;
  border-bottom: 1px solid var(--border-color);
}

.settings-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px;
}

.settings-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 30px;
  padding: 0 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 12px;
  color: var(--text-primary);
  transition: background var(--transition-fast);
}

.settings-item:hover {
  background: var(--hover-bg);
}

.settings-item input { margin: 0; }

.settings-label {
  flex: 1;
  user-select: none;
}

.settings-item.unready {
  opacity: 0.55;
  cursor: not-allowed;
}
</style>
