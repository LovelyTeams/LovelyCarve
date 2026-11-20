<template>
  <ul class="bcp-directory">
    <li v-for="row in rows" :key="row.id" class="bcp-tree-row"
      :style="{ '--bcp-indent': `${row.depth * 12}px` }">
      <button class="ui-btn ghost bcp-row-button" type="button" :title="row.title || row.label"
        :aria-expanded="row.node ? undefined : open(row.id)"
        :aria-current="row.node && row.node.nodeId === selectedId ? 'true' : undefined"
        @click="row.node ? emit('select', row.node) : toggle(row.id)">
        <ChevronRight v-if="!row.node" :size="12" class="bcp-chevron" :class="{ open: open(row.id) }" aria-hidden="true" />
        <span v-else aria-hidden="true"></span>
        <img v-if="row.icon && icons[row.icon]" class="bcp-icon" :src="icons[row.icon]" alt="" />
        <component v-else :is="row.glyph" :size="16" class="bcp-glyph" aria-hidden="true" />
        <span class="bcp-name">{{ row.label }}</span>
        <span class="bcp-count" :title="tr('已删除 {0} 条，共 {1} 条', { 0: row.deleted.toLocaleString(), 1: row.total.toLocaleString() })">
          <template v-if="row.deleted"><span class="bcp-deleted">{{ row.deleted.toLocaleString() }}</span><span aria-hidden="true">/</span></template>{{ row.total.toLocaleString() }}
        </span>
      </button>
    </li>
  </ul>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { AppWindow, ChevronRight, Table2, UserRound } from 'lucide-vue-next';
import { tr } from '../../../composables/useLocale';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';
import type { BcpDirectory, BcpResultItem } from '../model/bcpDirectory';

const props = defineProps<{
  directory: BcpDirectory;
  selectedId: string | undefined;
  searching: boolean;
  nodeLabel: (node: AnalysisResultNode) => string;
}>();
const emit = defineEmits<{ select: [node: AnalysisResultNode] }>();
const icons = Object.fromEntries(Object.entries(import.meta.glob<string>(
  '../../../assets/bcp-apps/*.{png,jpg}', { eager: true, import: 'default', query: '?url' },
)).map(([path, url]) => [path.slice(path.lastIndexOf('/') + 1), url]));
const expanded = ref(new Set<string>());

interface Row {
  id: string;
  label: string;
  title?: string;
  icon?: string;
  glyph: typeof AppWindow;
  depth: number;
  total: number;
  deleted: number;
  node?: AnalysisResultNode;
}

function open(id: string) { return props.searching || expanded.value.has(id); }
function toggle(id: string) {
  if (expanded.value.has(id)) expanded.value.delete(id);
  else expanded.value.add(id);
}
function resultRow({ node, deleted }: BcpResultItem, depth: number): Row {
  return { id: node.nodeId, label: props.nodeLabel(node), node, deleted, total: node.childCount, depth, glyph: Table2 };
}

const rows = computed<Row[]>(() => props.directory.apps.flatMap(app => {
  const branch = { ...app, title: [app.label, ...app.codes].join(' · '), depth: 0, glyph: AppWindow };
  if (!open(app.id)) return [branch];
  return [branch, ...app.results.map(item => resultRow(item, 1)), ...app.accounts.flatMap(account => {
    const title = account.label === account.account ? account.label : `${account.label} · ${account.account}`;
    const row = { ...account, title, depth: 1, glyph: UserRound };
    return open(account.id) ? [row, ...account.results.map(item => resultRow(item, 2))] : [row];
  })];
}));

watch(() => props.selectedId, id => {
  if (!id) return;
  const selected = (item: BcpResultItem) => item.node.nodeId === id;
  for (const app of props.directory.apps) {
    const account = app.accounts.find(candidate => candidate.results.some(selected));
    if (!account && !app.results.some(selected)) continue;
    expanded.value.add(app.id);
    if (account) expanded.value.add(account.id);
    break;
  }
}, { immediate: true });
</script>

<style scoped>
.bcp-directory { list-style: none; margin: 6px 0; padding: 0; }
.bcp-row-button {
  display: grid;
  grid-template-columns: 12px 18px minmax(0, 1fr) auto;
  gap: 6px;
  width: 100%;
  padding-inline: calc(6px + var(--bcp-indent)) 8px;
  text-align: start;
  color: var(--text-primary);
  font-weight: 400;
}
.bcp-row-button[aria-current="true"] { background: var(--sidebar-active-bg); color: var(--primary-color); }
.bcp-chevron { color: var(--text-secondary); }
.bcp-chevron.open { transform: rotate(90deg); }
.bcp-icon { width: 18px; height: 18px; }
.bcp-icon { border-radius: var(--radius-sm); object-fit: contain; }
.bcp-glyph { justify-self: center; color: var(--text-secondary); }
.bcp-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.bcp-count { text-align: end; color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; font-variant-numeric: tabular-nums; }
.bcp-deleted { color: var(--sev-critical-fg); }
</style>
