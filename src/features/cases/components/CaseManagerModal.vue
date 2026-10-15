<template>
  <Transition name="ui-modal">
    <div v-if="visible" class="ui-modal-overlay case-overlay" @click.self="requestClose">
      <div ref="dialogEl" class="ui-modal case-modal" role="dialog" aria-modal="true" aria-labelledby="case-manager-title" tabindex="-1" :aria-busy="actionBusy" @keydown.esc.stop="requestClose">
        <header class="ui-modal-header case-head">
          <span class="ui-modal-icon"><Briefcase :size="18" aria-hidden="true" /></span>
          <div class="ui-modal-heading">
            <h2 id="case-manager-title" class="ui-modal-title" :title="activeCase?.name">{{ activeCase?.name || tr("案件管理") }}</h2>
            <p class="ui-modal-subtitle">{{ activeCase ? tr("案件 · ") + activeCase.case_number : tr("新建案件，或继续已有工作") }}</p>
          </div>
          <span v-if="activeCase?.locked" class="ui-badge warn"><Lock :size="11" /> {{ tr("已锁定") }}</span>
          <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭案件管理')" :disabled="actionBusy" @click="requestClose"><X :size="16" /></button>
        </header>

        <section v-if="!activeCase" class="ui-modal-body case-empty">
          <div class="case-empty-actions">
            <button class="ui-btn primary" :disabled="actionBusy" @click="onCreate"><Plus :size="15" /> {{ tr("新建案件") }}</button>
            <button class="ui-btn" :disabled="actionBusy" @click="onOpen"><FolderOpen :size="15" /> {{ tr("打开案件") }}</button>
          </div>
          <div v-if="registryLoading && !cases.length" class="ui-empty" role="status"><Loader2 :size="24" class="spin" /><p class="ui-empty-desc">{{ tr("正在读取案件…") }}</p></div>
          <div v-if="registryError" class="ui-alert danger" role="alert"><span class="error-text">{{ registryError }}</span><button class="ui-btn sm" :disabled="registryLoading" @click="refreshCases">{{ tr("重试") }}</button></div>
          <p v-if="!registryLoading && !registryError && !cases.length" class="hint">{{ tr("暂无最近案件") }}</p>
          <div v-if="cases.length" class="recent-list">
            <h3 class="ui-section-title">{{ tr("最近案件") }} <span class="ui-count">{{ cases.length.toLocaleString() }}</span></h3>
            <div v-for="item in cases" :key="item.dir" class="recent-row">
              <button class="recent-open" :disabled="actionBusy || item.missing" :title="item.missing ? tr('案件目录或案件台账已不可读') : item.dir" @click="onOpenListed(item)">
                <span class="recent-icon"><Clock :size="16" /></span>
                <span class="recent-text">
                  <span class="recent-name">{{ item.name }} <Lock v-if="item.locked" :size="11" /><span v-if="item.missing" class="ui-badge sm danger">{{ tr("缺失") }}</span></span>
                  <span class="recent-dir mono">{{ item.dir }}</span>
                </span>
              </button>
              <button class="ui-icon-btn danger" :aria-label="(item.missing ? tr('移除记录 ') : tr('删除案件 ')) + item.name" :title="item.missing ? tr('移除缺失案件记录') : tr('删除案件数据（保留源镜像与其他文件）')" :disabled="actionBusy" @click="onRemoveCase(item)"><Trash2 :size="14" /></button>
            </div>
          </div>
        </section>

        <template v-else>
          <div class="ui-modal-body case-body">
            <section class="case-evidence">
              <div class="section-head">
                <h3 class="ui-section-title">{{ tr("检材") }} <span class="ui-count">{{ evidence.length.toLocaleString() }}</span></h3>
                <button class="ui-btn primary" :disabled="actionBusy || activeCase.locked" :title="activeCase.locked ? tr('解锁案件后可添加') : tr('添加镜像、文件夹或磁盘')" @click="emit('add-evidence')"><Plus :size="14" /> {{ tr("添加检材") }}</button>
              </div>
              <div v-if="evidence.length" class="ev-list">
                <div v-for="group in evidenceModel.groups" :key="group.parent.image_id" class="ev-group">
                  <CaseEvidenceRow :evidence="group.parent" :current="group.parent.image_id === imageId" :raid-level="group.level" :busy="actionBusy" :locked="activeCase.locked" @open="onOpenEvidence" @remove="onRemoveEvidence" />
                  <details v-disclosure-motion v-if="group.members.length" class="member-fold">
                    <summary><ChevronRight :size="13" /> {{ tr("成员镜像 ·") }} {{ group.members.length }}</summary>
                    <div class="member-list">
                      <CaseEvidenceRow v-for="member in group.members" :key="member.image_id" :evidence="member" :current="member.image_id === imageId" :busy="actionBusy" :locked="activeCase.locked" @open="onOpenEvidence" @remove="onRemoveEvidence" />
                    </div>
                  </details>
                </div>
                <CaseEvidenceRow v-for="item in evidenceModel.standalone" :key="item.image_id" :evidence="item" :current="item.image_id === imageId" :busy="actionBusy" :locked="activeCase.locked" @open="onOpenEvidence" @remove="onRemoveEvidence" />
              </div>
              <div v-else class="ui-empty evidence-empty">
                <span class="ui-empty-icon"><HardDrive :size="24" /></span>
                <p class="ui-empty-title">{{ tr("还没有检材") }}</p>
                <p class="ui-empty-desc">{{ activeCase.locked ? tr("解锁案件后，可添加检材。") : tr("添加镜像、文件夹或磁盘，开始查看与分析。") }}</p>
              </div>
            </section>

            <details v-disclosure-motion v-if="imageId" class="section-fold">
              <summary><Layers :size="15" /><span>{{ tr("卷快照") }}</span><span class="fold-note">{{ tr("当前检材 ·") }} {{ currentVolumes.length }} {{ tr("个卷") }}</span><ChevronRight :size="15" class="fold-chevron" /></summary>
              <div class="fold-content">
                <ul v-if="currentVolumes.length" class="vol-list">
                  <li v-for="vol in currentVolumes" :key="vol.key" class="vol-row">
                    <div class="vol-text"><span class="vol-name">{{ vol.label }}</span><span class="vol-key">{{ vol.key }}</span></div>
                    <span class="vol-status">
                      <template v-if="progressFor(vol.key)"><Loader2 :size="12" class="spin" /> {{ progressFor(vol.key)!.entries.toLocaleString() }} {{ tr("条…") }}</template>
                      <template v-else-if="snapshotFor(vol.key)">
                        {{ snapshotFor(vol.key)!.entry_count.toLocaleString() }} {{ tr("条") }}
                        <span v-if="snapshotFor(vol.key)!.truncated" class="ui-badge sm warn">{{ tr("截断") }}</span>
                        <span v-if="!snapshotFor(vol.key)!.indexed" class="ui-badge sm warn" :title="tr('文件索引不可用，浏览回退到内存目录')">{{ tr("未索引") }}</span>
                        <span v-if="snapshotFor(vol.key)!.stale" class="ui-badge sm danger">{{ tr("过期") }}</span>
                      </template>
                      <template v-else>{{ tr("未建立") }}</template>
                    </span>
                    <button v-if="progressFor(vol.key)" class="ui-btn sm" :disabled="snapshotActions.has(`${imageId}:${vol.key}`)" @click="onSnapshotAction(vol.key, 'cancel')"><Loader2 v-if="snapshotActions.has(`${imageId}:${vol.key}`)" :size="13" class="spin" />{{ tr("取消") }}</button>
                    <button v-else class="ui-btn sm" :disabled="actionBusy || activeCase.locked || snapshotActions.has(`${imageId}:${vol.key}`)" @click="onSnapshotAction(vol.key, 'build')"><Loader2 v-if="snapshotActions.has(`${imageId}:${vol.key}`)" :size="13" class="spin" />{{ snapshotActions.has(`${imageId}:${vol.key}`) ? tr("提交中") : snapshotFor(vol.key) ? tr("重建") : tr("建立") }}</button>
                  </li>
                </ul>
                <p v-else class="hint">{{ tr("当前检材没有可快照的卷。") }}</p>
              </div>
            </details>

            <details v-disclosure-motion class="section-fold">
              <summary><Briefcase :size="15" /><span>{{ tr("案件详情") }}</span><ChevronRight :size="15" class="fold-chevron" /></summary>
              <div class="fold-content">
                <dl class="case-properties">
                  <dt>{{ tr("案件编号") }}</dt><dd class="mono">{{ activeCase.case_number }}</dd>
                  <dt>{{ tr("检验人") }}</dt><dd>{{ activeCase.examiner || '—' }}</dd>
                  <dt>{{ tr("存储位置") }}</dt><dd class="mono">{{ activeCase.dir }}</dd>
                  <dt>{{ tr("最近更新") }}</dt><dd>{{ formatTime(activeCase.updated_at) }}</dd>
                  <dt>{{ tr("案件快照") }}</dt><dd>{{ activeCase.snapshots.length.toLocaleString() }} {{ tr("份") }}</dd>
                  <dt>{{ tr("库版本") }}</dt><dd>v{{ activeCase.schema_version }}</dd>
                </dl>
                <button class="ui-btn sm" :disabled="actionBusy" @click="onClose"><X :size="13" /> {{ tr("关闭案件") }}</button>
              </div>
            </details>
          </div>
        </template>
        <div v-if="operationError" class="ui-alert danger case-feedback" role="alert"><span class="error-text">{{ operationError }}</span></div>
        <div v-if="pendingAction || busy" class="case-progress" role="status"><Loader2 :size="14" class="spin" />{{ pendingAction || tr("正在处理案件…") }}</div>
        <footer class="ui-modal-footer case-footer">
          <template v-if="activeCase">
            <button class="ui-btn ghost start" :disabled="actionBusy" @click="onOpen"><FolderOpen :size="14" /> {{ tr("切换案件") }}</button>
            <button class="ui-btn ghost" :disabled="actionBusy" @click="onToggleLock">
              <template v-if="activeCase.locked"><Unlock :size="14" /> {{ tr("解锁案件") }}</template>
              <template v-else><Lock :size="14" /> {{ tr("锁定案件") }}</template>
            </button>
          </template>
          <button class="ui-btn" type="button" :disabled="actionBusy" @click="requestClose">{{ tr("完成") }}</button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { computed, ref, watch } from 'vue';
import { Briefcase, ChevronRight, Clock, FolderOpen, HardDrive, Layers, Loader2, Lock, Plus, Trash2, Unlock, X } from 'lucide-vue-next';
import CaseEvidenceRow from './CaseEvidenceRow.vue';

import type { ImageInfo } from '../../../lib/ipc/image';
import { useCaseSession } from '../composables/useCaseSession';
import { useRaidOverview } from '../../raid/api';
import { confirmWithResult } from '../../../composables/useConfirm';
import { useUiPreferences } from '../../../composables/useUiPreferences';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { formatEpochSeconds } from '../../../utils/format';
import type { CaseListItem, EvidenceRef } from '../../../lib/ipc/cases';
import type { RaidArray, RaidStack } from '../../../lib/ipc/raid';

const props = defineProps<{
  visible: boolean;
  imageId: string;
  imageInfo: ImageInfo | null;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'add-evidence'): void;
  /** Absolute paths of the opened case's evidence images, for the shell to
   *  auto-open as tabs. */
  (e: 'open-evidence', paths: string[]): void;
  /** Absolute path of an evidence image just removed from the case, so the
   *  shell can close its open tab (if any). */
  (e: 'evidence-removed', path: string): void;
}>();

const {
  activeCase,
  cases,
  registryLoading,
  registryError,
  snapshots,
  buildProgress,
  busy,
  pickDirectory,
  createCase,
  openCase,
  closeCase,
  setLocked,
  removeCase,
  buildSnapshot,
  cancelBuild,
  removeEvidence,
  refreshActive,
  refreshCases,
} = useCaseSession();

const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.visible, dialogEl);
const pendingAction = ref('');
const actionBusy = computed(() => busy.value || Boolean(pendingAction.value));
const operationError = ref('');
const snapshotActions = ref(new Set<string>());
function requestClose() {
  if (!actionBusy.value) emit('close');
}

const { skipCaseEvidenceRemoveConfirm } = useUiPreferences();

/** Last path segment of a directory, used as the new case's name. */
function dirName(dir: string): string {
  const segments = dir.split(/[\\/]+/).filter(Boolean);
  return segments[segments.length - 1] ?? dir;
}

/** File name (last path segment) of an evidence image path. */
function baseName(path: string): string {
  const segments = path.split(/[\\/]+/).filter(Boolean);
  return segments[segments.length - 1] ?? path;
}

/** Evidence images recorded in the active case. */
const evidence = computed<EvidenceRef[]>(() => activeCase.value?.evidence ?? []);

// Shared session RAID detection — used only to group the evidence list (an
// assembled array's member disks nest under the assembled image). No detection
// is triggered from here; the stack is already populated whenever a RAID image
// is in view (and on case restore). Null stack ⇒ a plain flat list.
const { stack: raidStack } = useRaidOverview();

/** An assembled RAID image plus the member disks that compose it. */
interface RaidEvidenceGroup {
  parent: EvidenceRef;
  members: EvidenceRef[];
  level: number;
}

interface EvidenceListModel {
  groups: RaidEvidenceGroup[];
  /** Evidence not part of any detected RAID array, rendered as a flat list. */
  standalone: EvidenceRef[];
}

/** Member evidence of one assembled array, in slot order (col0 → colN), limited
 *  to disks actually present in this case. Whole-disk members first, then any
 *  image hosting a PV of a volume group whose LV backs a member. */
function arrayMemberEvidence(
  arr: RaidArray,
  stack: RaidStack,
  byId: Map<string, EvidenceRef>,
): EvidenceRef[] {
  const out: EvidenceRef[] = [];
  const seen = new Set<string>();
  const push = (id: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const e = byId.get(id);
    if (e) out.push(e);
  };
  for (const m of [...arr.members].sort((a, b) => (a.slot ?? 999) - (b.slot ?? 999))) {
    if (m.origin.kind === 'whole') push(m.origin.image_id);
  }
  const vgs = new Set<string>();
  for (const m of arr.members) {
    if (m.origin.kind === 'lv') vgs.add(m.origin.vg_uuid);
  }
  for (const vg of stack.volume_groups) {
    if (!vgs.has(vg.vg_uuid)) continue;
    for (const pv of vg.members) push(pv.image_id);
  }
  return out;
}

/** Group the flat evidence list: each assembled RAID image becomes a parent
 *  with its member disks nested beneath; everything else stays standalone. */
const evidenceModel = computed<EvidenceListModel>(() => {
  const evs = evidence.value;
  const stack = raidStack.value;
  if (!stack) return { groups: [], standalone: evs };

  const byId = new Map(evs.map((e) => [e.image_id, e]));
  const groups: RaidEvidenceGroup[] = [];
  const claimed = new Set<string>();

  for (const arr of stack.arrays) {
    if (!arr.assembled_image_id) continue;
    const parent = byId.get(arr.assembled_image_id);
    if (!parent) continue; // assembled image isn't this case's evidence — skip
    const members = arrayMemberEvidence(arr, stack, byId);
    claimed.add(parent.image_id);
    for (const m of members) claimed.add(m.image_id);
    groups.push({ parent, members, level: arr.level });
  }

  const standalone = evs.filter((e) => !claimed.has(e.image_id));
  return { groups, standalone };
});

// Re-sync the active case whenever the modal opens — images loaded since it was
// created/opened register as evidence on the backend, but the shared
// `activeCase` ref is only set on create/open, so it can lag. Refreshing on
// open keeps the evidence list / snapshot counts current.
watch(
  () => props.visible,
  (visible) => {
    if (visible) {
      operationError.value = '';
      void refreshActive().catch((err) => { operationError.value = tr("刷新案件失败：{0}。请关闭后重新打开。", { 0: err }); });
      void refreshCases();
    }
  },
  { immediate: true },
);

interface VolumeChoice {
  key: string;
  label: string;
}

const currentVolumes = computed<VolumeChoice[]>(() => {
  const info = props.imageInfo;
  if (!info || !props.imageId) return [];
  // docker / folder / tar / zip / 7z / AD1 evidence all surface as a single
  // synthetic `root` volume (no partition table) — mirror the backend
  // `browsable_volume_keys`, which keys them all by "root". Without these here
  // the modal would fall through to the empty `partitions` list and show
  //「没有可快照的卷」, hiding the build/rebuild control even though the backend
  // snapshots them.
  if (
    info.source_format === 'docker' ||
    info.source_format === 'folder' ||
    info.source_format === 'tar' ||
    info.source_format === 'android_backup' ||
    info.source_format === 'zip' ||
    info.source_format === 'sevenz' ||
    info.source_format === 'ad1'
  ) {
    const label =
      info.source_format === 'docker'
        ? tr("容器根")
        : info.source_format === 'folder'
          ? tr("文件夹根")
          : info.source_format === 'ad1'
            ? tr("逻辑镜像根")
            : tr("归档根");
    return [{ key: 'root', label }];
  }
  return info.partitions.map((p) => ({
    key: `part:${p.id}`,
    label: p.label || tr("分区 {0} · {1}", { 0: p.id, 1: p.kind }),
  }));
});

function snapshotFor(volumeKey: string) {
  return snapshots.value.find((s) => s.image_id === props.imageId && s.volume_key === volumeKey);
}

function progressFor(volumeKey: string) {
  return buildProgress.value.get(`${props.imageId}:${volumeKey}`);
}

async function onCreate() {
  if (actionBusy.value) return;
  pendingAction.value = tr("正在新建案件…");
  operationError.value = '';
  try {
    const dir = await pickDirectory(tr("选择新建案件的目录"));
    if (!dir) return;
    await createCase(dir, dirName(dir));
  } catch (err) {
    operationError.value = tr("新建案件失败：{0}。请检查目录后重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

async function onOpen() {
  if (actionBusy.value) return;
  pendingAction.value = tr("正在打开案件…");
  operationError.value = '';
  try {
    const dir = await pickDirectory(tr("选择已有案件目录"));
    if (!dir) return;
    await openCase(dir);
    // Auto-open every evidence image as a tab. The backend re-loads each path
    // under its recorded (stable) image id, so snapshots match and no
    // duplicate manifest rows accrue. Dedup by path keeps the list clean.
    const paths = [...new Set((activeCase.value?.evidence ?? []).map((e) => e.path))];
    if (paths.length > 0) {
      emit('open-evidence', paths);
      emit('close');
    }
  } catch (err) {
    operationError.value = tr("打开案件失败：{0}。请检查案件目录后重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

/** Open a case from the registry list. Same post-open flow as `onOpen`. A
 *  `missing` entry (directory or case ledger unreadable) can only be deleted. */
async function onOpenListed(item: CaseListItem) {
  if (actionBusy.value || item.missing) return;
  pendingAction.value = tr("正在打开案件…");
  operationError.value = '';
  try {
    await openCase(item.dir);
    const paths = [...new Set((activeCase.value?.evidence ?? []).map((e) => e.path))];
    if (paths.length > 0) {
      emit('open-evidence', paths);
      emit('close');
    }
  } catch (err) {
    operationError.value = tr("打开案件失败：{0}。请检查案件目录后重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

/** Delete a case's own files (ledger, evidence directories with their
 *  indexes, analysis data and snapshots); other files in the directory stay.
 *  Always confirms — this is irreversible. */
async function onRemoveCase(item: CaseListItem) {
  if (actionBusy.value) return;
  const result = await confirmWithResult({
    title: tr("删除案件"),
    message: item.missing
      ? tr("「{0}」的案件目录已不可读，是否从案件列表中移除？", { 0: item.name })
      : tr("确定删除案件「{0}」（{1}）吗？将删除目录 {2} 中的案件数据，包括案件台账、索引、标签、分析结果与快照；目录里的其它文件保留。镜像文件本身不受影响。此操作不可恢复。", { 0: item.name, 1: item.case_number, 2: item.dir }),
    confirmText: item.missing ? tr("移除") : tr("删除"),
    danger: true,
  });
  if (!result.confirmed || actionBusy.value || !props.visible) return;
  pendingAction.value = item.missing ? tr("正在移除记录…") : tr("正在删除案件…");
  operationError.value = '';
  try {
    await removeCase(item.dir);
  } catch (err) {
    operationError.value = tr("删除案件失败：{0}。请检查目录权限后重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

async function onToggleLock() {
  if (actionBusy.value) return;
  const locked = !(activeCase.value?.locked ?? false);
  pendingAction.value = locked ? tr("正在锁定案件…") : tr("正在解锁案件…");
  operationError.value = '';
  try {
    await setLocked(locked);
  } catch (err) {
    operationError.value = tr("{0}案件失败：{1}。请重试。", { 0: locked ? tr("锁定") : tr("解锁"), 1: err });
  } finally {
    pendingAction.value = '';
  }
}

async function onClose() {
  if (actionBusy.value) return;
  pendingAction.value = tr("正在关闭案件…");
  operationError.value = '';
  try {
    await closeCase();
  } catch (err) {
    operationError.value = tr("关闭案件失败：{0}。请重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

/** Open a single evidence image as a tab (the shell de-dups against already
 *  open tabs), then close the modal. */
function onOpenEvidence(ev: EvidenceRef) {
  if (actionBusy.value) return;
  emit('open-evidence', [ev.path]);
  emit('close');
}

/** Remove an evidence image from the case (destructive — also drops its
 *  snapshots). Confirms first unless the user opted out. */
async function onRemoveEvidence(ev: EvidenceRef) {
  if (actionBusy.value || activeCase.value?.locked) return;
  const caseDirectory = activeCase.value?.dir;
  if (!skipCaseEvidenceRemoveConfirm.value) {
    const result = await confirmWithResult({
      title: tr("从案件卸载镜像"),
      message: tr("确定从案件移除「{0}」吗？该镜像在案件内的快照会一并删除，已打开的镜像标签页也会关闭。镜像文件本身不受影响。", { 0: baseName(ev.path) }),
      confirmText: tr("卸载"),
      danger: true,
      checkbox: {
        get label() { return tr("后续不再提醒"); },
      },
    });
    if (!result.confirmed) return;
    if (result.checkboxChecked) skipCaseEvidenceRemoveConfirm.value = true;
  }
  if (actionBusy.value || activeCase.value?.locked || !props.visible || activeCase.value?.dir !== caseDirectory) return;
  pendingAction.value = tr("正在移除检材…");
  operationError.value = '';
  try {
    await removeEvidence(ev.image_id);
    // Close the image's tab (if open) now that it's no longer case evidence.
    emit('evidence-removed', ev.path);
  } catch (err) {
    operationError.value = tr("移除检材失败：{0}。请重试。", { 0: err });
  } finally {
    pendingAction.value = '';
  }
}

async function onSnapshotAction(volumeKey: string, action: 'build' | 'cancel') {
  const imageId = props.imageId;
  const actionKey = `${imageId}:${volumeKey}`;
  if (snapshotActions.value.has(actionKey) || actionBusy.value || (action === 'build' && activeCase.value?.locked)) return;
  const directory = activeCase.value?.dir;
  snapshotActions.value = new Set(snapshotActions.value).add(actionKey);
  operationError.value = '';
  try {
    if (action === 'build') await buildSnapshot(imageId, volumeKey);
    else await cancelBuild(imageId, volumeKey);
  } catch (cause) {
    if (props.imageId === imageId && activeCase.value?.dir === directory) {
      operationError.value = tr("{0}快照失败：{1}。请重试。", { 0: action === 'build' ? tr("建立") : tr("取消"), 1: String(cause) });
    }
  } finally {
    const next = new Set(snapshotActions.value);
    next.delete(actionKey);
    snapshotActions.value = next;
  }
}

function formatTime(iso: string): string {
  const formatted = formatEpochSeconds(Date.parse(iso) / 1000, { precision: 'seconds', fallback: '', zeroAsMissing: false });
  return formatted ? tr("{0}（本地）", { 0: formatted }) : tr("时间未知");
}
</script>

<style scoped>
.case-overlay { z-index: 9999; }
.case-modal { width: min(720px, 100%); max-height: min(82vh, 800px); }
.case-body, .case-empty { padding: 20px 24px 0; }
.case-empty { padding-bottom: 24px; }
.section-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
.ev-list { display: flex; flex-direction: column; gap: 8px; padding-bottom: 20px; }
.evidence-empty { padding: 24px 12px 32px; }
.section-fold { border-top: 1px solid var(--border-color); }
summary { display: flex; align-items: center; gap: 8px; list-style: none; cursor: pointer; color: var(--text-secondary); font-size: 12px; }
summary::-webkit-details-marker { display: none; }
.section-fold > summary { padding: 14px 0; color: var(--text-primary); font-size: 13px; font-weight: 500; }
.section-fold > summary > svg:first-child { color: var(--primary-color); }
summary:hover { color: var(--primary-color); }
.fold-note { margin-left: auto; color: var(--text-secondary); font-size: 11px; font-weight: 400; }
.fold-chevron { margin-left: auto; color: var(--text-secondary); }
.fold-note + .fold-chevron { margin-left: 0; }
summary svg { flex-shrink: 0; }
.fold-chevron, .member-fold > summary svg { transition: transform var(--transition-fast); }
details[open] > summary > .fold-chevron, .member-fold[open] > summary > svg { transform: rotate(90deg); }
summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.fold-content { padding: 0 0 16px; }
.member-fold { margin: 8px 0 0 18px; padding-left: 12px; border-left: 1px solid var(--border-color); }
.member-fold > summary { padding: 4px 0; }
.member-list { display: grid; gap: 6px; margin-top: 6px; }
.case-properties { display: grid; grid-template-columns: 72px minmax(0, 1fr); gap: 10px 16px; margin: 0 0 16px; font-size: 12px; line-height: 1.5; }
dt { color: var(--text-secondary); }
dd { margin: 0; overflow-wrap: anywhere; user-select: text; }
.mono { font-size: 11px; }
.vol-list { list-style: none; margin: 0; padding: 0; display: grid; gap: 8px; }
.vol-row { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border: 1px solid var(--border-color); background: var(--surface-primary); border-radius: var(--radius-md); }
.vol-text { display: flex; flex-direction: column; gap: 4px; min-width: 0; flex: 1; }
.vol-name { font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.vol-key { font-family: var(--font-mono); font-size: 11px; color: var(--text-secondary); overflow-wrap: anywhere; }
.vol-status { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 4px; font-size: 11px; color: var(--text-secondary); }
.hint { margin: 0; color: var(--text-secondary); font-size: 12px; }
.case-feedback { margin: 12px 20px; flex-shrink: 0; }
.error-text { min-width: 0; overflow-wrap: anywhere; flex: 1; }
.case-progress { display: flex; align-items: center; gap: 8px; padding: 8px 20px; color: var(--text-secondary); font-size: 11px; }
.case-empty-actions { display: flex; gap: 8px; margin-bottom: 24px; }
.recent-list { display: grid; gap: 6px; }
.recent-list h3 { margin-bottom: 6px; }
.recent-row { display: flex; gap: 8px; align-items: center; border-bottom: 1px solid var(--border-color); padding: 4px 0; }
.recent-row:last-child { border-bottom: 0; }
.recent-open { display: flex; flex: 1; min-width: 0; align-items: center; gap: 12px; padding: 8px; border: 0; border-radius: var(--radius-md); background: transparent; text-align: left; color: var(--text-secondary); cursor: pointer; transition: background var(--transition-fast); }
.recent-open:hover:not(:disabled) { background: var(--hover-bg); }
.recent-open:disabled { opacity: 0.5; cursor: not-allowed; }
.recent-open:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.recent-icon { display: grid; place-items: center; flex-shrink: 0; width: 32px; height: 32px; border-radius: var(--radius-md); background: var(--surface-tertiary); color: var(--text-secondary); }
.recent-text { display: grid; gap: 4px; min-width: 0; }
.recent-name { display: flex; gap: 6px; align-items: center; font-size: 13px; font-weight: 500; color: var(--text-primary); overflow-wrap: anywhere; }
.recent-dir { font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
@media (max-width: 520px) {
  .case-overlay { padding: 12px; }
  .case-modal { max-height: 90vh; }
  .case-head { padding: 16px; gap: 8px; }
  .case-body, .case-empty { padding: 16px 16px 0; }
  .case-empty { padding-bottom: 16px; }
  .vol-row { flex-wrap: wrap; }
}
</style>
