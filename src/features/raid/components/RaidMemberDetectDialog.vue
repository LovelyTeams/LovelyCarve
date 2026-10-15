<template>
  <Transition name="ui-modal">
    <div
      v-if="visible"
      class="ui-modal-overlay raid-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="raid-detect-title"
      @click.self="emit('close')"
    >
      <div class="ui-modal raid-modal">
        <header class="ui-modal-header">
          <span class="ui-modal-icon"><Layers :size="18" /></span>
          <div class="ui-modal-heading">
            <h2 id="raid-detect-title" class="ui-modal-title">{{ tr("RAID 自动检测") }}</h2>
            <p class="ui-modal-subtitle">{{ tr("逐块放入成员盘，自动识别阵列；确认后再打开重组镜像") }}</p>
          </div>
          <button class="ui-icon-btn" :title="tr('重新检测')" :aria-label="tr('重新检测')" :disabled="detecting || members.length === 0" @click="emit('redetect')">
            <RefreshCw :size="16" :class="{ spin: detecting }" />
          </button>
          <button class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
        </header>

        <div class="ui-modal-body raid-body">
          <section class="member-pane" :class="{ 'drop-active': dragging }">
            <div class="pane-title">
              <span>{{ dragging ? tr("松开以添加成员盘") : tr("成员盘队列") }}</span>
              <span v-if="members.length" class="ui-count">{{ members.length }}</span>
            </div>

            <div v-if="members.length" class="member-list">
              <article
                v-for="member in members"
                :key="member.path"
                class="member-row"
                :class="`state-${memberState(member)}`"
              >
                <div class="member-icon">
                  <Loader2 v-if="member.status === 'loading'" :size="16" class="spin" />
                  <AlertTriangle v-else-if="member.status === 'failed'" :size="16" />
                  <HardDrive v-else :size="16" />
                </div>
                <div class="member-text">
                  <div class="member-title" :title="member.path">{{ baseName(member.path) }}</div>
                  <div class="member-meta mono">
                    <span>{{ member.sizeBytes == null ? tr("等待读取") : formatBytes(member.sizeBytes) }}</span>
                    <span>{{ memberStatusLabel(member) }}</span>
                  </div>
                  <div v-if="member.message" class="member-message">{{ member.message }}</div>
                </div>
                <button
                  class="ui-icon-btn sm danger row-remove"
                  :title="tr('从队列移出')"
                  :disabled="member.status === 'loading'"
                  @click="emit('remove-member', member)"
                >
                  <Trash2 :size="14" />
                </button>
              </article>
            </div>

            <div v-else class="ui-empty pane-state">
              <span class="ui-empty-icon"><HardDrive :size="22" /></span>
              <p class="ui-empty-desc">{{ tr("将镜像拖入此处，或点击下方添加成员盘。") }}</p>
            </div>

            <button class="ui-btn block add-member-btn" :disabled="detecting || opening" @click="emit('add-members')">
              <Plus :size="15" />
              <span>{{ tr("添加成员盘") }}</span>
            </button>
          </section>

          <section class="detect-pane">
            <div class="pane-title">
              <span>{{ tr("自动检测结果") }}</span>
              <span v-if="relatedArrays.length" class="ui-count accent">{{ relatedArrays.length }} {{ tr("阵列") }}</span>
            </div>

            <div v-if="detecting" class="ui-empty pane-state">
              <span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span>
              <p class="ui-empty-desc">{{ tr("正在扫描已放入成员盘与会话中的 LVM / mdadm 结构…") }}</p>
            </div>

            <div v-else-if="error" class="ui-empty pane-state error-state">
              <span class="ui-empty-icon"><AlertTriangle :size="22" /></span>
              <p class="ui-empty-desc">{{ error }}</p>
            </div>

            <div v-else-if="members.length === 0" class="ui-empty pane-state">
              <span class="ui-empty-icon"><Search :size="22" /></span>
              <p class="ui-empty-desc">{{ tr("放入成员盘后会自动检测。") }}</p>
            </div>

            <div v-else-if="relatedArrays.length === 0" class="ui-empty pane-state">
              <span class="ui-empty-icon"><CircleDashed :size="22" /></span>
              <p class="ui-empty-desc">{{ tr("还没有匹配到完整阵列。可以继续添加下一块成员盘，或稍后在镜像信息页手动重组。") }}</p>
            </div>

            <div v-else class="array-list">
              <article
                v-for="array in relatedArrays"
                :key="array.set_uuid"
                class="array-card"
                :class="`array-${arrayStatus(array).severity}`"
              >
                <div class="array-top">
                  <span class="ui-badge info mono level-pill">RAID{{ array.level }}</span>
                  <div class="array-title">
                    <span class="array-name" :title="array.set_uuid">{{ arrayTitle(array) }}</span>
                    <span class="array-sub mono">{{ activeMembers(array) }}/{{ array.raid_disks }} {{ tr("成员盘") }}</span>
                  </div>
                  <span class="ui-badge" :class="arrayStatus(array).severity">
                    {{ arrayStatus(array).label }}
                  </span>
                </div>

                <div class="array-metrics">
                  <span class="metric">
                    <span class="metric-k mono">{{ tr("容量") }}</span>
                    <span class="metric-v mono">{{ formatBytes(array.assembled_bytes) }}</span>
                  </span>
                  <span class="metric">
                    <span class="metric-k mono">{{ tr("块大小") }}</span>
                    <span class="metric-v mono">{{ chunkDisplay(array) }}</span>
                  </span>
                  <span class="metric">
                    <span class="metric-k mono">{{ tr("偏移") }}</span>
                    <span class="metric-v mono">{{ dataOffsetDisplay(array) }}</span>
                  </span>
                  <span class="metric">
                    <span class="metric-k mono">{{ tr("文件系统") }}</span>
                    <span class="metric-v">{{ filesystemDisplay(array) }}</span>
                  </span>
                </div>
              </article>
            </div>

            <details v-disclosure-motion v-if="diagnostics.length" class="diagnostics">
              <summary class="mono">{{ tr("诊断信息（") }}{{ diagnostics.length }}）</summary>
              <ul>
                <li v-for="(item, index) in diagnostics" :key="index" class="mono">{{ item }}</li>
              </ul>
            </details>
          </section>
        </div>

        <footer class="ui-modal-footer raid-footer">
          <button class="ui-btn ghost start" :disabled="opening" @click="emit('close')">{{ tr("暂不打开") }}</button>
          <button class="ui-btn" :disabled="detecting || opening" @click="emit('add-members')">
            <Plus :size="14" />
            <span>{{ tr("继续添加") }}</span>
          </button>
          <button class="ui-btn primary" :disabled="!canConfirm || opening" @click="emit('confirm-open')">
            <Loader2 v-if="opening" :size="14" class="spin" />
            <ExternalLink v-else :size="14" />
            <span>{{ tr("确认打开重组镜像") }}</span>
          </button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { formatBinaryBytes as formatBytes } from '../../../utils/format';
import { filesystemLabel } from '../../../lib/models/filesystemLabels';
import { type RaidMemberQueueItem } from '../model/types';

import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { computed, onUnmounted, watch } from 'vue';
import {
  AlertTriangle,
  CircleDashed,
  ExternalLink,
  HardDrive,
  Layers,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  X,
} from 'lucide-vue-next';
import type { RaidArray, RaidStack } from '../../../lib/ipc/raid';

const props = defineProps<{
  visible: boolean;
  members: RaidMemberQueueItem[];
  stack: RaidStack | null;
  detecting: boolean;
  opening: boolean;
  error: string | null;
  dragging?: boolean;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'add-members'): void;
  (e: 'redetect'): void;
  (e: 'confirm-open'): void;
  (e: 'remove-member', member: RaidMemberQueueItem): void;
}>();

const stagedIds = computed(() => new Set(props.members.map(member => member.id).filter((id): id is string => Boolean(id))));

const relatedArrays = computed(() => {
  if (!props.stack || stagedIds.value.size === 0) return [];
  return props.stack.arrays.filter(array => {
    for (const imageId of arrayImageIds(array)) {
      if (stagedIds.value.has(imageId)) return true;
    }
    return false;
  });
});

const diagnostics = computed(() => props.stack?.diagnostics ?? []);
const canConfirm = computed(() => !props.detecting && !props.members.some(member => member.status === 'loading')
  && relatedArrays.value.some(array => array.assembled_image_id));

function arrayImageIds(array: RaidArray): Set<string> {
  const ids = new Set<string>();
  for (const member of array.members) {
    const origin = member.origin;
    if (origin.kind === 'whole') {
      ids.add(origin.image_id);
    } else {
      const vg = props.stack?.volume_groups.find(item => item.vg_uuid === origin.vg_uuid);
      for (const pv of vg?.members ?? []) ids.add(pv.image_id);
    }
  }
  return ids;
}

function memberState(member: RaidMemberQueueItem): 'loading' | 'success' | 'warn' | 'error' | 'pending' {
  if (member.status === 'loading') return 'loading';
  if (member.status === 'failed') return 'error';
  if (member.isMember) return 'success';
  if (props.stack && relatedArrays.value.length > 0) return 'warn';
  return 'pending';
}

function memberStatusLabel(member: RaidMemberQueueItem): string {
  if (member.status === 'loading') return tr("载入中");
  if (member.status === 'failed') return tr("载入失败");
  if (member.isMember) return tr("已匹配");
  if (props.stack && relatedArrays.value.length > 0) return tr("非成员");
  return tr("待匹配");
}

function activeMembers(array: RaidArray): number {
  return array.members.filter(member => member.slot !== null).length;
}

function arrayStatus(array: RaidArray): { label: string; severity: 'success' | 'warn' | 'info' } {
  if (array.assembled_image_id) return { get label() { return tr("可打开"); }, severity: 'success' };
  if (activeMembers(array) < array.raid_disks) return { get label() { return tr("成员不全"); }, severity: 'warn' };
  return { get label() { return tr("未重组"); }, severity: 'info' };
}

function arrayTitle(array: RaidArray): string {
  return array.set_name || `md/${array.set_uuid.slice(0, 12)}`;
}

function chunkDisplay(array: RaidArray): string {
  const kib = array.chunksize_sectors / 2;
  return kib >= 1024 ? `${kib / 1024} MiB` : `${kib} KiB`;
}

function dataOffsetDisplay(array: RaidArray): string {
  const sectors = array.data_offset_sectors;
  if (sectors === 0) return tr("0 扇区");
  return tr("{0} 扇区", { 0: sectors.toLocaleString() });
}

function filesystemDisplay(array: RaidArray): string {
  const filesystem = props.stack?.filesystems.find(item => item.set_uuid === array.set_uuid);
  return filesystemLabel(filesystem?.kind);
}

function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}


function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

watch(
  () => props.visible,
  visible => {
    if (visible) document.addEventListener('keydown', onKeydown);
    else document.removeEventListener('keydown', onKeydown);
  },
);
onUnmounted(() => document.removeEventListener('keydown', onKeydown));
</script>

<style scoped>
.raid-overlay {
  z-index: 9999;
}

.raid-modal {
  width: min(900px, 100%);
  max-height: 86vh;
}

.raid-body {
  display: grid;
  grid-template-columns: minmax(300px, 0.86fr) minmax(360px, 1.14fr);
  gap: 16px;
  padding: 16px 20px;
}

.member-pane,
.detect-pane {
  min-height: 340px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.pane-title {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 24px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.member-pane.drop-active {
  outline: 2px dashed var(--primary-color);
  outline-offset: 4px;
  border-radius: var(--radius-md);
}

.drop-active .pane-title { color: var(--primary-color); }

.member-list,
.array-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  overflow: auto;
}

.member-list {
  flex: 1;
  min-height: 0;
}

.member-row,
.array-card {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
}

.member-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px;
}

.state-success {
  border-color: var(--sev-success-border);
  background: var(--sev-success-bg);
}

.state-warn {
  border-color: var(--sev-warn-border);
  background: var(--sev-warn-bg);
}

.state-error {
  border-color: var(--sev-critical-border);
  background: var(--sev-critical-bg);
}

.member-icon {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  flex-shrink: 0;
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
  color: var(--primary-color);
}

.state-error .member-icon {
  color: var(--sev-critical-fg);
}

.member-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.member-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.member-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  font-size: 11px;
  color: var(--text-secondary);
}

.member-message {
  font-size: 11px;
  line-height: 1.45;
  color: var(--sev-critical-fg);
}

.pane-state {
  flex: 1;
  min-height: 160px;
  border: 1px dashed var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
}

.error-state {
  color: var(--sev-critical-fg);
  border-color: var(--sev-critical-border);
  background: var(--sev-critical-bg);
}

.error-state .ui-empty-icon {
  background: transparent;
  color: currentColor;
}

.add-member-btn {
  height: 34px;
  border-style: dashed;
  border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color));
  background: var(--sev-info-bg);
  color: var(--primary-color);
}

.array-card {
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.array-success {
  border-color: var(--sev-success-border);
}

.array-warn {
  border-color: var(--sev-warn-border);
}

.array-info {
  border-color: var(--sev-info-border);
}

.array-top {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.array-title {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.array-name {
  min-width: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.array-sub {
  font-size: 11px;
  color: var(--text-secondary);
}

.array-metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.metric {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 7px 9px;
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
}

.metric-k {
  font-size: 11px;
  color: var(--text-secondary);
}

.metric-v {
  min-width: 0;
  font-size: 12px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diagnostics {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
  padding: 8px 10px;
  color: var(--text-secondary);
  font-size: 11px;
}

.diagnostics summary {
  cursor: pointer;
}

.diagnostics ul {
  margin: 8px 0 0;
  padding-left: 18px;
}

.diagnostics li {
  margin: 4px 0;
}

@media (max-width: 760px) {
  .raid-body {
    grid-template-columns: 1fr;
  }

  .raid-footer {
    flex-wrap: wrap;
  }

  .raid-footer .ui-btn {
    flex: 1 1 auto;
  }
}
</style>
