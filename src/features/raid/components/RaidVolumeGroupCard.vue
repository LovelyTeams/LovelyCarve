<template>
  <article class="raid-vg">
    <div class="array-banner vg-banner">
      <span class="status-led led-info" aria-hidden="true"></span>
      <span class="level-pill vg-pill mono">LVM VG</span>
      <span class="array-name" :title="vg.vg_uuid">{{ vg.vg_name }}</span>
      <span class="vg-meta mono">{{ vg.members.length }} PV · {{ vg.logical_volumes.length }} LV</span>
    </div>
    <ul v-if="assembledVolumes.length" class="spanning-lvs">
      <li v-for="lv in assembledVolumes" :key="lv.name" class="spanning-lv">
        <span class="lv-name mono">{{ lv.name }}</span>
        <span class="lv-size mono">{{ formatBytes(lv.size_bytes) }}</span>
        <span v-if="physicalVolumeCount(lv) > 1" class="ui-badge sm outline">{{ tr("跨 {0} 个物理卷", { 0: physicalVolumeCount(lv) }) }}</span>
        <button v-if="lv.assembled_image_id !== imageId" class="ui-btn sm" :disabled="openingId === lv.assembled_image_id"
          :aria-busy="openingId === lv.assembled_image_id" :title="tr('打开逻辑卷')" @click="emit('open', lv.assembled_image_id)">
          <Loader2 v-if="openingId === lv.assembled_image_id" :size="13" class="spin" />
          <ExternalLink v-else :size="13" />{{ tr("打开逻辑卷") }}
        </button>
      </li>
    </ul>
    <details v-disclosure-motion class="array-details"><summary><ChevronRight :size="13" />{{ tr("逻辑卷详情") }}</summary><p class="vg-detail-text">{{ vgSummary(vg, tr) }}</p></details>
  </article>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { ChevronRight, ExternalLink, Loader2 } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { RaidLogicalVolume, RaidVolumeGroup } from '../../../lib/ipc/raid';
import { formatBytes, vgSummary } from '../model/stackView';
const props = defineProps<{ vg: RaidVolumeGroup; imageId?: string; openingId?: string | null }>();
const emit = defineEmits<{ open: [imageId: string | null] }>();
/** Complete logical volumes can be opened as images of their own. */
const assembledVolumes = computed(() => props.vg.logical_volumes.filter(lv => lv.assembled_image_id));
function physicalVolumeCount(lv: RaidLogicalVolume): number {
  return new Set(lv.segments.map(segment => `${segment.image_id}:${segment.partition_id}`)).size;
}
</script>
<style scoped src="../styles/raid-summary.css"></style>
<style scoped>
/* ── LVM VG slim banner variant ── */
.raid-vg { gap: 0; }
.vg-banner { background: var(--surface-tertiary); }
.vg-detail-text { font-size: 12px; line-height: 1.7; color: var(--text-secondary); overflow-wrap: anywhere; margin: 0 0 16px; }
.spanning-lvs { list-style: none; margin: 8px 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
.spanning-lv { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; min-height: 28px; }
.lv-name { font-size: 12px; color: var(--text-primary); }
.lv-size { font-size: 11px; color: var(--text-secondary); }
.spanning-lv .ui-btn { margin-left: auto; }
</style>
