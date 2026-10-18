<template>
  <article class="raid-array">
    <header class="array-header">
      <div class="array-identity">
        <h4 class="array-title" :title="`${arrayTitle(arr)} · ${arr.set_uuid}`">{{ levelShortLabel(arr.level) }}</h4>
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
          :title="tr('打开重组镜像标签页')" @click="emit('open')">
          <Loader2 v-if="opening" :size="14" class="spin" />
          <ExternalLink v-else :size="14" />{{ tr("打开重组镜像") }}
        </button>
      </div>
      <div v-else-if="arr.degradable" class="array-actions">
        <button class="ui-btn" :disabled="assembling" :aria-busy="assembling"
          :title="tr('不等待缺失成员，用其余成员的校验或镜像数据重组')" @click="emit('assemble-degraded')">
          <Loader2 v-if="assembling" :size="14" class="spin" />
          <Wrench v-else :size="14" />{{ tr("降级重组") }}
        </button>
      </div>
    </header>

    <section class="members-section" :aria-label="tr('成员盘顺序')">
      <ol class="member-order">
        <li v-for="entry in memberSlots" :key="entry.slot" class="member-order-item">
          <span class="ui-badge outline mono slot-number" :title="tr('槽位 {0}', { 0: entry.slot })">{{ entry.slot }}</span>
          <span v-if="entry.member" class="chip-name" :title="memberTitle(entry.member)">
            {{ memberShortLabel(entry.member, imagePaths) }}
          </span>
          <span v-else class="missing-member">{{ tr("缺失成员") }}</span>
          <span v-if="entry.member?.reconstructed" class="ui-badge sm warn">{{ tr("校验重建") }}</span>
          <span v-else-if="entry.member && memberStateLabel(entry.member, tr)" class="ui-badge sm warn">{{ memberStateLabel(entry.member, tr) }}</span>
        </li>
      </ol>
      <ul v-if="extraMembers.length" class="extra-members">
        <li v-for="member in extraMembers" :key="`${member.dev_number}:${member.device_uuid}:${originKey(member)}`" class="member-order-item">
          <span class="chip-name" :title="memberTitle(member)">{{ memberShortLabel(member, imagePaths) }}</span>
          <span v-if="memberStateLabel(member, tr)" class="ui-badge sm outline">{{ memberStateLabel(member, tr) }}</span>
        </li>
      </ul>
    </section>

    <details v-disclosure-motion :key="`${imageId}:${arr.set_uuid}`"
      class="ui-disclosure layout-diagram array-details" :open="diagramOpen" @toggle="onDiagramToggle">
      <summary><ChevronRight :size="14" class="ui-chevron" />{{ tr("重组详情") }}</summary>
      <div class="details-content" :class="{ 'with-diagram': showDiagram(arr) }">
        <dl class="geometry-fields">
          <div><dt>{{ tr("级别") }}</dt><dd>{{ levelLabel(arr, tr) }}</dd></div>
          <div v-if="hasChunk(arr)"><dt>{{ tr("数据块大小") }}</dt><dd class="mono">{{ chunkDisplay(arr) }}</dd></div>
          <div v-if="hasLayout(arr)"><dt>{{ arr.level === 10 ? tr("副本布局") : tr("校验布局") }}</dt><dd>{{ layoutLabel(arr, tr) }}</dd></div>
          <div><dt>{{ tr("数据偏移") }}</dt><dd class="mono">{{ dataOffsetDisplay(arr, tr) }}</dd></div>
          <div v-if="arr.metadata_version"><dt>{{ tr("元数据版本") }}</dt><dd class="mono">{{ arr.metadata_version }}</dd></div>
        </dl>
        <RaidLayoutDiagram v-if="showDiagram(arr)" :level="arr.level" :layout="arr.layout"
          :num-disks="arr.raid_disks" :disk-labels="diskLabelsFor(arr, imagePaths)"
          :missing-slots="missingSlotsFor(arr)" :chunk-label="hasChunk(arr) ? chunkDisplay(arr) : null" />
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
import { CircleAlert, CircleCheck, CircleDot, ChevronRight, ExternalLink, HardDriveDownload, Loader2, Wrench } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { RaidArray, RaidArrayMember } from '../../../lib/ipc/raid';
import {
  activeMembers, arrayStatus, arrayTitle, chunkDisplay, dataOffsetDisplay, diskLabelsFor, hasChunk, hasLayout,
  heuristicTitle, layoutLabel, levelLabel, levelShortLabel, memberShortLabel, memberStateLabel, missingSlotsFor,
  orderedMembers, originTitle, showDiagram, slotlessMembers,
} from '../model/stackView';
import RaidLayoutDiagram from './RaidLayoutDiagram.vue';

const props = defineProps<{ arr: RaidArray; imageId: string; imagePaths: ReadonlyMap<string, string>; opening: boolean; exporting: boolean; assembling?: boolean; diagramOpen: boolean; diagnostics?: string[] }>();
const emit = defineEmits<{ open: []; export: []; 'assemble-degraded': []; 'diagram-toggle': [open: boolean] }>();
const isCurrentImage = computed(() => props.arr.assembled_image_id === props.imageId);
const statusIcons = { success: CircleCheck, warn: CircleAlert, info: CircleDot };
const status = computed(() => arrayStatus(props.arr, tr));
const customName = computed(() => props.arr.set_name === `auto-raid${props.arr.level}` ? null : props.arr.set_name);
// One entry per slot: the member read there, else the stale/rebuilding one
// whose slot is rebuilt.
const memberSlots = computed(() => {
  const members = new Map<number | null, RaidArrayMember>();
  for (const member of orderedMembers(props.arr)) {
    const current = members.get(member.slot);
    if (!current || (current.state !== 'active' && member.state === 'active')) members.set(member.slot, member);
  }
  return Array.from({ length: props.arr.raid_disks }, (_, slot) => ({ slot, member: members.get(slot) }));
});
const extraMembers = computed(() => {
  const shown = new Set(memberSlots.value.map(entry => entry.member).filter(Boolean));
  return [...slotlessMembers(props.arr), ...orderedMembers(props.arr).filter(member => !shown.has(member))];
});
function memberTitle(member: RaidArrayMember): string {
  const origin = originTitle(member.origin, props.imagePaths, tr);
  return member.events == null ? origin : `${origin} · events ${member.events}`;
}
function originKey(member: RaidArrayMember): string {
  return member.origin.kind === 'whole' ? `${member.origin.image_id}:${member.origin.byte_offset}` : `${member.origin.vg_uuid}:${member.origin.lv_name}`;
}
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
.members-section { display: flex; flex-direction: column; gap: 8px; }
.member-order, .extra-members { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 140px), 1fr)); gap: 8px 16px; padding: 0; margin: 0; list-style: none; }
.member-order-item { min-width: 0; display: flex; align-items: center; gap: 8px; min-height: 24px; }
.slot-number { padding-inline: 6px; }
.chip-name { min-width: 0; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.extra-members .chip-name { color: var(--text-secondary); }
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
