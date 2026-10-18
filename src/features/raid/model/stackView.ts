import { joinList, sourceText, type Translator } from '../../../i18n/translate';
import type { RaidStack, RaidArray, RaidArrayMember, RaidMemberOrigin, RaidMemberState, RaidVolumeGroup } from '../../../lib/ipc/raid';
import { formatBinaryBytes as formatBytes } from '../../../utils/format';
import { decodeRaid10Layout, hasStripeLayout } from './stripeLayout';
export { formatBytes };

/** VGs with a PV on this image, or whose cross-PV logical volume this image is. */
export function relevantRaidVolumeGroups(stack: RaidStack | null, imageId: string): RaidVolumeGroup[] {
  return stack?.volume_groups.filter(vg =>
    vg.members.some(m => m.image_id === imageId)
    || vg.logical_volumes.some(lv => lv.assembled_image_id === imageId)) ?? [];
}

/** Arrays this image participates in: a whole-disk member lives on it, an
 *  LV member's volume group has a PV on it, or the image IS the assembled
 *  synthetic device (its info page shows the array it came from). */
export function relevantRaidArrays(s: RaidStack | null, imageId: string): RaidArray[] {
  if (!s) return [];
  const vgOnImage = new Set(
    s.volume_groups
      .filter(vg => vg.members.some(m => m.image_id === imageId))
      .map(vg => vg.vg_uuid),
  );
  return s.arrays.filter(
    arr =>
      arr.assembled_image_id === imageId ||
      arr.members.some(m =>
        m.origin.kind === 'whole' ? m.origin.image_id === imageId : vgOnImage.has(m.origin.vg_uuid),
      ),
  );
}

export function arrayTitle(arr: RaidArray): string {
  return arr.set_name || `md/${arr.set_uuid.slice(0, 12)}`;
}

/** Compact level name for titles: `Linear`, `RAID 5`, … */
export function levelShortLabel(level: number): string {
  return level === -1 ? 'Linear' : `RAID ${level}`;
}

/** Tooltip for the metadata-less badge: the parameters the heuristic
 *  recovered, so an examiner can see the reconstruction was inferred. */
export function heuristicTitle(arr: RaidArray, tr: Translator = sourceText): string {
  const parts = [tr("无 RAID 元数据，参数经文件系统校验推定"), tr("块大小 {0}K", { 0: arr.chunksize_sectors / 2 })];
  if (arr.level === 5) parts.push(tr("布局 {0}", { 0: layoutLabel(arr, tr) }));
  parts.push(tr("数据偏移 {0} 扇区", { 0: arr.data_offset_sectors }));
  return parts.join(' · ');
}

/** Members currently read: active members, minus stale or rebuilding ones
 *  whose slot is rebuilt from redundancy. */
export function activeMembers(arr: RaidArray): number {
  return arr.members.filter(m => m.slot !== null && m.state === 'active' && !m.reconstructed).length;
}

type StatusSeverity = 'success' | 'warn' | 'info';

/** md levels the backend assembles. */
const SUPPORTED_LEVELS = new Set([-1, 0, 1, 4, 5, 6, 10]);
const SUPPORTED_RAID6_LAYOUTS = new Set([0, 1, 2, 3, 4, 5, 8, 9, 10, 16, 17, 18, 19, 20]);

function layoutSupported(arr: RaidArray): boolean {
  if (arr.level === 5) return arr.layout >= 0 && arr.layout <= 5;
  if (arr.level === 6) return SUPPORTED_RAID6_LAYOUTS.has(arr.layout);
  return true;
}

export function arrayStatus(arr: RaidArray, tr: Translator = sourceText): { label: string; sev: StatusSeverity } {
  if (arr.assembled_image_id) {
    return arr.degraded ? { label: tr("已降级重组"), sev: 'warn' } : { label: tr("已重组"), sev: 'success' };
  }
  if (arr.degradable) {
    return { label: tr("缺 {0} 块，可降级重组", { 0: arr.raid_disks - activeMembers(arr) }), sev: 'warn' };
  }
  // Firmware RAID members whose disk record is unknown: present, but their
  // slots cannot be told.
  if (arr.members.some(m => m.state === 'unidentified')) {
    return { label: tr("成员顺序未知，见诊断"), sev: 'warn' };
  }
  if (activeMembers(arr) < arr.raid_disks) {
    return { label: tr("成员不全 {0}/{1}", { 0: activeMembers(arr), 1: arr.raid_disks }), sev: 'warn' };
  }
  if (!SUPPORTED_LEVELS.has(arr.level)) {
    return { label: tr("RAID {0} 暂不支持重组", { 0: arr.level }), sev: 'info' };
  }
  if (!layoutSupported(arr)) {
    return { label: tr("RAID {0} 布局 {1} 暂不支持重组", { 0: arr.level, 1: arr.layout }), sev: 'info' };
  }
  // Remaining causes (reshape in progress, member geometry conflicts, probe
  // read errors) are only distinguishable via the diagnostics list.
  return { label: tr("未能重组，见诊断"), sev: 'warn' };
}

/** Relative capacity of a member vs. the largest column (0..100), for the thin
 *  micro-gauge under each disk chip. Pure presentation — surfaces relative
 *  member sizes from data already on screen; never affects layout/logic. */
export function gaugePct(arr: RaidArray, m: RaidArrayMember): number {
  const max = Math.max(...arr.members.map(mm => mm.data_size_sectors), 1);
  return Math.max(4, Math.round((m.data_size_sectors / max) * 100));
}

/** Members that hold a column, in slot order (col0 → colN). Spares, faulty,
 *  journal and duplicate entries are dropped — the chain shows the layout. */
export function orderedMembers(arr: RaidArray): RaidArrayMember[] {
  return arr.members
    .filter(m => m.slot != null && m.state !== 'duplicate')
    .sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
}

/** Members without a column (spare, faulty, journal) or loaded twice. */
export function slotlessMembers(arr: RaidArray): RaidArrayMember[] {
  return arr.members.filter(m => m.slot == null || m.state === 'duplicate');
}

const MEMBER_STATE_LABELS: Record<Exclude<RaidMemberState, 'active'>, string> = {
  spare: '备用',
  faulty: '故障',
  journal: '日志盘',
  stale: '陈旧',
  rebuilding: '重建中',
  duplicate: '重复加载',
  unidentified: '未匹配',
};

/** Badge text for a member that is not simply read; null for active members. */
export function memberStateLabel(m: RaidArrayMember, tr: Translator = sourceText): string | null {
  return m.state === 'active' ? null : tr(MEMBER_STATE_LABELS[m.state]);
}

/** Compact disk name for a member: the .dd basename for whole-disk members,
 *  the LV name for LV-backed ones. */
export function memberShortLabel(m: RaidArrayMember, imagePaths: ReadonlyMap<string, string>): string {
  if (m.origin.kind === 'lv') return m.origin.lv_name;
  const p = imagePaths.get(m.origin.image_id);
  return p ? baseName(p) : m.origin.image_id.slice(0, 8);
}

// Display names for md RAID5/6 layouts. The 异步/同步 wording is the convention
// domestic forensic / RAID rebuild tools use: 异步 = asymmetric, 同步 = symmetric.
const PARITY_LAYOUT_NAMES: Record<number, string> = {
  0: '左异步',
  1: '右异步',
  2: '左同步',
  3: '右同步',
  4: '首盘校验',
  5: '末盘校验',
  8: 'DDF 零起轮转',
  9: 'DDF 末盘起轮转',
  10: 'DDF 连续轮转',
  16: '左异步（Q 固定末盘）',
  17: '右异步（Q 固定末盘）',
  18: '左同步（Q 固定末盘）',
  19: '右同步（Q 固定末盘）',
  20: '首盘校验（Q 固定末盘）',
};

/** Selectable RAID5 / RAID6 layouts in display order. */
export const RAID5_LAYOUTS = [0, 1, 2, 3, 4, 5];
export const RAID6_LAYOUTS = [0, 1, 2, 3, 4, 5, 8, 9, 10, 16, 17, 18, 19, 20];

export function parityLayoutName(layout: number, tr: Translator = sourceText): string {
  const name = PARITY_LAYOUT_NAMES[layout];
  return name ? tr(name) : tr("布局 #{0}", { 0: layout });
}

const LEVEL_NAMES: Record<number, string> = {
  [-1]: '线性（JBOD）',
  0: 'RAID 0（条带）',
  1: 'RAID 1（镜像）',
  4: 'RAID 4（专用校验）',
  5: 'RAID 5（单校验）',
  6: 'RAID 6（双校验）',
  10: 'RAID 10（条带镜像）',
};

export function levelLabel(arr: RaidArray, tr: Translator = sourceText): string {
  const name = LEVEL_NAMES[arr.level];
  return name ? tr(name) : `RAID ${arr.level}`;
}

/** RAID10 copy layout, e.g. `近端 2 份`, `远端 2 份`, `偏移 2 份`. */
export function raid10LayoutName(layout: number, tr: Translator = sourceText): string {
  const { near, far, offset } = decodeRaid10Layout(layout);
  const parts: string[] = [];
  if (near > 1) parts.push(tr("近端 {0} 份", { 0: near }));
  if (far > 1) parts.push(offset ? tr("偏移 {0} 份", { 0: far }) : tr("远端 {0} 份", { 0: far }));
  return parts.join(' · ') || tr("布局 #{0}", { 0: layout });
}

/** Layout of the array in words, for the levels that have one. */
export function layoutLabel(arr: RaidArray, tr: Translator = sourceText): string {
  return arr.level === 10 ? raid10LayoutName(arr.layout, tr) : parityLayoutName(arr.layout, tr);
}

/** Whether the level has a layout worth naming. */
export function hasLayout(arr: RaidArray): boolean {
  return arr.level === 5 || arr.level === 6 || arr.level === 10;
}

/** Whether the chunk size matters for the level (RAID1 has none). */
export function hasChunk(arr: RaidArray): boolean {
  return arr.level !== 1 && arr.chunksize_sectors > 0;
}

/** Chunk (stripe unit) size — `chunksize_sectors` is in 512-byte sectors. */
export function chunkDisplay(arr: RaidArray): string {
  const kib = arr.chunksize_sectors / 2;
  return kib >= 1024 ? `${kib / 1024} MiB` : `${kib} KiB`;
}

/** Per-member reserved header before the array data starts. */
export function dataOffsetDisplay(arr: RaidArray, tr: Translator = sourceText): string {
  const s = arr.data_offset_sectors;
  if (s === 0) return tr("0 扇区");
  return tr("{0} 扇区 · {1}", { 0: s.toLocaleString(), 1: formatBytes(s * 512) });
}

/** The stripe diagram covers the levels with a chunk layout to draw. */
export function showDiagram(arr: RaidArray): boolean {
  return hasStripeLayout(arr.level);
}

/** Compact per-slot column labels for the diagram (slot index → disk name). */
export function diskLabelsFor(arr: RaidArray, imagePaths: ReadonlyMap<string, string>): (string | null)[] {
  const labels: (string | null)[] = new Array(arr.raid_disks).fill(null);
  for (const m of orderedMembers(arr)) {
    if (m.slot == null || m.slot < 0 || m.slot >= arr.raid_disks) continue;
    labels[m.slot] = memberShortLabel(m, imagePaths);
  }
  return labels;
}

/** Slots whose data is not read from their disk: rebuilt members, and — once
 *  the array is assembled — slots with no current member. */
export function missingSlotsFor(arr: RaidArray): number[] {
  const slots = new Set<number>();
  for (const m of arr.members) if (m.reconstructed && m.slot != null) slots.add(m.slot);
  if (arr.assembled_image_id) {
    const read = new Set(arr.members.filter(m => m.slot != null && m.state === 'active' && !m.reconstructed).map(m => m.slot));
    for (let slot = 0; slot < arr.raid_disks; slot++) if (!read.has(slot)) slots.add(slot);
  }
  return [...slots].sort((a, b) => a - b);
}

export function chunkLabelFor(arr: RaidArray): string {
  return `${arr.chunksize_sectors / 2}K`;
}

export function vgSummary(vg: RaidVolumeGroup, tr: Translator = sourceText): string {
  const lvs = joinList(vg.logical_volumes.map(lv => `${lv.name} ${formatBytes(lv.size_bytes)}`), tr);
  return lvs ? `${vg.members.length} PV · LV ${lvs}` : `${vg.members.length} PV`;
}

function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

export function originTitle(origin: RaidMemberOrigin, imagePaths: ReadonlyMap<string, string>, tr: Translator = sourceText): string {
  if (origin.kind === 'lv') return tr("逻辑卷 {0}/{1}（VG {2}）", { 0: origin.vg_name, 1: origin.lv_name, 2: origin.vg_uuid });
  const path = imagePaths.get(origin.image_id) ?? origin.image_id;
  return tr("{0} · 分区 {1} · 偏移 0x{2}", { 0: path, 1: origin.partition_id, 2: origin.byte_offset.toString(16).toUpperCase() });
}

/** Suggested RAW filename for a reconstructed array: array name (or a short
 *  set-uuid fallback) + RAID level, with Windows-illegal chars stripped. */
export function exportBaseName(arr: RaidArray): string {
  const raw = arr.set_name || `md-${arr.set_uuid.slice(0, 12)}`;
  const safe = raw.replace(/[\\/:*?"<>|]/g, '_').replace(/\s+/g, '_');
  return `${safe || 'raid'}_${arr.level === -1 ? 'linear' : `RAID${arr.level}`}`;
}
