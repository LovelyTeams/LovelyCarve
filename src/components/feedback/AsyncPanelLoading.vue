<template>
  <div class="async-panel-loading">
    <Loader2 :size="18" class="spin" />
    <span>{{ label }}</span>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { Loader2 } from 'lucide-vue-next';

withDefaults(defineProps<{
  label?: string;
}>(), {
  get label() { return tr("正在加载面板..."); },
});
</script>

<style scoped>
.async-panel-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  height: 100%;
  min-height: 220px;
  color: var(--text-secondary);
  font-size: 13px;
}

.async-panel-loading .spin {
  color: var(--primary-color);
}
</style>