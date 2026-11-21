<template>
  <!-- One shape for every preview state — loading, failure, nothing to show —
       so each viewer answers the same way: what happened and what to do next. -->
  <div class="ui-empty preview-state" :class="tone" :role="tone === 'error' ? 'alert' : 'status'">
    <span class="ui-empty-icon state-icon" :class="{ accent: tone === 'accent' }" aria-hidden="true">
      <Loader2 v-if="tone === 'loading'" :size="22" class="spin" />
      <component :is="icon" v-else :size="22" />
    </span>
    <p v-if="title" class="ui-empty-title state-title">{{ title }}</p>
    <p v-if="description" class="ui-empty-desc state-desc" :class="{ mono: monoDescription }">{{ description }}</p>
    <div v-if="$slots.default" class="ui-empty-actions"><slot /></div>
  </div>
</template>

<script setup lang="ts">
import type { Component } from 'vue';
import { Loader2 } from 'lucide-vue-next';

withDefaults(defineProps<{
  /** `loading` spins; `error` reads as a failure and is announced as an alert. */
  tone?: 'neutral' | 'accent' | 'loading' | 'error';
  icon?: Component;
  title?: string;
  description?: string;
  /** Backend messages and offsets keep their monospace form. */
  monoDescription?: boolean;
}>(), { tone: 'neutral', icon: undefined, title: '', description: '' });
</script>

<style scoped>
.preview-state {
  flex: 1;
  min-height: 0;
  padding: 32px 24px;
}

.state-title {
  font-size: 13px;
}

.state-desc {
  max-width: 420px;
  overflow-wrap: anywhere;
}

.state-desc.mono {
  font-size: 11px;
}

.preview-state.error .state-icon {
  background: var(--sev-critical-bg);
  color: var(--sev-critical-fg);
}

.preview-state.error .state-desc {
  color: var(--sev-critical-fg);
}

.preview-state.loading .state-icon {
  background: transparent;
  color: var(--text-secondary);
}
</style>
