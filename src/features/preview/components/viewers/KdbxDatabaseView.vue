<template>
  <div class="kdbx-database">
    <!-- 视图栏：条目搜索、窄面板下代替群组清单的下拉框、锁定。 -->
    <PreviewChromeSlot area="tools">
      <label class="ui-search kdbx-search">
        <Search :size="13" aria-hidden="true" />
        <input v-model="query" type="search" :placeholder="tr('搜索条目')" :aria-label="tr('搜索条目')" spellcheck="false" />
      </label>
      <UiSelect :model-value="sourceValue" class="sm mono source-picker" :options="sourceOptions" :aria-label="tr('群组')"
        @update:model-value="selectSource" />
      <button type="button" class="ui-icon-btn sm" :title="tr('锁定')" :aria-label="tr('锁定')" @click="emit('lock')"><Lock :size="14" /></button>
    </PreviewChromeSlot>
    <PreviewChromeSlot area="statusStart">
      <span class="ui-badge sm success"><LockOpen :size="11" />{{ tr("已解密") }}</span>
      <span v-if="database.header_hash === 'mismatch'" class="ui-badge sm warn"><AlertTriangle :size="11" />{{ tr("头部哈希不符") }}</span>
      <span class="ui-footbar-stat"><strong>{{ database.counts.entries.toLocaleString() }}</strong> {{ tr("个条目") }}</span>
      <span class="ui-footbar-stat"><strong>{{ database.counts.groups.toLocaleString() }}</strong> {{ tr("个群组") }}</span>
    </PreviewChromeSlot>

    <aside class="table-list" :aria-label="tr('群组')">
      <button type="button" class="table-item" :class="{ active: view === 'info' }" :aria-pressed="view === 'info'" @click="view = 'info'">
        <Info :size="13" class="table-icon" /><span class="table-name">{{ tr("数据库信息") }}</span>
      </button>
      <div class="list-sep" aria-hidden="true" />
      <button v-for="row in groups" :key="row.key" type="button" class="table-item"
        :class="{ active: view === 'entries' && groupKey === row.key }" :aria-pressed="view === 'entries' && groupKey === row.key"
        :style="{ paddingLeft: `${8 + row.depth * 12}px` }" :title="row.path.join('/') || row.group.name" @click="selectGroup(row.key)">
        <Trash2 v-if="row.recycleBin" :size="13" class="table-icon" />
        <FolderOpen v-else-if="row.depth === 0" :size="13" class="table-icon" />
        <Folder v-else :size="13" class="table-icon" />
        <span class="table-name">{{ row.group.name || tr("（未命名）") }}</span>
        <span class="row-pill mono">{{ row.entryCount.toLocaleString() }}</span>
      </button>
      <div class="list-sep" aria-hidden="true" />
      <button type="button" class="table-item" :class="{ active: view === 'deleted' }" :aria-pressed="view === 'deleted'" @click="view = 'deleted'">
        <ArchiveX :size="13" class="table-icon" /><span class="table-name">{{ tr("已删除对象") }}</span>
        <span class="row-pill mono">{{ database.deleted_objects.length.toLocaleString() }}</span>
      </button>
    </aside>

    <div class="grid-pane">
      <KdbxDatabaseInfo v-if="view === 'info'" class="pane-fill" :database="database" :group-name="groupName" />

      <div v-else-if="view === 'deleted'" class="grid-scroll">
        <table class="data-table kdbx-table">
          <colgroup><col class="col-uuid" /><col class="col-time" /><col /></colgroup>
          <thead>
            <tr><th>UUID</th><th>{{ tr("删除时间") }}</th><th>{{ tr("现存对象") }}</th></tr>
          </thead>
          <tbody>
            <tr v-for="(object, index) in database.deleted_objects" :key="index">
              <td class="mono">{{ object.uuid ?? '—' }}</td>
              <td class="mono" :title="kdbxTimeText(object.deletion_time).title">{{ kdbxTimeText(object.deletion_time).text }}</td>
              <td>{{ existingName(object.uuid) ?? '—' }}</td>
            </tr>
            <tr v-if="!database.deleted_objects.length"><td colspan="3" class="empty-cell">{{ tr("没有已删除对象") }}</td></tr>
          </tbody>
        </table>
      </div>

      <div v-else class="entries-split">
        <div ref="tableScroll" class="grid-scroll entries-scroll">
          <table class="data-table kdbx-table">
            <thead>
              <tr>
                <th class="col-title">{{ tr("标题") }}</th>
                <th class="col-user">{{ tr("用户名") }}</th>
                <th class="col-url">{{ tr("网址") }}</th>
                <th class="col-time">{{ tr("修改时间") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in shownEntries" :key="row.key" :data-key="row.key" tabindex="0"
                :aria-selected="row.key === selectedKey ? 'true' : 'false'"
                @click="selectEntry(row)" @keydown="onRowKeydown($event, row)" @contextmenu="openMenuAt($event, row)">
                <td>
                  <span class="title-cell">
                    <KeyRound :size="13" class="row-icon" />
                    <span class="title-text" :class="{ expired: isExpired(row.entry, now) }">{{ entryTitle(row) }}</span>
                    <span v-if="row.inRecycleBin && groupKey === ''" class="ui-badge sm" :title="tr('回收站')" :aria-label="tr('回收站')"><Trash2 :size="10" /></span>
                    <span v-if="row.entry.attachments.length" class="ui-badge sm mono" :title="tr('附件')"><Paperclip :size="10" />{{ row.entry.attachments.length }}</span>
                    <span v-if="row.entry.history.length" class="ui-badge sm mono" :title="tr('历史版本')"><History :size="10" />{{ row.entry.history.length }}</span>
                  </span>
                </td>
                <td class="mono">{{ fieldValue(row.entry, 'UserName') || '—' }}</td>
                <td class="mono col-url">{{ fieldValue(row.entry, 'URL') || '—' }}</td>
                <td class="mono" :title="kdbxTimeText(row.entry.times.last_modification).title">{{ kdbxTimeText(row.entry.times.last_modification).text }}</td>
              </tr>
              <tr v-if="!shownEntries.length">
                <td colspan="4" class="empty-cell">{{ query.trim() ? tr("没有匹配的条目") : tr("没有条目") }}</td>
              </tr>
            </tbody>
          </table>
          <p v-if="visibleEntries.length > shownEntries.length" class="trunc-row mono">
            {{ tr("仅显示前 {0} 个条目，共 {1} 个", { 0: shownEntries.length.toLocaleString(), 1: visibleEntries.length.toLocaleString() }) }}
          </p>
        </div>
        <KdbxEntryDetail v-if="selectedRow" :key="selectedRow.key" class="entry-detail" :row="selectedRow" :database="database"
          :credentials="credentials" :image-id="imageId" :node-id="nodeId" :group-name="groupName" />
        <PreviewState v-else class="entry-detail" :icon="KeyRound" :title="tr('选择一个条目')" />
      </div>
    </div>

    <UiContextMenu v-model:open="menu.open" :x="menu.x" :y="menu.y" :entries="menuEntries" :label="tr('条目操作')"
      :heading="menu.target ? entryTitle(menu.target) : ''" :heading-icon="KeyRound" />
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// 解密后的数据库：左侧群组树（含回收站）与「数据库信息」「已删除对象」，右侧条目表与条目详情。
import { computed, nextTick, ref, watch } from 'vue';
import {
  AlertTriangle, ArchiveX, Copy, Folder, FolderOpen, History, Info, KeyRound, Lock, LockOpen, Paperclip, Search, Trash2,
} from 'lucide-vue-next';
import type { KdbxCredentials, KdbxDatabase } from '../../../../lib/ipc/kdbx';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import { copyText } from '../../../../composables/clipboard';
import { isContextMenuKey, useContextMenu } from '../../../../composables/useContextMenu';
import UiContextMenu, { type UiContextMenuEntry } from '../../../../components/ui/UiContextMenu.vue';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';
import {
  entryMatches, entryRows, fieldLabel, fieldValue, groupRows, isExpired, isUnder, kdbxTimeText, type KdbxEntryRow,
} from '../../model/kdbx';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import PreviewState from '../PreviewState.vue';
import KdbxDatabaseInfo from './KdbxDatabaseInfo.vue';
import KdbxEntryDetail from './KdbxEntryDetail.vue';

const props = defineProps<{
  database: KdbxDatabase;
  credentials: KdbxCredentials;
  imageId: ImageId;
  nodeId: NodeId;
}>();
const emit = defineEmits<{ lock: [] }>();

/** Rows rendered at once; a search narrows larger databases. */
const ROW_CAP = 2000;

const view = ref<'entries' | 'info' | 'deleted'>('entries');
const groupKey = ref('');
const query = ref('');
const selectedKey = ref<string | null>(null);
const tableScroll = ref<HTMLElement | null>(null);
const now = Math.floor(Date.now() / 1000);

const groups = computed(() => groupRows(props.database.root, props.database.recycle_bin));
const entries = computed(() => entryRows(groups.value));
const visibleEntries = computed(() => entries.value.filter(row => isUnder(row, groupKey.value) && entryMatches(row, query.value)));
const shownEntries = computed(() => visibleEntries.value.slice(0, ROW_CAP));
const selectedRow = computed(() => entries.value.find(row => row.key === selectedKey.value) ?? null);

const groupNames = computed(() => new Map(groups.value.filter(row => row.group.uuid).map(row => [row.group.uuid as string, row.group.name])));
function groupName(uuid: string | null): string | null {
  return uuid ? groupNames.value.get(uuid) ?? null : null;
}
const entryNames = computed(() => new Map(entries.value.filter(row => row.entry.uuid).map(row => [row.entry.uuid as string, entryTitle(row)])));
function existingName(uuid: string | null): string | null {
  return uuid ? groupName(uuid) ?? entryNames.value.get(uuid) ?? null : null;
}

function entryTitle(row: KdbxEntryRow): string {
  return fieldValue(row.entry, 'Title') || tr('（无标题）');
}

// 窄面板里代替群组清单的下拉框。群组键不含 NUL。
const INFO_SOURCE = '\u0000info';
const DELETED_SOURCE = '\u0000deleted';
const sourceValue = computed(() => (view.value === 'info' ? INFO_SOURCE : view.value === 'deleted' ? DELETED_SOURCE : groupKey.value));
const sourceOptions = computed<UiSelectOption<string>[]>(() => [
  { value: INFO_SOURCE, label: tr('数据库信息') },
  ...groups.value.map(row => ({ value: row.key, label: `${' '.repeat(row.depth)}${row.group.name || tr('（未命名）')}` })),
  { value: DELETED_SOURCE, label: tr('已删除对象') },
]);
function selectSource(value: string) {
  if (value === INFO_SOURCE) view.value = 'info';
  else if (value === DELETED_SOURCE) view.value = 'deleted';
  else selectGroup(value);
}

function selectGroup(key: string) {
  view.value = 'entries';
  groupKey.value = key;
}

function selectEntry(row: KdbxEntryRow) {
  selectedKey.value = row.key;
}

function focusRow(key: string) {
  void nextTick(() => tableScroll.value?.querySelector<HTMLElement>(`tr[data-key="${CSS.escape(key)}"]`)?.focus());
}

function onRowKeydown(event: KeyboardEvent, row: KdbxEntryRow) {
  if (isContextMenuKey(event)) {
    event.preventDefault();
    openBelow(event.currentTarget as Element, row);
    return;
  }
  if (event.key === 'Enter') {
    selectEntry(row);
    return;
  }
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  event.preventDefault();
  const rows = shownEntries.value;
  const index = rows.findIndex(candidate => candidate.key === row.key);
  const next = rows[index + (event.key === 'ArrowDown' ? 1 : -1)];
  if (!next) return;
  selectEntry(next);
  focusRow(next.key);
}

// 右键菜单：复制指针下条目的字段。
const { menu, openFromEvent, openBelow } = useContextMenu<KdbxEntryRow>();
function openMenuAt(event: MouseEvent, row: KdbxEntryRow) {
  selectEntry(row);
  openFromEvent(event, row);
}
const menuEntries = computed<UiContextMenuEntry[]>(() => {
  const row = menu.target;
  if (!row) return [];
  const copyField = (key: string) => {
    const value = fieldValue(row.entry, key);
    const label = fieldLabel(key, tr);
    return {
      id: `copy-${key}`,
      label: tr('复制{0}', { 0: label }),
      icon: Copy,
      disabled: !value,
      action: () => { if (value) void copyText(value, label); },
    };
  };
  return [
    copyField('UserName'),
    copyField('Password'),
    copyField('URL'),
    {
      id: 'copy-uuid',
      label: tr('复制 UUID'),
      icon: Copy,
      disabled: !row.entry.uuid,
      action: () => { if (row.entry.uuid) void copyText(row.entry.uuid, 'UUID'); },
    },
  ];
});

// 换群组或搜索后，选中项不在列表里就清空；切换数据库时回到首个群组。
watch(visibleEntries, rows => {
  if (selectedKey.value && !rows.some(row => row.key === selectedKey.value)) selectedKey.value = null;
});
watch(() => props.database, () => {
  view.value = 'entries';
  groupKey.value = '';
  query.value = '';
  selectedKey.value = null;
});
</script>

<style scoped src="../../../../styles/data-table.css"></style>
<style scoped src="../../../../styles/db-browser.css"></style>
<style scoped>
.kdbx-database {
  display: flex;
  flex: 1;
  align-items: stretch;
  min-height: 0;
  overflow: hidden;
}

.kdbx-search {
  width: min(220px, 40cqw);
  height: 26px;
}

.list-sep {
  flex: 0 0 auto;
  height: 1px;
  margin: 4px 2px;
  background: var(--border-color);
}

.pane-fill {
  flex: 1;
  min-height: 0;
}

/* 条目表与详情：窄时上下排列，宽时左右并排。 */
.entries-split {
  display: grid;
  flex: 1;
  grid-template-rows: minmax(140px, 42%) minmax(0, 1fr);
  min-height: 0;
}

.entries-scroll {
  container: kdbx-entries / inline-size;
  border-bottom: 1px solid var(--border-color);
}

/* A narrow table keeps title, user name and time; the URL stays in the detail. */
@container kdbx-entries (max-width: 640px) {
  .col-url {
    display: none;
  }
}

.entry-detail {
  min-height: 0;
}

@container file-preview (min-width: 1040px) {
  .entries-split {
    grid-template-columns: minmax(0, 1fr) minmax(340px, 42%);
    grid-template-rows: minmax(0, 1fr);
  }

  .entries-scroll {
    border-right: 1px solid var(--border-color);
    border-bottom: none;
  }
}

.kdbx-table {
  width: 100%;
  table-layout: fixed;
}

.kdbx-table td {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.col-title {
  width: 36%;
}

.col-user {
  width: 22%;
}

.col-time {
  width: 200px;
}

.col-uuid {
  width: 300px;
}

.title-cell {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.title-cell > .ui-badge,
.row-icon {
  flex: none;
}

.row-icon {
  color: var(--text-secondary);
}

.title-text {
  flex: 0 1 auto;
  min-width: 3em;
  overflow: hidden;
  text-overflow: ellipsis;
}

.title-text.expired {
  color: var(--text-secondary);
  text-decoration: line-through;
}

.empty-cell {
  padding: 12px 14px;
  color: var(--text-secondary);
}

.trunc-row {
  margin: 0;
  padding: 8px 14px;
  color: var(--text-secondary);
  font-size: 11px;
}
</style>
