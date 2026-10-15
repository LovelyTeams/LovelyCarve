import { sourceText, type Translator } from '../../../i18n/translate';
// 时间线面板的纯展示逻辑:柱状图钻取时收窄时间范围并降低粒度,
// 历史范围压栈以支持撤销。
//
// 关键约定：桶标签（date_str）是后端 strftime 按 `time_zone_hours` 偏移
// 聚合出来的「显示时区」字符串；把桶标签换算回 unix 秒区间时必须再减去
// 同一时区偏移，与明细表 formatEpoch 的加偏移互逆。

/** 粒度梯子：年/月/日/时/分/秒，默认 year。 */
export const TIMELINE_UNITS = ['year', 'month', 'day', 'hour', 'minute', 'second'] as const;

export type TimelineUnitValue = (typeof TIMELINE_UNITS)[number];

export const UNIT_LABELS: Record<TimelineUnitValue, string> = {
  year: '年',
  month: '月',
  day: '日',
  hour: '时',
  minute: '分',
  second: '秒',
};

/** 钻取降档（点柱子：范围收窄到该桶 + 粒度降一档；秒已到底）。 */
export function drillDown(unit: TimelineUnitValue): TimelineUnitValue {
  const index = TIMELINE_UNITS.indexOf(unit);
  return TIMELINE_UNITS[Math.min(index + 1, TIMELINE_UNITS.length - 1)];
}

/** 放大反向（回到上一档）。 */
export function zoomUp(unit: TimelineUnitValue): TimelineUnitValue {
  const index = TIMELINE_UNITS.indexOf(unit);
  return TIMELINE_UNITS[Math.max(index - 1, 0)];
}

/** 浏览器本地时区偏移小时数（东八区 = 8；可能与聚合参数一致使用）。 */
export function localTimeZoneHours(): number {
  return -new Date().getTimezoneOffset() / 60;
}

/** 解析 strftime 桶标签：`2024` / `2024-05` / `2024-05-12` /
 *  `2024-05-12 08` / `2024-05-12 08:30` / `2024-05-12 08:30:59`（零填充定长）。 */
function parseBucketParts(dateStr: string): {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
} {
  const [datePart, timePart] = dateStr.split(' ');
  const dateFields = datePart.split('-');
  while (dateFields.length < 3) dateFields.push('01');
  const timeFields = (timePart ?? '').split(':').filter((piece) => piece !== '');
  while (timeFields.length < 3) timeFields.push('00');
  return {
    year: Number(dateFields[0]),
    month: Number(dateFields[1]),
    day: Number(dateFields[2]),
    hour: Number(timeFields[0]),
    minute: Number(timeFields[1]),
    second: Number(timeFields[2]),
  };
}

function partsToEpoch(
  parts: ReturnType<typeof parseBucketParts>,
  tzHours: number,
): number {
  const epochMs = Date.UTC(
    parts.year,
    parts.month - 1,
    parts.day,
    parts.hour,
    parts.minute,
    parts.second,
  );
  // 桶标签是显示时区表示 → 换算回 UTC epoch 需减偏移。
  return Math.round(epochMs / 1000 - tzHours * 3600);
}

/** 一个桶的闭区间 unix 秒 `[start, end]`（end = 下一桶起点 - 1，供查询
 *  参数 start/end 直接使用）。 */
export function bucketRange(
  dateStr: string,
  unit: TimelineUnitValue,
  tzHours: number,
): { start: number; end: number } {
  const startParts = parseBucketParts(dateStr);
  const start = partsToEpoch(startParts, tzHours);
  const bumped = { ...startParts };
  switch (unit) {
    case 'year':
      bumped.year += 1;
      break;
    case 'month':
      bumped.month += 1;
      break;
    case 'day':
      bumped.day += 1;
      break;
    case 'hour':
      bumped.hour += 1;
      break;
    case 'minute':
      bumped.minute += 1;
      break;
    case 'second':
      bumped.second += 1;
      break;
  }
  // Date.UTC 自动进位（月 13 → 次年 1，秒 60 → 下一分）。
  return { start, end: partsToEpoch(bumped, tzHours) - 1 };
}

/** 明细表「时间」列：按聚合时区把 unix 秒格式化为 `YYYY-MM-DD HH:MM:SS`。 */
export function formatEpoch(epoch: number, tzHours: number): string {
  const shifted = new Date((epoch + tzHours * 3600) * 1000);
  const pad = (value: number) => String(value).padStart(2, '0');
  return (
    `${shifted.getUTCFullYear()}-${pad(shifted.getUTCMonth() + 1)}-${pad(shifted.getUTCDate())} ` +
    `${pad(shifted.getUTCHours())}:${pad(shifted.getUTCMinutes())}:${pad(shifted.getUTCSeconds())}`
  );
}

/** 柱状图轴标签：最右一段有意义的分量（tooltip 仍显示完整 date_str）。 */
export function bucketShortLabel(dateStr: string, unit: TimelineUnitValue, tr: Translator = sourceText): string {
  switch (unit) {
    case 'year':
      return dateStr;
    case 'month':
      return dateStr.slice(5);
    case 'day':
      return dateStr.slice(8);
    case 'hour':
      return tr("{0}时", { 0: dateStr.slice(11) });
    case 'minute':
      return dateStr.slice(14);
    case 'second':
      return dateStr.slice(17);
  }
}

/** 供人读的时间范围文本（钻取提示/撤销按钮 title）。 */
export function formatRange(start: number, end: number, tzHours: number): string {
  return `${formatEpoch(start, tzHours)} ~ ${formatEpoch(end, tzHours)}`;
}

/** 状态中文标签。 */
export function statusLabel(status: string, tr: Translator = sourceText): string {
  switch (status) {
    case 'buildRequired':
      return tr("未创建");
    case 'running':
      return tr("构建中");
    case 'stale':
      return tr("有新数据未处理");
    case 'ready':
      return tr("已就绪");
    default:
      return status;
  }
}

/** datatype 编号 → 展示名（后端注册表镜像：1=文件 2=插件时间线）。 */
export function dataTypeLabel(dataType: number, tr: Translator = sourceText): string {
  switch (dataType) {
    case 1:
      return tr("文件");
    case 2:
      return tr("插件时间线");
    case 3:
      return tr("NTFS 变更日志");
    default:
      return tr("类型 {0}", { 0: dataType });
  }
}

export function flagTone(flag: string): string {
  if (!flag) return 'var(--text-secondary)';
  if (flag.includes('删除')) return 'var(--sev-critical-fg)';
  if (flag.includes('创建')) return 'var(--chart-2)';
  if (flag.includes('重命名')) return 'var(--chart-4)';
  if (flag.includes('属性') || flag.includes('权限')) return 'var(--chart-6)';
  if (flag.includes('访问')) return 'var(--chart-1)';
  if (flag.includes('修改') || flag.includes('内容')) return 'var(--chart-3)';
  return 'var(--text-secondary)';
}
