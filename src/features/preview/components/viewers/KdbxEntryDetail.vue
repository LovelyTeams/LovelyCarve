<template>
  <div class="report-viewer kdbx-entry">
    <header class="detail-head">
      <div class="head-text">
        <h3 class="report-name">{{ title }}</h3>
        <div class="report-badges">
          <span v-if="versionIndex !== null" class="ui-badge sm info">
            <History :size="11" />{{ tr("历史版本 {0} / {1}", { 0: versionIndex + 1, 1: entry.history.length }) }}
          </span>
          <span v-if="isExpired(version, now)" class="ui-badge sm warn"><CalendarX :size="11" />{{ tr("已过期") }}</span>
          <span v-if="row.inRecycleBin" class="ui-badge sm"><Trash2 :size="11" />{{ tr("回收站") }}</span>
          <span v-for="tag in version.tags" :key="tag" class="ui-badge sm outline"><Tag :size="10" />{{ tag }}</span>
        </div>
        <p v-if="row.path.length" class="detail-path mono" :title="row.path.join('/')">{{ row.path.join('/') }}</p>
      </div>
      <button v-if="versionIndex !== null" type="button" class="ui-btn sm" @click="showVersion(null)">
        <Undo2 :size="13" />{{ tr("当前版本") }}
      </button>
    </header>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("字段") }} <span class="ui-count">{{ fields.length }}</span></h4></div>
      <dl class="detail-list">
        <div v-for="field in fields" :key="field.key" class="detail-row" :class="{ changed: changedHere.has(field.key) }">
          <dt :title="field.key">
            <span>{{ fieldLabel(field.key, tr) }}</span>
            <Lock v-if="field.protected" :size="11" class="dt-icon" :aria-label="tr('受保护')" />
          </dt>
          <dd>
            <div class="value-row">
              <span class="value-text" :class="{ mono: field.key !== 'Notes', masked: isMasked(field) }">{{ shown(field.value, field) }}</span>
              <button v-if="field.protected && field.value" type="button" class="ui-icon-btn sm" :aria-pressed="revealed.has(field.key)"
                :title="revealed.has(field.key) ? tr('隐藏') : tr('显示')" :aria-label="revealed.has(field.key) ? tr('隐藏') : tr('显示')"
                @click="toggleReveal(field.key)">
                <EyeOff v-if="revealed.has(field.key)" :size="13" /><Eye v-else :size="13" />
              </button>
              <button v-if="field.value" type="button" class="ui-icon-btn sm"
                :title="copiedKey === field.key ? tr('已复制') : tr('复制{0}', { 0: fieldLabel(field.key, tr) })"
                :aria-label="tr('复制{0}', { 0: fieldLabel(field.key, tr) })" @click="copy(field.key, field.value)">
                <Check v-if="copiedKey === field.key" :size="13" class="ui-copied" /><Copy v-else :size="13" />
              </button>
            </div>
            <p v-if="field.resolved !== null" class="value-resolved mono" :title="tr('引用解析结果')">
              <CornerDownRight :size="12" />{{ shown(field.resolved, field) }}
            </p>
            <p v-if="field.unreadable" class="ui-field-error"><CircleAlert :size="12" /><span>{{ field.unreadable }}</span></p>
          </dd>
        </div>
      </dl>
      <span class="sr-only" role="status">{{ copiedKey === null ? '' : tr("已复制") }}</span>
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("时间") }}</h4></div>
      <dl class="detail-list">
        <div v-for="item in times" :key="item.key" class="detail-row">
          <dt>{{ item.label }}</dt>
          <dd class="mono" :title="item.title">
            {{ item.text }}
            <span v-if="item.unzoned" class="ui-badge sm warn">{{ tr("未标明时区") }}</span>
          </dd>
        </div>
      </dl>
    </section>

    <section v-if="version.attachments.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("附件") }} <span class="ui-count">{{ version.attachments.length }}</span></h4></div>
      <ul class="attachment-list">
        <li v-for="(attachment, index) in version.attachments" :key="index">
          <button type="button" class="attachment-item" :class="{ active: preview.index === index }" :aria-pressed="preview.index === index"
            :disabled="!canPreview(attachment)" @click="openAttachment(index)">
            <Paperclip :size="13" class="attachment-icon" />
            <span class="attachment-name">{{ attachment.name || tr("（未命名）") }}</span>
            <span v-if="attachment.protected" class="ui-badge sm"><Lock :size="10" /></span>
            <span class="attachment-size mono">{{ attachment.size === null ? '—' : formatBytes(attachment.size) }}</span>
          </button>
          <p v-if="attachment.unreadable" class="ui-field-error"><CircleAlert :size="12" /><span>{{ attachment.unreadable }}</span></p>
          <p v-else-if="attachment.sha256" class="attachment-hash mono" :title="attachment.sha256">SHA-256 {{ attachment.sha256 }}</p>
        </li>
      </ul>
      <div v-if="preview.index !== null" class="attachment-preview">
        <div class="preview-head">
          <span class="preview-name mono">{{ version.attachments[preview.index]?.name }}</span>
          <div class="ui-seg" role="group" :aria-label="tr('附件视图')">
            <button type="button" class="ui-seg-item" :aria-pressed="preview.tab === 'text'" @click="preview.tab = 'text'">{{ tr("文本") }}</button>
            <button type="button" class="ui-seg-item" :aria-pressed="preview.tab === 'hex'" @click="preview.tab = 'hex'">{{ tr("十六进制") }}</button>
          </div>
        </div>
        <p v-if="preview.loading" class="preview-state"><Loader2 :size="13" class="spin" />{{ tr("加载中…") }}</p>
        <p v-else-if="preview.error" class="ui-alert danger" role="alert"><CircleAlert :size="14" /><span>{{ preview.error }}</span></p>
        <template v-else-if="preview.bytes">
          <p v-if="previewTruncated" class="ui-alert warn" role="note">
            <AlertTriangle :size="14" /><span>{{ tr("已截断：仅读取前 {0}", { 0: formatBytes(preview.bytes.length) }) }}</span>
          </p>
          <div v-if="preview.tab === 'text'" class="byte-text mono">
            <div v-for="(chunk, index) in previewText" :key="index" class="text-chunk">{{ chunk }}</div>
          </div>
          <div v-else class="byte-hex mono">
            <div v-for="line in previewHex" :key="line.offset" class="hex-line">
              <span class="hex-offset">{{ line.offset }}</span><span>{{ line.bytes }}</span><span class="hex-ascii">{{ line.ascii }}</span>
            </div>
          </div>
        </template>
      </div>
    </section>

    <section v-if="versionIndex === null && entry.history.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("历史版本") }} <span class="ui-count">{{ entry.history.length }}</span></h4></div>
      <ol class="history-list">
        <li v-for="item in historyItems" :key="item.index">
          <button type="button" class="history-item" @click="showVersion(item.index)">
            <History :size="13" class="history-icon" />
            <span class="history-label">{{ tr("版本 {0}", { 0: item.index + 1 }) }}</span>
            <span class="history-time mono" :title="item.time.title">{{ item.time.text }}</span>
            <span class="history-changes">{{ item.changes }}</span>
          </button>
        </li>
      </ol>
    </section>

    <section v-if="version.auto_type" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("自动输入") }}</h4></div>
      <dl class="detail-list">
        <div class="detail-row"><dt>{{ tr("启用") }}</dt><dd>{{ yesNo(version.auto_type.enabled) }}</dd></div>
        <div v-if="version.auto_type.default_sequence" class="detail-row"><dt>{{ tr("默认序列") }}</dt><dd class="mono">{{ version.auto_type.default_sequence }}</dd></div>
        <div v-for="(association, index) in version.auto_type.associations" :key="index" class="detail-row">
          <dt>{{ tr("窗口") }}</dt>
          <dd class="mono">{{ association.window || '—' }}<span v-if="association.sequence" class="dim"> {{ association.sequence }}</span></dd>
        </div>
      </dl>
    </section>

    <section v-if="version.custom_data.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("自定义数据") }} <span class="ui-count">{{ version.custom_data.length }}</span></h4></div>
      <dl class="detail-list">
        <div v-for="item in version.custom_data" :key="item.key" class="detail-row"><dt class="mono" :title="item.key">{{ item.key }}</dt><dd class="mono">{{ item.value }}</dd></div>
      </dl>
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("其他") }}</h4></div>
      <KdbxDetailList :rows="otherFields" />
    </section>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';
import { joinList } from '../../../../i18n/translate';

import { computed, reactive, ref, watch } from 'vue';
import {
  AlertTriangle, CalendarX, Check, CircleAlert, Copy, CornerDownRight, Eye, EyeOff, History, Loader2, Lock, Paperclip, Tag,
  Trash2, Undo2,
} from 'lucide-vue-next';
import { fsKdbxAttachment, type KdbxAttachment, type KdbxCredentials, type KdbxDatabase, type KdbxEntry, type KdbxField } from '../../../../lib/ipc/kdbx';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import { formatBytes } from '../../../../utils/format';
import { useCopyFeedback } from '../../composables/useCopyFeedback';
import KdbxDetailList from './KdbxDetailList.vue';
import { hexRows, looksTextual, textChunks } from '../../model/bytePreview';
import {
  changedFields, changedLabel, fieldLabel, fieldValue, isExpired, kdbxTimeText, MASKED_VALUE, orderedFields,
  type KdbxDetail, type KdbxEntryRow,
} from '../../model/kdbx';

const props = defineProps<{
  row: KdbxEntryRow;
  database: KdbxDatabase;
  credentials: KdbxCredentials;
  imageId: ImageId;
  nodeId: NodeId;
  groupName: (uuid: string | null) => string | null;
}>();

const ATTACHMENT_FETCH_CAP = 1024 * 1024;
const now = Math.floor(Date.now() / 1000);

const versionIndex = ref<number | null>(null);
const revealed = ref(new Set<string>());
const { copiedKey, copy, reset: resetCopy } = useCopyFeedback();

const entry = computed(() => props.row.entry);
const version = computed<KdbxEntry>(() => (versionIndex.value === null ? entry.value : entry.value.history[versionIndex.value] ?? entry.value));
const title = computed(() => fieldValue(version.value, 'Title') || tr('（无标题）'));
const fields = computed(() => orderedFields(version.value));

/** Fields a history version changed relative to the version before it. */
const changedHere = computed(() => {
  const index = versionIndex.value;
  if (index === null || index === 0) return new Set<string>();
  return new Set(changedFields(entry.value.history[index - 1], entry.value.history[index]));
});

const historyItems = computed(() => entry.value.history.map((item, index) => {
  const newer = entry.value.history[index + 1] ?? entry.value;
  const changed = changedFields(item, newer).map(key => changedLabel(key, tr));
  return {
    index,
    time: kdbxTimeText(item.times.last_modification),
    changes: changed.length ? tr('之后修改：{0}', { 0: joinList(changed, tr) }) : '',
  };
}));

function isMasked(field: KdbxField): boolean {
  return field.protected && !revealed.value.has(field.key) && field.value !== '';
}

function shown(value: string, field: KdbxField): string {
  if (isMasked(field)) return MASKED_VALUE;
  return value === '' ? '—' : value;
}

function toggleReveal(key: string) {
  const next = new Set(revealed.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  revealed.value = next;
}

function yesNo(value: boolean | null): string {
  return value === null ? tr('继承') : value ? tr('是') : tr('否');
}

const times = computed(() => {
  const value = version.value.times;
  const rows = [
    { key: 'creation', label: tr('创建时间'), time: value.creation },
    { key: 'modified', label: tr('修改时间'), time: value.last_modification },
    { key: 'accessed', label: tr('访问时间'), time: value.last_access },
    { key: 'location', label: tr('位置变更时间'), time: value.location_changed },
    ...(value.expires ? [{ key: 'expiry', label: tr('过期时间'), time: value.expiry }] : []),
  ];
  return [
    ...rows.map(row => ({ key: row.key, label: row.label, ...kdbxTimeText(row.time) })),
    ...(value.usage_count === null ? [] : [{
      key: 'usage', label: tr('使用次数'), text: value.usage_count.toLocaleString(), title: '', unzoned: false,
    }]),
  ];
});

const otherFields = computed<KdbxDetail[]>(() => {
  const value = version.value;
  const rows: KdbxDetail[] = [
    { key: 'uuid', label: 'UUID', value: value.uuid, mono: true, copyable: true },
    { key: 'icon', label: tr('图标'), value: value.icon_id, mono: true },
    { key: 'customIcon', label: tr('自定义图标'), value: value.custom_icon, mono: true },
    { key: 'foreground', label: tr('前景色'), value: value.foreground_color || null, mono: true },
    { key: 'background', label: tr('背景色'), value: value.background_color || null, mono: true },
    { key: 'overrideUrl', label: tr('覆盖网址'), value: value.override_url || null, mono: true },
    { key: 'qualityCheck', label: tr('密码质量检查'), value: value.quality_check === null ? null : yesNo(value.quality_check) },
    { key: 'previousGroup', label: tr('原所在群组'), value: props.groupName(value.previous_parent_group) ?? value.previous_parent_group, mono: !props.groupName(value.previous_parent_group) },
  ];
  return rows.filter(row => row.value !== null && row.value !== '');
});

// ── 附件预览 ───────────────────────────────────────────────────────────────
const preview = reactive({
  index: null as number | null,
  bytes: null as Uint8Array | null,
  loading: false,
  error: null as string | null,
  tab: 'text' as 'text' | 'hex',
});
let previewSequence = 0;
const previewText = computed(() => textChunks(preview.bytes));
const previewHex = computed(() => hexRows(preview.bytes));
const previewTruncated = computed(() => {
  const size = preview.index === null ? null : version.value.attachments[preview.index]?.size ?? null;
  return !!preview.bytes && size !== null && size > preview.bytes.length;
});

function canPreview(attachment: KdbxAttachment): boolean {
  return !!entry.value.uuid && !attachment.unreadable && attachment.size !== null;
}

async function openAttachment(index: number) {
  const uuid = entry.value.uuid;
  if (!uuid) return;
  const current = ++previewSequence;
  preview.index = index;
  preview.bytes = null;
  preview.error = null;
  preview.loading = true;
  try {
    const buffer = await fsKdbxAttachment({
      imageId: props.imageId,
      nodeId: props.nodeId,
      credentials: props.credentials,
      attachment: { entry: uuid, ...(versionIndex.value === null ? {} : { history: versionIndex.value }), index },
      maxBytes: ATTACHMENT_FETCH_CAP,
    });
    if (current !== previewSequence) return;
    preview.bytes = new Uint8Array(buffer);
    preview.tab = looksTextual(preview.bytes) ? 'text' : 'hex';
  } catch (error) {
    if (current === previewSequence) preview.error = String(error);
  } finally {
    if (current === previewSequence) preview.loading = false;
  }
}

function closePreview() {
  previewSequence++;
  preview.index = null;
  preview.bytes = null;
  preview.error = null;
  preview.loading = false;
}

function showVersion(index: number | null) {
  versionIndex.value = index;
}

// 换版本时收起显示的密码与附件预览。
watch(versionIndex, () => {
  revealed.value = new Set();
  resetCopy();
  closePreview();
});
</script>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.kdbx-entry {
  gap: 20px;
}

.detail-head {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: 0;
}

.head-text {
  flex: 1;
  min-width: 0;
}

.detail-path {
  margin: 4px 0 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.detail-list {
  margin: 0;
}

.detail-row {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: 12px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  line-height: 1.7;
}

.detail-row.changed {
  background: var(--sev-warn-bg);
}

.detail-row dt {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  min-width: 0;
  color: var(--text-secondary);
  overflow-wrap: anywhere;
}

.dt-icon {
  flex: none;
  margin-top: 4px;
}

.detail-row dd {
  min-width: 0;
  margin: 0;
  color: var(--text-primary);
  overflow-wrap: anywhere;
}

.value-row {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  min-width: 0;
}

.value-text {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
  user-select: text;
}

.value-text.masked {
  color: var(--text-secondary);
  letter-spacing: 0.1em;
  user-select: none;
}

.value-resolved {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  margin: 2px 0 0;
  color: var(--text-secondary);
  overflow-wrap: anywhere;
}

.value-resolved > svg {
  flex: none;
  margin-top: 4px;
}

.attachment-list,
.history-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.attachment-item,
.history-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-height: 30px;
  padding: 0 8px;
  border: none;
  border-radius: var(--radius-md);
  background: none;
  color: var(--text-primary);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.attachment-item:hover:not(:disabled),
.history-item:hover {
  background: var(--hover-bg);
}

.attachment-item.active {
  background: var(--sidebar-active-bg);
  color: var(--primary-color);
}

.attachment-item:disabled {
  cursor: default;
  opacity: 0.55;
}

.attachment-item:focus-visible,
.history-item:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: -2px;
}

.attachment-icon,
.history-icon {
  flex: none;
  color: var(--text-secondary);
}

.attachment-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.history-label {
  flex: none;
  min-width: 56px;
  white-space: nowrap;
}

.attachment-size,
.history-time {
  flex: none;
  color: var(--text-secondary);
  font-size: 11px;
}

.history-changes {
  flex: 2;
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.attachment-hash {
  margin: 0 0 4px 29px;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.attachment-preview {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}

.preview-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.preview-name {
  min-width: 0;
  overflow: hidden;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.preview-state {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  color: var(--text-secondary);
  font-size: 12px;
}

.byte-text,
.byte-hex {
  max-height: 320px;
  overflow: auto;
  font-size: 12px;
  line-height: 1.6;
}

.byte-text {
  white-space: pre-wrap;
  word-break: break-all;
}

.text-chunk {
  content-visibility: auto;
  contain-intrinsic-size: auto 400px;
}

.hex-line {
  display: flex;
  gap: 12px;
  white-space: pre;
}

.hex-offset,
.hex-ascii {
  color: var(--text-secondary);
}
</style>
