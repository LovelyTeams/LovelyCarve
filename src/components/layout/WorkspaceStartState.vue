<script setup lang="ts">
import type { Component } from 'vue';
import { Loader2 } from 'lucide-vue-next';

defineProps<{
  icon: Component;
  title: string;
  /** Only an error or a real value; the title and action carry the rest. */
  description?: string;
  actionLabel?: string;
  actionIcon?: Component;
  loading?: boolean;
  disabled?: boolean;
}>();
const emit = defineEmits<{ action: [] }>();
</script>

<template>
  <div class="workspace-start" :aria-busy="loading || undefined">
    <div class="workspace-start-toolbar"><slot name="toolbar" /></div>
    <div class="ui-empty workspace-start-content">
      <span class="ui-empty-icon accent workspace-start-icon">
        <Loader2 v-if="loading" :size="30" class="spin" />
        <component :is="icon" v-else :size="30" />
      </span>
      <p class="ui-empty-title">{{ title }}</p>
      <p v-if="description" class="ui-empty-desc">{{ description }}</p>
      <div v-if="actionLabel || $slots.actions" class="ui-empty-actions">
        <button v-if="actionLabel" type="button" class="ui-btn primary" :disabled="loading || disabled" @click="emit('action')">
          <component :is="actionIcon" v-if="actionIcon" :size="14" />{{ actionLabel }}
        </button>
        <slot name="actions" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.workspace-start { display: flex; flex: 1; flex-direction: column; min-width: 0; min-height: 0; overflow: auto; background: var(--surface-secondary); }
.workspace-start-toolbar { display: flex; flex-shrink: 0; align-items: center; min-height: 30px; margin: 12px 12px 0; }
.workspace-start-content { flex: 1 0 auto; padding: 36px 20px 78px; }
.workspace-start-icon { flex-shrink: 0; }
</style>
