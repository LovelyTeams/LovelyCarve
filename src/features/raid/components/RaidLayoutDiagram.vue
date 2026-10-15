<template>
  <div v-if="usable" class="raid-diagram" :style="{ '--disk-count': cols }" :aria-label="tr('RAID {0} 条带布局示意', { 0: level })">
    <div class="diagram-scroll">
      <div class="diagram-board">
        <div class="array-grid">
          <!-- Left stripe-index gutter: numbers each row so a single stripe is
               traceable across every disk. Heights are token-locked to the disk
               columns so the rows always line up. -->
          <div class="stripe-gutter" aria-hidden="true">
            <div class="gutter-cap mono">{{ tr("条带") }}</div>
            <div class="gutter-rows">
              <div
                v-for="(_row, r) in grid"
                :key="r"
                class="gutter-row mono"
                :class="{ active: hoveredRow === r }"
              >
                S{{ r }}
              </div>
            </div>
          </div>

          <div class="disks">
            <div
              v-for="s in slotIdx"
              :key="s"
              class="disk"
              :class="{ dropped: s === droppedSlot }"
            >
              <div class="disk-head">
                <span class="disk-col mono">{{ tr("槽位") }} {{ s }}</span>
                <span class="disk-id">
                  <component
                    :is="s === droppedSlot ? AlertTriangle : HardDrive"
                    :size="11"
                    class="disk-icon"
                  />
                  <span class="disk-name mono" :title="labelFor(s)">{{ labelFor(s) }}</span>
                </span>
              </div>
              <div class="disk-body">
                <div
                  v-for="(row, r) in grid"
                  :key="r"
                  class="cell"
                  :class="[
                    cellClass(row[s]),
                    { rowhi: hoveredRow === r },
                  ]"
                  @mouseenter="hoveredRow = r"
                  @mouseleave="hoveredRow = null"
                >
                  {{ cellText(row[s]) }}
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div class="legend">
      <span class="lg mono"><i class="sw data"></i>{{ tr("数据") }}</span>
      <span class="lg mono"><i class="sw p"></i>{{ tr("P 校验") }}</span>
      <span v-if="hasQ" class="lg mono"><i class="sw q"></i>{{ tr("Q 校验") }}</span>
      <span v-if="droppedSlot != null" class="lg mono"><i class="sw dropped"></i>{{ tr("掉线盘（校验重建）") }}</span>
    </div>
    <p class="caption">{{ tr("前") }} {{ rows }} {{ tr("条带示意") }}<span v-if="chunkLabel"> {{ tr("· 每块") }} {{ chunkLabel }}</span></p>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref } from 'vue';
import { AlertTriangle, HardDrive } from 'lucide-vue-next';

// Pure-presentation stripe-layout diagram: it recomputes the mdadm parity map
// from the recovered geometry so the picture *is* the reconstruction, not a
// stock illustration. Reproduces the four RAID5/6 layouts (0=左异步/Left-Asym,
// 1=右异步/Right-Asym, 2=左同步/Left-Sym, 3=右同步/Right-Sym; 异步=asymmetric,
// 同步=symmetric) verified against the standard chart.
const props = defineProps<{
  /** RAID level: 0 (striped), 5 (single parity) or 6 (dual parity). */
  level: number;
  /** mdadm layout 0..3 — only meaningful for level 5/6. */
  layout: number;
  /** Number of columns (= raid_disks). */
  numDisks: number;
  /** Per-slot disk label (index = slot); falls back to `槽位 N`. */
  diskLabels?: (string | null)[];
  /** Slot of a dropped/reconstructed column, or null. Ignored for RAID0. */
  missingSlot?: number | null;
  /** Chunk size caption, e.g. `512 KiB`. */
  chunkLabel?: string | null;
}>();

type CellKind = 'data' | 'p' | 'q' | 'empty';
interface Cell {
  kind: CellKind;
  dataIndex?: number;
}

// Hovered stripe row (index into `grid`); drives the cross-disk highlight that
// highlights that stripe across every disk. Pure presentation.
const hoveredRow = ref<number | null>(null);

const cols = computed(() => Math.max(0, Math.floor(props.numDisks)));
const usable = computed(() => cols.value >= 2 && [0, 5, 6].includes(props.level));
const slotIdx = computed(() => Array.from({ length: cols.value }, (_, i) => i));

// Enough rows to show one full parity rotation (parity returns to its start
// after `n` stripes); RAID0 has none, so 4 rows is plenty. Capped so a wide
// array stays a readable thumbnail rather than a wall of cells.
const rows = computed(() => (props.level === 0 ? 4 : Math.min(Math.max(cols.value, 4), 8)));

const droppedSlot = computed(() => (props.level === 0 ? null : props.missingSlot ?? null));
const hasQ = computed(() => props.level === 6);

function mod(a: number, n: number): number {
  return ((a % n) + n) % n;
}

const grid = computed<Cell[][]>(() => {
  const n = cols.value;
  const out: Cell[][] = [];
  let data = 0;
  for (let r = 0; r < rows.value; r++) {
    const rowCells: Cell[] = Array.from({ length: n }, () => ({ kind: 'empty' as CellKind }));
    let pd = -1;
    let qd = -1;
    if (props.level === 5) {
      // Left layouts walk parity right→left; right layouts left→right.
      pd = props.layout === 0 || props.layout === 2 ? mod(n - 1 - r, n) : mod(r, n);
    } else if (props.level === 6) {
      pd = props.layout === 0 || props.layout === 2 ? mod(n - 2 - r, n) : mod(r, n);
      qd = mod(pd + 1, n);
    }
    if (pd >= 0) rowCells[pd] = { kind: 'p' };
    if (qd >= 0) rowCells[qd] = { kind: 'q' };

    const symmetric = props.level !== 0 && (props.layout === 2 || props.layout === 3);
    if (props.level === 0) {
      for (let c = 0; c < n; c++) rowCells[c] = { kind: 'data', dataIndex: data++ };
    } else if (symmetric) {
      // Data continues just past the parity block(s), wrapping — the rotation
      // that distinguishes symmetric from asymmetric.
      const start = props.level === 6 ? mod(qd + 1, n) : mod(pd + 1, n);
      for (let i = 0; i < n; i++) {
        const c = mod(start + i, n);
        if (rowCells[c].kind === 'empty') rowCells[c] = { kind: 'data', dataIndex: data++ };
      }
    } else {
      // Asymmetric: data fills the non-parity slots in ascending order.
      for (let c = 0; c < n; c++) {
        if (rowCells[c].kind === 'empty') rowCells[c] = { kind: 'data', dataIndex: data++ };
      }
    }
    out.push(rowCells);
  }
  return out;
});

function labelFor(slot: number): string {
  return props.diskLabels?.[slot] || tr("槽位 {0}", { 0: slot });
}

function cellClass(cell: Cell): Record<string, boolean> {
  return { data: cell.kind === 'data', p: cell.kind === 'p', q: cell.kind === 'q' };
}

function cellText(cell: Cell): string {
  if (cell.kind === 'data') return `D${(cell.dataIndex ?? 0) + 1}`;
  if (cell.kind === 'p') return 'P';
  if (cell.kind === 'q') return 'Q';
  return '';
}

</script>

<style scoped>
/* Row geometry is token-locked so the stripe gutter and every disk column share
 * the same header height / cell height / gap and therefore line up exactly. */
.raid-diagram {
  --cell-h: 28px;
  --cell-gap: 4px;
  --head-h: 40px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}

.diagram-scroll {
  overflow-x: auto;
  padding-bottom: 4px;
}

/* A subtle board seats the columns — same surface-tertiary "track" language the
 * disk-chain uses in the parent card, so the two views read as one system. */
.diagram-board {
  padding: 12px;
  min-width: calc(var(--disk-count) * 80px + 48px);
  background: var(--surface-tertiary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}

.array-grid {
  display: grid;
  grid-template-columns: 28px minmax(0, 1fr);
  gap: 8px;
}

/* ── Stripe-index gutter ── */
.stripe-gutter {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}

.gutter-cap {
  height: var(--head-h);
  display: flex;
  align-items: flex-end;
  justify-content: center;
  /* Matches .disk-head's margin so the gutter rows line up with the cells. */
  margin-bottom: 8px;
  font-size: 11px;
  color: var(--text-secondary);
  padding-bottom: 4px;
}

.gutter-rows {
  display: flex;
  flex-direction: column;
  gap: var(--cell-gap);
}

.gutter-row {
  height: var(--cell-h);
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding-right: 2px;
  font-size: 11px;
  color: var(--text-secondary);
  transition: color var(--transition-fast), font-weight var(--transition-fast);
}

.gutter-row.active {
  color: var(--primary-color);
  font-weight: 700;
}

/* ── Disk columns ── */
.disks {
  display: grid;
  grid-template-columns: repeat(var(--disk-count), minmax(0, 1fr));
  gap: 8px;
}

.disk {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.disk-head {
  height: var(--head-h);
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: 4px;
  margin-bottom: 8px;
}

.disk-col {
  font-size: 11px;
  color: var(--primary-color);
}

.disk-id {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}

.disk-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.disk-name {
  font-size: 11px;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.disk-body {
  display: flex;
  flex-direction: column;
  gap: var(--cell-gap);
}

/* Each chunk is its own rounded block — the data/P/Q colour language matches the
 * legend swatches below. */
.cell {
  height: var(--cell-h);
  display: flex;
  align-items: center;
  justify-content: center;
  font-family: var(--font-mono);
  font-size: 11px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--surface-secondary);
  color: var(--text-secondary);
  transition: opacity var(--transition-fast), box-shadow var(--transition-fast),
    border-color var(--transition-fast);
}

.cell.data {
  background: var(--sev-info-bg);
  color: var(--sev-info-fg);
  border-color: var(--sev-info-border);
}

.cell.p {
  background: var(--sev-warn-bg);
  color: var(--sev-warn-fg);
  border-color: var(--sev-warn-border);
  font-weight: 700;
}

.cell.q {
  background: var(--sev-success-bg);
  color: var(--sev-success-fg);
  border-color: var(--sev-success-border);
  font-weight: 700;
}

/* Hovering any block highlights its whole stripe (row), so one
 * stripe is traceable end-to-end across the disks. */
.cell.rowhi {
  box-shadow: 0 0 0 2px var(--primary-glow);
}


/* Dropped/reconstructed column: warning + dashed so the examiner sees at a glance
 * which physical disk's data was inferred from parity, not read. */
.disk.dropped .disk-icon {
  color: var(--sev-warn-fg);
}

.disk.dropped .disk-name {
  color: var(--sev-warn-fg);
}

.disk.dropped .cell {
  border-style: dashed;
  border-color: var(--sev-warn-border);
}

/* ── Legend ── */
.legend {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 12px;
  font-size: 11px;
  color: var(--text-secondary);
}

.lg {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.sw {
  width: 12px;
  height: 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-color);
  flex-shrink: 0;
}

.sw.data {
  background: var(--sev-info-bg);
  border-color: var(--sev-info-border);
}

.sw.p {
  background: var(--sev-warn-bg);
  border-color: var(--sev-warn-border);
}

.sw.q {
  background: var(--sev-success-bg);
  border-color: var(--sev-success-border);
}

.sw.dropped {
  background: transparent;
  border-style: dashed;
  border-color: var(--sev-warn-border);
}

.caption {
  margin: 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-secondary);
}

</style>
