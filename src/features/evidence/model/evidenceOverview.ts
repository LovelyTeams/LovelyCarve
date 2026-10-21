import { sourceText, type Translator } from '../../../i18n/translate';
import type { ImageInfo, DiskPartition, EwfHeaderDate } from '../../../lib/ipc/image';
import { formatBinaryBytes as formatBytes, formatEpochSeconds } from '../../../utils/format';
import { KIND_LABELS } from '../../../lib/models/filesystemLabels';
export { formatBytes, KIND_LABELS };

/** Shared evidence heading and workspace tab name; keep RAID session identities in details. */
export function evidenceDisplayName(info: Pick<ImageInfo, 'path' | 'source_format'>, tr: Translator = sourceText): string {
  if (info.source_format === 'assembled') {
    const automatic = /^assembled:\/\/md:auto-raid(\d+)-[a-f0-9]{32}$/i.exec(info.path);
    if (automatic) return tr("RAID {0} 重组镜像", { 0: automatic[1] });
    const logicalVolume = /^assembled:\/\/lvm:[^:]+:(.+)$/.exec(info.path);
    return logicalVolume ? tr("逻辑卷 {0}", { 0: logicalVolume[1] }) : tr("RAID 重组镜像");
  }
  return info.path.replace(/\\/g, '/').split('/').filter(Boolean).pop() ?? info.path;
}

/** 没有可浏览数据结构的分区（MSR 等未知类型、BIOS Boot、扩展分区容器），整行弱化。 */
export const QUIET_KINDS = new Set(['unknown', 'biosboot', 'extended']);

// 后端 TableScheme 序列化为全小写。
export const SCHEME_LABELS: Record<string, string> = {
  gpt: 'GPT',
  mbr: 'MBR (DOS)',
  firmware: '小米 HDR1 固件',
  apm: 'APM',
  bsd: 'BSD Disklabel',
  luks: 'LUKS 容器',
  mdadm: 'MDADM RAID',
  lvm2: 'LVM2 物理卷',
  volume: '整盘卷',
};


export function partitionTitle(p: { id: number; kind: string; start_lba: number; label: string | null }, tr: Translator = sourceText): string {
  if (p.label) return p.label;
  // 与后端 fs_tree.rs::partition_name 一致：只留序号。类型在徽章、
  // 几何在起始扇区列，都不进标题。
  return tr("分区 {0}", { 0: p.id });
}

/** 起始扇区的悬停提示：完整范围、长度与字节偏移。 */
export function partitionGeometry(p: DiskPartition, sectorSize: number, unit: string, tr: Translator = sourceText): string {
  const end = p.start_lba + p.length_lba - 1;
  const offset = (p.start_lba * sectorSize).toString(16).toUpperCase();
  return tr("{0} {1} – {2}，共 {3} {4}\n字节偏移 0x{5}", { 0: unit, 1: p.start_lba.toLocaleString(), 2: end.toLocaleString(), 3: p.length_lba.toLocaleString(), 4: unit, 5: offset });
}


// 分类色取 themes.css 的 --chart-1..6：三个主题各自定义了一套
// 与主题氛围匹配的色板（亮色饱和调 / 暗色亮调 / 粉色玫瑰系）。
const PART_COLORS = [
  'var(--chart-1)',
  'var(--chart-2)',
  'var(--chart-3)',
  'var(--chart-4)',
  'var(--chart-5)',
  'var(--chart-6)',
];

export function partColor(index: number): string {
  return PART_COLORS[index % PART_COLORS.length];
}

export interface LayoutSegment {
  key: string;
  leftPct: number;
  widthPct: number;
  lenBytes: number;
  color: string;
  tooltip: string;
  partition: DiskPartition;
}


export function evidenceLayout(info: ImageInfo | null, tr: Translator = sourceText): LayoutSegment[] {
  if (!info || info.size_bytes <= 0 || info.partitions.length === 0) return [];
  const total = info.size_bytes;
  const sorted = info.partitions
    .map((p, index) => ({ p, index }))
    .sort((a, b) => a.p.start_lba - b.p.start_lba);
  const segments: LayoutSegment[] = [];
  for (const { p, index } of sorted) {
    const startBytes = Math.min(p.start_lba * info.sector_size, total);
    const lenBytes = Math.min(p.length_lba * info.sector_size, total - startBytes);
    if (lenBytes <= 0) continue;
    segments.push({
      key: `part-${p.id}`,
      leftPct: (startBytes / total) * 100,
      widthPct: (lenBytes / total) * 100,
      lenBytes,
      color: partColor(index),
      tooltip: `${partitionTitle(p, tr)} · ${KIND_LABELS[p.kind] || p.kind} · ${formatBytes(lenBytes)}`,
      partition: p,
    });
  }
  return segments;
}


export interface InfoRow {
  label: string;
  value: string;
  mono?: boolean;
  copyable?: boolean;
  /** Stable source label used to group rows before localization. */
  acquisitionKey?: string;
}

// 头部之外的检材事实：精确字节数、扇区大小、磁盘标识；卷影副本会话再加来源。
// 镜像 ID 与镜像文件的创建 / 修改时间是本机侧信息，不在此列。
export function evidenceFacts(info: ImageInfo | null, tr: Translator = sourceText): InfoRow[] {
  if (!info) return [];
  const rows: InfoRow[] = [
    { label: tr("逻辑大小"), value: tr("{0} 字节", { 0: info.size_bytes.toLocaleString() }), mono: true },
    { label: info.optical_disc || ['xiaomi_firmware', 'android_backup'].includes(info.source_format) ? tr("寻址单位") : tr("扇区大小"), value: `${info.sector_size} B`, mono: true },
  ];
  if (info.optical_disc) {
    rows.push({ label: tr("光盘会话数"), value: String(new Set(info.optical_disc.tracks.map(track => track.session)).size), mono: true });
    rows.push({ label: tr("光盘轨道数"), value: String(info.optical_disc.tracks.length), mono: true });
    rows.push({ label: tr("光盘数据文件数"), value: String(info.optical_disc.files.length), mono: true });
  }
  if (info.source_format === 'assembled') rows.push({ label: tr("重组镜像标识"), value: info.path, mono: true, copyable: true });
  if (info.disk_id) rows.push({ label: tr("磁盘标识"), value: info.disk_id, mono: true, copyable: true });
  const shadow = info.shadow_copy;
  if (shadow) {
    rows.push({ label: tr("卷影副本"), value: tr("分区 #{0} 的第 {1} 个（按创建时间）", { 0: shadow.partition_id, 1: shadow.index + 1 }) });
    const taken = formatEpochSeconds(shadow.created_unix_ms == null ? null : shadow.created_unix_ms / 1000, {
      timezone: 'utc', precision: 'seconds', suffix: true, fallback: '',
    });
    if (taken) rows.push({ label: tr("快照时间"), value: taken, mono: true });
    if (shadow.shadow_copy_id) rows.push({ label: tr("卷影副本 ID"), value: shadow.shadow_copy_id, mono: true, copyable: true });
  }
  return rows;
}

export function acquisitionRows(info: ImageInfo | null, tr: Translator = sourceText): InfoRow[] {
  const ewf = info?.ewf;
  if (!ewf) return [];
  const dateValue = (raw: string | null, parsed: EwfHeaderDate | null | undefined): string | null => {
    if (!raw || !parsed) return raw;
    return parsed.time_basis === 'local'
      ? tr('{0}（采集机本地时间，时区未记录）', { 0: parsed.display })
      : tr('{0}（Unix 时间，按 UTC 显示）', { 0: parsed.display });
  };
  const candidates: Array<[string, string | null, boolean?]> = [
    ['案件编号', ewf.case_number, true],
    ['证据编号', ewf.evidence_number, true],
    ['检验人', ewf.examiner_name],
    ['采集时间', dateValue(ewf.acquiry_date, ewf.acquiry_date_parsed), true],
    ['设备型号', ewf.model],
    ['序列号', ewf.serial_number, true],
    ['描述', ewf.description],
    ['备注', ewf.notes],
    ['采集格式', ewf.format],
    ['介质类型', ewf.media_type],
    ['压缩方式', ewf.compression],
    ['分段数', ewf.segment_count != null ? String(ewf.segment_count) : null, true],
    ['系统时间', dateValue(ewf.system_date, ewf.system_date_parsed), true],
    ['采集系统', ewf.acquiry_operating_system],
    ['采集软件版本', ewf.acquiry_software_version, true],
  ];
  // Always reachable in the disclosure, including malformed/unparsed headers.
  candidates.push(
    ['采集时间原始值', ewf.acquiry_date, true],
    ['系统时间原始值', ewf.system_date, true],
  );
  return candidates
    .filter((entry): entry is [string, string, boolean?] => !!entry[1])
    .map(([label, value, mono]) => ({ label: tr(label), value, mono, acquisitionKey: label }));
}

/** 直接显示的采集字段；其余放进「更多采集信息」。 */
export const ACQUISITION_HIGHLIGHTS = new Set(['案件编号', '证据编号', '检验人', '采集时间', '设备型号', '序列号']);

export type HashVerdict = 'match' | 'mismatch' | 'pending';
export interface StoredHashRow { label: string; value: string; verdict: HashVerdict; }

function normalizeDigest(value: string): string {
  return value.replace(/\s+/g, '').toLowerCase();
}

export function storedHashRows(info: ImageInfo | null, result: Partial<Record<'md5' | 'sha1', string | null>> | undefined): StoredHashRow[] {
  const ewf = info?.ewf;
  if (!ewf) return [];
  const stored: Array<[string, 'md5' | 'sha1', string | null]> = [
    ['MD5', 'md5', ewf.stored_md5],
    ['SHA1', 'sha1', ewf.stored_sha1],
  ];
  return stored
    .filter((entry): entry is [string, 'md5' | 'sha1', string] => !!entry[2])
    .map(([label, algo, value]) => {
      const computedDigest = result?.[algo];
      const verdict: HashVerdict = !computedDigest
        ? 'pending'
        : normalizeDigest(computedDigest) === normalizeDigest(value) ? 'match' : 'mismatch';
      return { label, value, verdict };
    });
}

export function evidenceHashVerdict(rows: StoredHashRow[]): Exclude<HashVerdict, 'pending'> | null {
  const judged = rows.filter(row => row.verdict !== 'pending');
  if (judged.length === 0) return null;
  return judged.every(row => row.verdict === 'match') ? 'match' : 'mismatch';
}
