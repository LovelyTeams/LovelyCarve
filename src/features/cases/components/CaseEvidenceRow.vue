<template>
  <article class="evidence-row" :class="{ current }">
    <div class="evidence-main">
      <div class="evidence-icon"><Boxes v-if="raidLevel !== undefined" :size="20" /><HardDrive v-else :size="20" /></div>
      <button class="evidence-name" :disabled="busy" :title="tr('打开 {0}', { 0: evidence.path })" @click="emit('open', evidence)">
        <span class="file-name">{{ fileName }}</span>
        <span class="file-meta">
          <span v-if="current" class="ui-badge sm info">{{ tr("当前检材") }}</span>
          <span v-if="raidLevel !== undefined">RAID {{ raidLevel }}</span>
          <span>{{ evidence.source_format.toUpperCase() }}</span>
          <span v-if="evidence.platform">{{ evidence.platform.toUpperCase() }}</span>
          <span>{{ formatBytes(evidence.size_bytes) }}</span>
        </span>
      </button>
      <div class="row-actions">
        <button class="ui-btn sm ghost" :disabled="busy" @click="emit('open', evidence)">{{ tr("打开") }}</button>
        <button class="ui-icon-btn details-toggle" :aria-label="tr('{0} {1} 的详情', { 0: expanded ? tr('收起') : tr('展开'), 1: fileName })" :aria-expanded="expanded" :aria-controls="detailsId" :title="tr('检材详情')" @click="expanded = !expanded"><ChevronDown :size="16" :class="{ rotated: expanded }" /></button>
      </div>
    </div>
    <UiTransition><div v-if="expanded" :id="detailsId" class="evidence-details">
      <dl>
        <dt>{{ tr("位置") }}</dt><dd class="file-path">{{ evidence.path }}</dd>
        <dt>{{ tr("添加时间") }}</dt><dd>{{ addedAt }}</dd>
      </dl>
      <button class="ui-btn sm danger-soft" :disabled="busy || locked" @click="emit('remove', evidence)"><Trash2 :size="13" /> {{ tr("从案件移除") }}</button>
    </div></UiTransition>
  </article>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import UiTransition from '../../../components/ui/UiTransition.vue';
import { computed, ref, useId } from 'vue';
import { Boxes, ChevronDown, HardDrive, Trash2 } from 'lucide-vue-next';
import type { EvidenceRef } from '../../../lib/ipc/cases';
import { formatBytes, formatEpochSeconds } from '../../../utils/format';

const props = defineProps<{
  evidence: EvidenceRef;
  current: boolean;
  busy: boolean;
  locked: boolean;
  raidLevel?: number;
}>();
const emit = defineEmits<{
  open: [evidence: EvidenceRef];
  remove: [evidence: EvidenceRef];
}>();
const expanded = ref(false);
const detailsId = useId();
const fileName = computed(() => props.evidence.path.split(/[\\/]+/).filter(Boolean).pop() || props.evidence.path);
const addedAt = computed(() => {
  const value = props.evidence.added_at;
  const formatted = formatEpochSeconds(Date.parse(value) / 1000, { precision: 'seconds', fallback: '', zeroAsMissing: false });
  return formatted ? tr("{0}（本地）", { 0: formatted }) : tr("时间未知");
});
</script>

<style scoped>
.evidence-row { border: 1px solid var(--border-color); border-radius: var(--radius-md); background: var(--surface-secondary); overflow: hidden; transition: border-color var(--transition-fast), box-shadow var(--transition-fast); }
.evidence-row:hover { border-color: color-mix(in srgb, var(--primary-color) 35%, var(--border-color)); }
.evidence-row.current { border-color: var(--sev-info-border); box-shadow: 0 0 0 1px color-mix(in srgb, var(--primary-color) 20%, transparent); }
.evidence-main { display: flex; align-items: center; gap: 12px; padding: 12px 12px 12px 14px; }
.evidence-icon { display: grid; place-items: center; width: 36px; height: 36px; flex-shrink: 0; border-radius: var(--radius-md); background: var(--sev-info-bg); color: var(--primary-color); }
.evidence-name { flex: 1; min-width: 0; padding: 0; border: 0; background: transparent; color: var(--text-primary); text-align: left; cursor: pointer; }
.evidence-name:disabled { cursor: not-allowed; opacity: 0.5; }
.evidence-name:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.file-name { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; line-height: 1.6; }
.evidence-name:hover .file-name { color: var(--primary-color); }
.file-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; margin-top: 4px; color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; }
.file-meta .ui-badge { font-family: var(--font-ui); }
.row-actions { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
.details-toggle svg { transition: transform var(--transition-fast); }
.rotated { transform: rotate(180deg); }
.evidence-details { border-top: 1px solid var(--border-color); padding: 12px 14px; background: var(--surface-primary); }
dl { display: grid; grid-template-columns: 60px minmax(0, 1fr); gap: 8px 12px; margin: 0 0 10px; font-size: 11px; line-height: 1.6; }
dt { color: var(--text-secondary); }
dd { margin: 0; color: var(--text-primary); overflow-wrap: anywhere; }
.file-path { font-family: var(--font-mono); user-select: text; }
@media (max-width: 520px) {
  .evidence-main { flex-wrap: wrap; gap: 8px; padding: 12px; }
  .evidence-name { flex-basis: calc(100% - 48px); }
  .row-actions { margin-left: 44px; }
}
</style>
