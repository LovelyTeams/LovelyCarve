<template>
  <section class="about-panel" :aria-label="tr('软件信息')">
    <div class="about-name"><BrandMark :size="24" /><strong>LovelyCarve</strong></div>
    <dl class="about-details">
      <div><dt>{{ tr("软件版本") }}</dt><dd class="mono">v{{ version }}</dd></div>
      <div><dt>{{ tr("构建时间（本地）") }}</dt><dd>{{ buildTime }}</dd></div>
    </dl>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import BrandMark from '../../../components/display/BrandMark.vue';
import { useAppVersion } from '../../../composables/useAppVersion';
import { formatEpochSeconds } from '../../../utils/format';

const { version, buildDate } = useAppVersion();
const buildTime = computed(() => {
  const timestamp = Date.parse(buildDate);
  return Number.isFinite(timestamp) ? formatEpochSeconds(timestamp / 1000, { precision: 'seconds' }) : tr("未知");
});
</script>

<style scoped>
.about-panel { display: flex; flex-direction: column; gap: 24px; }
.about-name { display: flex; align-items: center; gap: 8px; font-size: 15px; }
.about-details { display: flex; flex-direction: column; gap: 16px; margin: 0; font-size: 12px; }
.about-details > div { display: flex; flex-wrap: wrap; gap: 8px 16px; }
.about-details dt { min-width: 112px; color: var(--text-secondary); }
.about-details dd { margin: 0; color: var(--text-primary); }
</style>
