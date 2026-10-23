<template>
<div class="ui-seg view-switch" role="group" :aria-label="tr('视图')">
  <button class="ui-seg-item" type="button" :aria-pressed="view === 'list'" :title="tr('列表视图')" @click="view = 'list'">
    <List :size="13" aria-hidden="true" /><span class="switch-text">{{ tr("列表") }}</span>
  </button>
  <button
    class="ui-seg-item"
    type="button"
    :aria-pressed="view === 'treemap'"
    :disabled="!treemapAvailable && view !== 'treemap'"
    :title="treemapAvailable || view === 'treemap' ? tr('树图视图：按空间占用查看当前目录') : tr('树图仅支持 NTFS 卷')"
    @click="view = 'treemap'"
  >
    <LayoutGrid :size="13" aria-hidden="true" /><span class="switch-text">{{ tr("树图") }}</span>
  </button>
</div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { LayoutGrid, List } from 'lucide-vue-next';
import type { FileViewMode } from '../model/treemap';

defineProps<{ treemapAvailable: boolean }>();
const view = defineModel<FileViewMode>({ required: true });
</script>

<style scoped>
.view-switch .ui-seg-item {
  gap: 6px;
}
</style>
