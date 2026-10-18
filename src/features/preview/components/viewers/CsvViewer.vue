<template>
  <div class="csv-viewer">
    <!-- 加载 / 错误 / 空态 -->
    <div v-if="loading" class="state-row">
      <IndeterminateBar :label="tr('正在解析表格…')" />
    </div>
    <div v-else-if="error" class="state-row mono error">
      <AlertTriangle :size="14" /> {{ tr("无法解析为表格：{0}", { 0: error }) }}
    </div>
    <div v-else-if="isEmpty" class="state-row mono">{{ tr("空文档，无可视化内容。") }}</div>

    <template v-else>
      <!-- 工具栏：首行表头开关 + 分隔符选择（切换即后端重解析）+ 行列统计总览 -->
      <div class="toolbar">
        <button
          class="ui-btn sm"
          :class="{ active: headerMode }"
          :aria-pressed="headerMode"
          :title="tr('把首行当作表头')"
          @click="headerMode = !headerMode"
        >
          <Heading :size="13" /> {{ tr("首行表头") }}
        </button>

        <label class="delim-pick">
          <span class="delim-lead">{{ tr("分隔符") }}</span>
          <UiSelect v-model="manualDelim" class="sm mono" :options="delimiterOptions" />
        </label>

        <span v-if="sortCol !== null" class="sort-tag" :title="tr('点击列头切换排序，再点一次可清除')">
          {{ sortDir === 'asc' ? tr("按「{0}」升序", { 0: headerCells[sortCol] }) : tr("按「{0}」降序", { 0: headerCells[sortCol] }) }}
          <button class="sort-clear" :title="tr('清除排序')" @click="clearSort"><X :size="11" /></button>
        </span>

        <span class="spacer"></span>

        <span class="stat mono">{{ totalDataRows.toLocaleString() }} {{ tr("行 ×") }} {{ colCount }} {{ tr("列") }}</span>
      </div>

      <!-- 表格 -->
      <div class="grid-scroll" ref="rootEl">
        <table class="data-table csv-table">
          <thead>
            <tr>
              <th class="rownum-th mono">{{ tr("行") }}</th>
              <th
                v-for="(h, ci) in rendered.header"
                :key="ci"
                class="col-th mono"
                :class="{ sorted: sortCol === ci }"
                :title="tr('{0}（点击排序）', { 0: headerCells[ci] })"
                @click="toggleSort(ci)"
              >
                <span class="th-inner">
                  <span class="th-label" v-html="h"></span>
                  <ArrowUp v-if="sortCol === ci && sortDir === 'asc'" :size="12" class="sort-ico on" />
                  <ArrowDown v-else-if="sortCol === ci" :size="12" class="sort-ico on" />
                  <ChevronsUpDown v-else :size="12" class="sort-ico" />
                </span>
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(r, ri) in rendered.rows" :key="ri">
              <td class="rownum mono">{{ visibleRows[ri].num }}</td>
              <td v-for="(cell, ci) in r" :key="ci" class="mono" :title="visibleRows[ri].cells[ci]">
                <span v-html="cell"></span>
              </td>
            </tr>
            <tr v-if="!rendered.rows.length">
              <td class="dim empty-cell mono" :colspan="colCount + 1">{{ tr("（无数据行）") }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-if="capNotes.length" class="cap-note mono">
        <AlertTriangle :size="12" />
        <span>{{ capNotes.join('　') }}</span>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// CSV / TSV 的「原类型视图」：解析在后端（core/csv_table.rs，经 fs_csv_preview 读
// 镜像内文件字节后解析），前端只负责渲染表格 + 交互——首行表头可切、分隔符可选
// （切换即后端重解析）、列头点击排序、关键字高亮/计数/定位。自取数据型（同
// SqliteViewer / PeViewer）：父级只传 image-id / node-id，不再透传文本。
import { computed, ref, watch, onBeforeUnmount } from 'vue';
import { AlertTriangle, ArrowDown, ArrowUp, ChevronsUpDown, Heading, X } from 'lucide-vue-next';
import { DELIMITERS, type Delimiter } from '../../model/csv';
import { fsCsvPreview, type CsvTable } from '../../../../lib/ipc/csv';
import { escapeHtml } from '../../../../utils/safeHtml';
import { highlightHtml } from '../../../../utils/highlight';
import IndeterminateBar from '../../../../components/ui/IndeterminateBar.vue';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';

const props = defineProps<{
  imageId: string;
  nodeId: string;
  /** 扩展名/识别给出的分隔符提示（.tsv → 制表符）；缺省时后端按内容嗅探。 */
  delimiter?: Delimiter;
  query?: string;
}>();

// 命中数上报父级（DiskFileDetail 据此显示 X/Y 计数并启用上一个/下一个）——计数来自
// 实际渲染的 <mark>，与高亮、与可见行严格一致。
const emit = defineEmits<{ (e: 'matches', count: number): void }>();
const rootEl = ref<HTMLElement | null>(null);

// DOM 渲染上限：整张大表铺进 DOM 会卡，只渲染排序后的前 ROW_CAP 行（排序/统计仍覆盖
// 已解析的全部行）。后端另有 50,000 行的返回上限（core/csv_table::MAX_ROWS）。
const ROW_CAP = 2000;
const MAX_ROWS = 50_000;
const READ_CAP_LABEL = '16 MiB';

const DELIM_LABELS: Record<Delimiter, string> = {
  get ','() { return tr("逗号 ,"); },
  get ';'() { return tr("分号 ;"); },
  get '\t'() { return tr("制表符 ⇥"); },
  get '|'() { return tr("竖线 |"); },
};

const loading = ref(true);
const error = ref<string | null>(null);
const table = ref<CsvTable | null>(null);

const headerMode = ref(true);
// 'auto' = 用提示/后端嗅探；否则用户手动指定（切换即重解析）。组件按 node.id 挂 key，切文件即重置。
const manualDelim = ref<Delimiter | 'auto'>('auto');

// 卸载守卫 + 单调请求序号：兜底「卸载后异步落地」与「切分隔符时旧请求后到」两种竞态。
let disposed = false;
let loadSeq = 0;

// 发给后端的分隔符参数：手动选了就用手动值；'auto' 时透传扩展名提示（.tsv → '\t'），
// 没提示则 undefined 让后端嗅探。
const delimArg = computed<string | undefined>(() =>
  manualDelim.value !== 'auto' ? manualDelim.value : props.delimiter,
);

async function load(): Promise<void> {
  const my = ++loadSeq;
  loading.value = true;
  error.value = null;
  try {
    const t = await fsCsvPreview({ imageId: props.imageId, nodeId: props.nodeId, delimiter: delimArg.value });
    if (disposed || my !== loadSeq) return;
    table.value = t;
    // 重解析（如切分隔符）可能让列数变少——把越界的排序列清掉，避免工具栏排序标签 /
    // 列头高亮指向一个已不存在的列。
    if (sortCol.value !== null && sortCol.value >= (t.colCount ?? 0)) clearSort();
    loading.value = false;
  } catch (e) {
    if (disposed || my !== loadSeq) return;
    error.value = String(e);
    table.value = null;
    loading.value = false;
  }
}

watch(() => [props.imageId, props.nodeId, delimArg.value], () => load(), { immediate: true });
onBeforeUnmount(() => {
  disposed = true;
});

const colCount = computed(() => table.value?.colCount ?? 0);
const isEmpty = computed(() => colCount.value === 0);
const allRows = computed(() => table.value?.rows ?? []);

// 自动模式下分隔符标签：用后端嗅探/命中的分隔符；其它情况退回扩展名提示，再不济显示「嗅探」。
const charToDelim = (s: string): Delimiter =>
  (DELIMITERS as readonly string[]).includes(s) ? (s as Delimiter) : ',';
const autoDelimLabel = computed(() => {
  if (table.value && manualDelim.value === 'auto') return DELIM_LABELS[charToDelim(table.value.delimiter)];
  if (props.delimiter) return DELIM_LABELS[props.delimiter];
  return tr("嗅探");
});
const delimiterOptions = computed<UiSelectOption<Delimiter | 'auto'>[]>(() => [
  { value: 'auto', get label() { return tr("自动（{0}）", { 0: autoDelimLabel.value }); } },
  ...DELIMITERS.map(delimiter => ({ value: delimiter, label: DELIM_LABELS[delimiter] })),
]);

// 表头：首行表头时取第 0 行，否则用列序号；不足列数处补列序号。
const headerCells = computed<string[]>(() => {
  const cols = colCount.value;
  const base = headerMode.value ? allRows.value[0] ?? [] : [];
  return Array.from({ length: cols }, (_, i) => base[i] ?? tr("列 {0}", { 0: i + 1 }));
});

// 数据行：携带「数据行序号」(num)，排序后仍指回它在文件中的原始次序（取证忠实）。
interface DataRow {
  num: number;
  cells: string[];
}
const dataRows = computed<DataRow[]>(() => {
  const cols = colCount.value;
  const body = headerMode.value ? allRows.value.slice(1) : allRows.value;
  return body.map((r, i) => ({ num: i + 1, cells: Array.from({ length: cols }, (_, c) => r[c] ?? '') }));
});

// 后端报告的总数据行数（读取窗口内，可能 > 已返回行数）——总览与截断提示都用它。
const totalDataRows = computed(() => {
  const total = table.value?.totalRows ?? 0;
  return headerMode.value ? Math.max(0, total - 1) : total;
});

// ── 排序（前端，覆盖已解析的全部数据行）──────────────────────────────────
const sortCol = ref<number | null>(null);
const sortDir = ref<'asc' | 'desc'>('asc');

function toggleSort(ci: number): void {
  if (sortCol.value !== ci) {
    sortCol.value = ci;
    sortDir.value = 'asc';
  } else if (sortDir.value === 'asc') {
    sortDir.value = 'desc';
  } else {
    clearSort(); // 第三次点击清除排序，回到文件序
  }
}
function clearSort(): void {
  sortCol.value = null;
  sortDir.value = 'asc';
}

// 数值感知：仅十进制数字串（含正负号 / 小数 / 千分位 / 科学计数）按数值比，避免
// Number('0x1f') / Number('Infinity') 之类误判；其余按字符串本地化比较。
function toNum(s: string): number | null {
  const t = s.trim().replace(/,/g, '');
  if (t === '' || !/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

const sortedRows = computed<DataRow[]>(() => {
  const base = dataRows.value;
  const col = sortCol.value;
  if (col === null || col >= colCount.value) return base;
  const dir = sortDir.value === 'asc' ? 1 : -1;
  // slice() 避免就地改 computed；现代 Array.sort 稳定，等值保持文件序。
  return base.slice().sort((a, b) => {
    const av = a.cells[col] ?? '';
    const bv = b.cells[col] ?? '';
    // 空值恒沉底（不随升降序翻转），把缺失值聚到一处。
    const ae = av === '';
    const be = bv === '';
    if (ae && be) return 0;
    if (ae) return 1;
    if (be) return -1;
    const an = toNum(av);
    const bn = toNum(bv);
    if (an !== null && bn !== null) return (an - bn) * dir;
    return av.localeCompare(bv, undefined, { numeric: true, sensitivity: 'base' }) * dir;
  });
});

// 渲染窗口：只把排序后的前 ROW_CAP 行铺进 DOM（其余靠导出 / 排序后取头部查看）。
const visibleRows = computed(() => sortedRows.value.slice(0, ROW_CAP));

const q = computed(() => (props.query ?? '').trim());

// 单元渲染一次过：表头 + 可见数据行各转一次 HTML（无关键字时仅转义；有关键字时转义
// + <mark> 命中），顺带累加命中数。结果经 Vue computed 缓存，仅随数据/表头/排序/关键字
// 变化重算。两条路径都经 safeHtml，v-html 喂入的始终是已转义文本，不存在注入面。
interface RenderedTable {
  header: string[];
  rows: string[][];
  matchCount: number;
}
const rendered = computed<RenderedTable>(() => {
  const query = q.value;
  let matchCount = 0;
  const cell = (s: string): string => {
    if (!query) return escapeHtml(s);
    const r = highlightHtml(s, query);
    matchCount += r.count;
    return r.html;
  };
  const header = headerCells.value.map(cell);
  const rows = visibleRows.value.map((r) => r.cells.map(cell));
  return { header, rows, matchCount };
});

// 命中数变化即上报；首次（immediate）也上报，让父级初始计数对齐。
watch(() => rendered.value.matchCount, (n) => emit('matches', n), { immediate: true });

// 截断提示（可叠加）：字节截断 / 行数超返回上限 / 仅渲染前 ROW_CAP 行。
const capNotes = computed<string[]>(() => {
  const t = table.value;
  if (!t) return [];
  const notes: string[] = [];
  if (t.truncatedBytes) notes.push(tr("文件大于 {0}，仅解析前 {1} 字节。", { 0: READ_CAP_LABEL, 1: READ_CAP_LABEL }));
  if (t.truncatedRows) {
    notes.push(tr("数据较多，仅解析前 {0} 行（共约 {1} 行）；导出后查看完整数据。", { 0: MAX_ROWS.toLocaleString(), 1: t.totalRows.toLocaleString() }));
  } else if (sortedRows.value.length > ROW_CAP) {
    notes.push(tr("行数较多，仅渲染前 {0} 行（排序覆盖已解析的 {1} 行）。", { 0: ROW_CAP.toLocaleString(), 1: sortedRows.value.length.toLocaleString() }));
  }
  return notes;
});

// 定位第 i 个命中（按文档序：表头在前，可见数据行行主序在后），滚动到视野并标当前；
// i<0 仅清除当前态。关键字 / 排序变化会整表重渲染，旧 <mark> 随之消失，无需额外清理。
function focusMatch(i: number): void {
  const root = rootEl.value;
  if (!root) return;
  const marks = root.querySelectorAll('mark');
  marks.forEach((el) => el.classList.remove('current'));
  const target = marks[i] as HTMLElement | undefined;
  if (target) {
    target.classList.add('current');
    target.scrollIntoView({ block: 'center', inline: 'nearest' });
  }
}

defineExpose({ focusMatch });
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped>
.csv-table { width: max-content; min-width: 100%; }
.csv-table th, .csv-table td { max-width: 360px; overflow: hidden; text-overflow: ellipsis; }

.csv-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--surface-primary);
}
/* ── 状态行 ── */
.state-row {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 24px;
  font-size: 12px;
  line-height: 1.6;
  text-align: center;
  color: var(--text-secondary);
}
.state-row.error {
  color: var(--sev-critical-fg);
}

/* ── 工具栏 ── */
.toolbar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 40px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
}

.delim-pick {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.delim-lead {
  font-size: 12px;
  color: var(--text-secondary);
}

/* 当前排序标签（可一键清除）。 */
.sort-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  font-size: 11px;
  padding: 0 3px 0 8px;
  border-radius: var(--radius-sm);
  background: var(--sev-info-bg);
  color: var(--sev-info-fg);
}
.sort-clear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 1px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.sort-clear:hover {
  background: var(--sev-info-bg-hover);
}

.spacer {
  flex: 1;
}
.stat {
  font-size: 11px;
  color: var(--text-secondary);
}

/* ── 表格 ── */
.grid-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
/* 列头可点排序：hover 提示、当前排序列高亮。 */
.col-th {
  cursor: pointer;
  user-select: none;
  transition: background var(--transition-fast), color var(--transition-fast);
}
.col-th:hover {
  background: var(--hover-bg);
}
.col-th.sorted {
  color: var(--primary-color);
}
.th-inner {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
}
.th-label {
  overflow: hidden;
  text-overflow: ellipsis;
}
.sort-ico {
  flex: none;
  color: var(--text-secondary);
  opacity: 0;
  transition: opacity var(--transition-fast);
}
.col-th:hover .sort-ico {
  opacity: 0.6;
}
.sort-ico.on {
  opacity: 1;
  color: var(--primary-color);
}

.rownum-th {
  min-width: 44px;
  color: var(--text-secondary);
  /* 左上角同时吸顶、吸左，层级高于普通表头。 */
  position: sticky;
  left: 0;
  z-index: 3;
}
.rownum {
  color: var(--text-secondary);
  text-align: right;
  /* 行号列吸附左侧，横向滚动时仍可见；z-index 1 确保盖在普通数据格之上。 */
  position: sticky;
  left: 0;
  background: var(--surface-secondary);
  z-index: 1;
}
.csv-table tbody tr:hover .rownum {
  background: var(--surface-tertiary);
}

.dim {
  color: var(--text-secondary);
}
.empty-cell {
  padding: 10px 14px;
}

/* ── 截断提示 ── */
.cap-note {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  font-size: 12px;
  border-top: 1px solid var(--sev-warn-border);
  background: var(--sev-warn-bg);
  color: var(--sev-warn-fg);
}

/* 命中高亮（v-html 注入的 <mark>），与其它预览视图一致；当前命中用强调色。 */
.csv-viewer :deep(mark) {
  background: var(--sev-warn-bg-hover);
  color: inherit;
  border-radius: 2px;
}
.csv-viewer :deep(mark.current) {
  background: var(--primary-solid);
  color: var(--accent-contrast);
}
</style>
