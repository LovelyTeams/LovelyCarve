<template>
  <article class="raid-vg">
    <div class="array-banner vg-banner">
      <span class="status-led led-info" aria-hidden="true"></span>
      <span class="level-pill vg-pill mono">LVM VG</span>
      <span class="array-name" :title="vg.vg_uuid">{{ vg.vg_name }}</span>
      <span class="vg-meta mono">{{ vg.members.length }} PV · {{ vg.logical_volumes.length }} LV</span>
    </div>
    <details v-disclosure-motion class="array-details"><summary><ChevronRight :size="13" />{{ tr("逻辑卷详情") }}</summary><p class="vg-detail-text">{{ vgSummary(vg) }}</p></details>
  </article>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ChevronRight } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { RaidVolumeGroup } from '../../../lib/ipc/raid';
import { vgSummary } from '../model/stackView';
defineProps<{ vg: RaidVolumeGroup }>();
</script>
<style scoped src="../styles/raid-summary.css"></style>
<style scoped>
/* ── LVM VG slim banner variant ── */
.raid-vg { gap: 0; }
.vg-banner { background: var(--surface-tertiary); }
.vg-detail-text { font-size: 12px; line-height: 1.7; color: var(--text-secondary); overflow-wrap: anywhere; margin: 0 0 16px; }

</style>
