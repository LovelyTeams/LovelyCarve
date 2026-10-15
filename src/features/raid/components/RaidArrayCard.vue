<template>
  <article class="raid-array">
    <header class="array-header">
      <div class="array-identity">
        <h4 class="array-title" :title="`${arrayTitle(arr)} · ${arr.set_uuid}`">RAID {{ arr.level }}</h4>
        <span v-if="customName" class="array-name">{{ customName }}</span>
        <span class="ui-badge array-status" :class="status.sev">
          <component :is="statusIcons[status.sev]" :size="12" />{{ status.label }}
        </span>
        <span v-if="arr.heuristic" class="ui-badge outline" :title="heuristicTitle(arr, tr)">{{ tr("参数推定") }}</span>
        <span class="member-count mono">{{ activeMembers(arr) }} / {{ arr.raid_disks }} {{ tr("成员盘") }}</span>
      </div>
      <div v-if="arr.assembled_image_id && !isCurrentImage" class="array-actions">
        <button class="ui-btn" :disabled="exporting" :aria-busy="exporting"
          :title="tr('将重组镜像导出为 RAW（.dd）文件')" @click="emit('export')">
          <Loader2 v-if="exporting" :size="14" class="spin" />
          <HardDriveDownload v-else :size="14" />{{ tr("导出重组镜像") }}
        </button>
        <button class="ui-btn primary" :disabled="opening" :aria-busy="opening"
          :title="tr('打开重组镜像标签')" @click="emit('open')">
          <Loader2 v-if="opening" :size="14" class="spin" />
          <ExternalLink v-else :size="14" />{{ tr("打开重组镜像") }}
        </button>
      </div>
    </header>

    <section class="members-section" :aria-label="tr('成员盘顺序')">
      <ol class="member-order">
        <li v-for="entry in memberSlots" :key="entry.slot" class="member-order-item">
          <span class="ui-badge outline mono slot-number" :title="tr('槽位 {0}', { 0: entry.slot })">{{ entry.slot }}</span>
          <span v-if="entry.member" class="chip-name" :title="originTitle(entry.member.origin, imagePaths, tr)">
            {{ memberShortLabel(entry.member, imagePaths) }}
          </span>
          <span v-else class="missing-member">{{ tr("缺失成员") }}</span>
          <span v-if="entry.member?.reconstructed" class="ui-badge sm warn">{{ tr("校验重建") }}</span>
        </li>
      </ol>
    </section>

    <details v-disclosure-motion :key="`${imageId}:${arr.set_uuid}`"
      class="ui-disclosure layout-diagram array-details" :open="diagramOpen" @toggle="onDiagramToggle">
      <summary><ChevronRight :size="14" class="ui-chevron" />{{ tr("重组详情") }}</summary>
      <div class="details-content" :class="{ 'with-diagram': showDiagram(arr) }">
        <dl class="geometry-fields">
          <div><dt>{{ tr("数据块大小") }}</dt><dd class="mono">{{ chunkDisplay(arr) }}</dd></div>
          <div v-if="arr.level === 5 || arr.level === 6"><dt>{{ tr("校验布局") }}</dt><dd>{{ layoutLabel(arr, tr) }}</dd></div>
          <div><dt>{{ tr("数据偏移") }}</dt><dd class="mono">{{ dataOffsetDisplay(arr, tr) }}</dd></div>
        </dl>
        <RaidLayoutDiagram v-if="showDiagram(arr)" :level="arr.level" :layout="arr.layout"
          :num-disks="arr.raid_disks" :disk-labels="diskLabelsFor(arr, imagePaths)"
          :missing-slot="missingSlotFor(arr)" :chunk-label="chunkDisplay(arr)" />
      </div>
      <div v-if="diagnostics?.length" class="details-diagnostics diagnostics">
        <h5>{{ tr("检测说明") }}</h5>
        <ul class="mono"><li v-for="(message, index) in diagnostics" :key="index">{{ message }}</li></ul>
      </div>
    </details>
  </article>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { CircleAlert, CircleCheck, CircleDot, ChevronRight, ExternalLink, HardDriveDownload, Loader2 } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { RaidArray } from '../../../lib/ipc/raid';
import { activeMembers, arrayStatus, arrayTitle, chunkDisplay, dataOffsetDisplay, diskLabelsFor, heuristicTitle, layoutLabel, memberShortLabel, missingSlotFor, orderedMembers, originTitle, showDiagram } from '../model/stackView';
import RaidLayoutDiagram from './RaidLayoutDiagram.vue';

const props = defineProps<{ arr: RaidArray; imageId: string; imagePaths: ReadonlyMap<string, string>; opening: boolean; exporting: boolean; diagramOpen: boolean; diagnostics?: string[] }>();
const emit = defineEmits<{ open: []; export: []; 'diagram-toggle': [open: boolean] }>();
const isCurrentImage = computed(() => props.arr.assembled_image_id === props.imageId);
const statusIcons = { success: CircleCheck, warn: CircleAlert, info: CircleDot };
const status = computed(() => arrayStatus(props.arr, tr));
const customName = computed(() => props.arr.set_name === `auto-raid${props.arr.level}` ? null : props.arr.set_name);
const memberSlots = computed(() => {
  const members = new Map(orderedMembers(props.arr).map(member => [member.slot, member]));
  return Array.from({ length: props.arr.raid_disks }, (_, slot) => ({ slot, member: members.get(slot) }));
});
function onDiagramToggle(event: Event) { emit('diagram-toggle', (event.target as HTMLDetailsElement).open); }
</script>

<style scoped>
.raid-array { min-width: 0; display: flex; flex-direction: column; gap: 16px; }
.raid-array + .raid-array { border-top: 1px solid var(--border-color); padding-top: 16px; }
.array-header, .array-identity, .array-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
.array-header { justify-content: space-between; gap: 12px 16px; }
.array-identity { min-width: 0; flex: 1; }
.array-title { margin: 0; font-size: 13px; font-weight: 600; white-space: nowrap; }
.array-name { min-width: 0; font-size: 12px; color: var(--text-secondary); overflow-wrap: anywhere; }
dt { color: var(--text-secondary); font-size: 11px; }
dd { margin: 0; color: var(--text-primary); font-size: 12px; }
.member-count { font-size: 11px; color: var(--text-secondary); }
.member-order { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 140px), 1fr)); gap: 8px 16px; padding: 0; margin: 0; list-style: none; }
.member-order-item { min-width: 0; display: flex; align-items: center; gap: 8px; min-height: 24px; }
.slot-number { padding-inline: 6px; }
.chip-name { min-width: 0; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.missing-member { font-size: 12px; color: var(--sev-warn-fg); }
.array-details { border-top: 1px solid var(--border-color); }
.array-details > summary { min-height: 36px; color: var(--text-secondary); font-size: 12px; }
.array-details > summary:hover { color: var(--primary-color); }
.array-details > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; border-radius: var(--radius-sm); }
.details-content { padding-top: 8px; }
.details-content.with-diagram { display: grid; grid-template-columns: 208px minmax(0, 640px); align-items: start; gap: 24px; }
.geometry-fields { display: flex; flex-direction: column; gap: 16px; margin: 0; }
.geometry-fields > div { display: flex; flex-direction: column; gap: 4px; }
.geometry-fields dd { overflow-wrap: anywhere; }
.details-diagnostics { margin-top: 16px; color: var(--text-secondary); font-size: 11px; }
.details-diagnostics h5 { margin: 0; font-size: 11px; font-weight: 600; }
.details-diagnostics ul { display: flex; flex-direction: column; gap: 4px; margin: 8px 0 0; padding-left: 16px; overflow-wrap: anywhere; }
.mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
@container raid-card (max-width: 620px) {
  .array-identity { flex-basis: 100%; }
  .array-actions { margin-left: auto; }
  .details-content.with-diagram { grid-template-columns: 1fr; gap: 16px; }
  .geometry-fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
  .geometry-fields > div:last-child { grid-column: 1 / -1; }
}
</style>
