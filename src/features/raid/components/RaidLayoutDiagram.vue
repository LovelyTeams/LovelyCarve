<template>
  <div v-if="usable" class="raid-diagram" :style="{ '--disk-count': cols }" :aria-label="tr('{0} 条带布局示意', { 0: levelShortLabel(level) })">
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
              :class="{ dropped: droppedSlots.has(s) }"
            >
              <div class="disk-head">
                <span class="disk-col mono">{{ tr("槽位") }} {{ s }}</span>
                <span class="disk-id">
                  <component
                    :is="droppedSlots.has(s) ? AlertTriangle : HardDrive"
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
      <span v-if="hasCopies" class="lg mono"><i class="sw copy"></i>{{ tr("镜像副本") }}</span>
      <span v-if="hasP" class="lg mono"><i class="sw p"></i>{{ tr("P 校验") }}</span>
      <span v-if="hasQ" class="lg mono"><i class="sw q"></i>{{ tr("Q 校验") }}</span>
      <span v-if="droppedSlots.size > 0" class="lg mono"><i class="sw dropped"></i>{{ droppedLegend }}</span>
    </div>
    <p class="caption">{{ tr("前") }} {{ rows }} {{ tr("条带示意") }}<span v-if="chunkLabel"> {{ tr("· 每块") }} {{ chunkLabel }}</span></p>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref } from 'vue';
import { AlertTriangle, HardDrive } from 'lucide-vue-next';
import { hasStripeLayout, stripeGrid, stripeRows, type StripeCell } from '../model/stripeLayout';
import { levelShortLabel } from '../model/stackView';

// Pure-presentation stripe-layout diagram: it recomputes md's chunk placement
// from the array geometry so the picture *is* the reconstruction, not a stock
// illustration — every md RAID5/6 rotation (异步 = asymmetric, 同步 =
// symmetric), RAID4, RAID10 copies and RAID1 mirrors.
const props = defineProps<{
  /** md level: 0, 1, 4, 5, 6 or 10. */
  level: number;
  /** md layout field: the RAID5/6 rotation or the RAID10 copy layout. */
  layout: number;
  /** Number of columns (= raid_disks). */
  numDisks: number;
  /** Per-slot disk label (index = slot); falls back to `槽位 N`. */
  diskLabels?: (string | null)[];
  /** Slots whose data is rebuilt or served by other members. */
  missingSlots?: number[];
  /** Chunk size caption, e.g. `512 KiB`. */
  chunkLabel?: string | null;
}>();

// Hovered stripe row (index into `grid`); drives the cross-disk highlight that
// highlights that stripe across every disk. Pure presentation.
const hoveredRow = ref<number | null>(null);

const cols = computed(() => Math.max(0, Math.floor(props.numDisks)));
const usable = computed(() => cols.value >= 1 && hasStripeLayout(props.level));
const slotIdx = computed(() => Array.from({ length: cols.value }, (_, i) => i));
const rows = computed(() => stripeRows(props.level, props.layout, cols.value));
const grid = computed(() => stripeGrid(props.level, props.layout, cols.value, rows.value));

const droppedSlots = computed(() => (props.level === 0 ? new Set<number>() : new Set(props.missingSlots ?? [])));
const hasP = computed(() => [4, 5, 6].includes(props.level));
const hasQ = computed(() => props.level === 6);
const hasCopies = computed(() => props.level === 1 || props.level === 10);
const droppedLegend = computed(() => (hasCopies.value ? tr("缺失盘（由镜像读取）") : tr("缺失盘（由校验重建）")));

function labelFor(slot: number): string {
  return props.diskLabels?.[slot] || tr("槽位 {0}", { 0: slot });
}

function cellClass(cell: StripeCell): Record<string, boolean> {
  return { data: cell.kind === 'data', copy: cell.kind === 'copy', p: cell.kind === 'p', q: cell.kind === 'q' };
}

function cellText(cell: StripeCell): string {
  if (cell.kind === 'data' || cell.kind === 'copy') return `D${(cell.dataIndex ?? 0) + 1}`;
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

/* A mirror copy repeats a data chunk: same hue, dashed and lighter. */
.cell.copy {
  background: var(--surface-secondary);
  color: var(--sev-info-fg);
  border-color: var(--sev-info-border);
  border-style: dashed;
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

.sw.copy {
  background: var(--surface-secondary);
  border-color: var(--sev-info-border);
  border-style: dashed;
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
