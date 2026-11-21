<template>
  <!-- Inside a preview the content moves into its view bar or status bar; a
       viewer rendered on its own (tests, other hosts) keeps it in place. -->
  <Teleport v-if="target" :to="target"><slot /></Teleport>
  <slot v-else-if="!chrome && active" />
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { usePreviewChrome, type PreviewChromeArea } from '../composables/previewChrome';

const props = withDefaults(defineProps<{
  area: PreviewChromeArea;
  /** False while the owning view is hidden (a sub-view behind v-show). */
  active?: boolean;
}>(), { active: true });

const chrome = usePreviewChrome();
const target = computed(() => (props.active ? chrome?.[props.area].value ?? null : null));
</script>
