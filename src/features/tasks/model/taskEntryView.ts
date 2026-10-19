// 任务中心条目的展示:内置任务的标题、对象与状态说明按任务种类从后端报告的
// 计数、阶段与标签生成;插件任务的名称与结果由任务面板按插件译文与运行结果展示。
import { joinList, sourceText, type Translator } from '../../../i18n/translate';
import type { EvidencePlatform } from '../../../lib/ipc/autodetect';
import type {
  BackendTaskKind,
  BuiltinTaskEntry,
  PluginTaskEntry,
  TaskAction,
  TaskCount,
  TaskEntry,
  TaskView,
} from '../../../lib/ipc/taskRuntime';
import type { SchedTaskRecord } from '../../../lib/ipc/tasks';
import { formatBytes } from '../../../utils/format';
import { evidenceTypeLabel, platformLabel } from '../../evidence/model/autodetectLabels';
import { parseTaskTime, type TaskResultPanel } from './taskCenterView';
import { canCancelStatus, canRestartStatus, canStartStatus, isTerminalStatus } from './taskStatus';

/** The title of a native task kind the task center lists. */
export function builtinTaskTitle(kind: BackendTaskKind, tr: Translator = sourceText): string {
  switch (kind) {
    case 'imageLoad': return tr("检材加载");
    case 'imageExport':
    case 'bitlockerExport': return tr("镜像导出");
    case 'imageHash': return tr("镜像哈希");
    case 'encryptedVolumeUnlock': return tr("加密卷解锁");
    case 'autoDetect': return tr("自动识别平台");
    case 'volumeSnapshot':
    case 'sessionSnapshot': return tr("文件系统加载");
    case 'globalSearch': return tr("全局搜索");
    case 'timelineBuild': return tr("时间线重建");
    case 'carveScan': return tr("文件恢复");
    case 'carveRecovery': return tr("导出恢复文件");
    case 'directoryExport': return tr("目录导出");
    case 'dockerPrepare': return tr("Docker 分析");
    // Kinds the task center does not list keep their wire name.
    default: return kind;
  }
}

/** A volume key as the file tree names it. */
function volumeLabel(volumeKey: string, tr: Translator, label?: string): string {
  const partition = /^part:(\d+)$/.exec(volumeKey)?.[1];
  if (partition !== undefined) return tr("分区 {0}{1}", { 0: partition, 1: label ? ` · ${label}` : '' });
  return volumeKey === 'root' ? tr("根目录") : volumeKey;
}

const SCHEME_LABEL: Record<string, string> = { luks: 'LUKS', veracrypt: 'VeraCrypt', filevault: 'FileVault 2' };

/** What a native task works on. */
export function builtinSubject(entry: BuiltinTaskEntry, tr: Translator = sourceText): string {
  const { subject, labels } = entry.view;
  switch (entry.kind) {
    case 'volumeSnapshot':
    case 'sessionSnapshot':
      return subject[0] ? volumeLabel(subject[0], tr) : '';
    case 'encryptedVolumeUnlock': {
      const target = subject[0] ? volumeLabel(subject[0], tr, subject[1]) : tr("整个镜像 / 容器");
      const scheme = labels.scheme ? SCHEME_LABEL[labels.scheme] ?? labels.scheme : '';
      return [scheme, target].filter(Boolean).join(' · ');
    }
    case 'globalSearch':
    case 'imageHash':
      return joinList(subject, tr);
    default:
      return subject.join(' · ');
  }
}

/** 平台(含厂商与备份工具) · 检材类型。 */
function detectionConclusion(view: TaskView, tr: Translator): string {
  const platform = view.labels.platform
    ? view.labels.platformName || platformLabel(view.labels.platform as EvidencePlatform, tr)
    : tr("未识别到平台");
  const evidenceType = view.flags.includes('backup') ? tr("备份") : evidenceTypeLabel(view.labels.evidenceType ?? '', tr);
  return `${platform} · ${evidenceType}`;
}

function openingStage(view: TaskView, tr: Translator): string {
  switch (view.stage) {
    case 'decompressing': return tr("解压 {0}", { 0: view.labels.item ?? '' });
    case 'detectingPartitions': return tr("检测分区");
    case 'detectingFilesystems': return tr("检测文件系统");
    default: return tr("打开镜像");
  }
}

/** The line under a native task's name: how far it got, or what it produced. */
export function builtinMessage(entry: BuiltinTaskEntry, tr: Translator = sourceText): string {
  const { view, status } = entry;
  const count = (key: TaskCount) => view.counts[key] ?? 0;
  const number = (key: TaskCount) => count(key).toLocaleString();
  const active = status === 'created' || status === 'queued' || status === 'running' || status === 'pause';
  const amount = entry.progress;
  const done = amount?.done ?? 0;
  const total = amount?.total ?? 0;
  const bytes = amount?.unit === 'bytes' ? (total ? `${formatBytes(done)} / ${formatBytes(total)}` : formatBytes(done)) : '';
  const faults = count('syntheticBytes')
    ? tr(" · 警告：含 {0} 补零字节（{1} 个坏区）", { 0: formatBytes(count('syntheticBytes')), 1: count('badRegions') })
    : '';
  const truncated = view.flags.includes('truncated');
  switch (entry.kind) {
    case 'imageLoad':
      if (status === 'success') return tr("检材已就绪");
      if (status === 'cancel') return tr("加载已取消");
      if (status === 'queued') return tr("等待加载");
      return active ? [openingStage(view, tr), bytes].filter(Boolean).join(' · ') : '';
    case 'imageExport':
    case 'bitlockerExport':
      if (status === 'success') return tr("已导出 {0}{1}", { 0: formatBytes(done), 1: faults });
      if (status === 'cancel') return tr("导出已取消（保留部分文件）");
      return active ? bytes : '';
    case 'imageHash':
      if (status === 'success') return tr("哈希已完成{0}", { 0: faults });
      return active ? bytes : '';
    case 'encryptedVolumeUnlock':
      if (status === 'success') return tr("已解锁 · {0}", { 0: view.labels.cipher ?? '' });
      if (status === 'cancel') return tr("已取消解锁");
      return active ? tr("正在派生密钥并解锁；取消将在当前派生阶段结束后生效") : '';
    case 'autoDetect':
      if (status === 'success') return detectionConclusion(view, tr);
      if (status === 'queued') return tr("等待识别");
      return active ? tr("正在按文件系统类型与标志文件判定平台") : '';
    case 'volumeSnapshot':
    case 'sessionSnapshot': {
      const limit = truncated ? tr(" · 已达条目上限，结果不完整") : '';
      if (status === 'success') {
        return view.flags.includes('reused')
          ? tr("已复用现有索引 · {0} 项", { 0: number('entries') })
          : `${tr("索引已就绪 · {0} 项", { 0: number('entries') })}${limit}`;
      }
      if (status === 'queued') return tr("等待加载");
      if (!active) return '';
      if (view.stage === 'index') return tr("已写入 {0} 项，正在建立索引", { 0: number('entries') });
      return count('entries') ? tr("已读取 {0} 项{1}", { 0: number('entries'), 1: limit }) : tr("正在读取文件系统");
    }
    case 'globalSearch':
      if (active) return tr("已扫描 {0} / {1} 个文件 · {2} 处命中", { 0: done.toLocaleString(), 1: total.toLocaleString(), 2: number('hits') });
      return tr("{0} 个命中文件 · {1} 处命中", { 0: number('matchedFiles'), 1: number('hits') });
    case 'timelineBuild':
      if (active) {
        return tr("已处理 {0}{1} 项 · {2} 条时间线", { 0: done.toLocaleString(), 1: total > 0 ? ` / ${total.toLocaleString()}` : '', 2: number('rows') });
      }
      return tr("文件 {0} 条 · 事件 {1} 条", { 0: number('fileRows'), 1: number('eventRows') });
    case 'carveScan': {
      const limited = truncated ? tr(" · 已达结果上限") : '';
      if (view.stage === 'filesystem') {
        return active
          ? tr("正在扫描文件系统 · 已检查 {0} 项", { 0: done.toLocaleString() })
          : tr("发现 {0} 个文件{1}", { 0: number('found'), 1: limited });
      }
      return tr("已扫描 {0} / {1} · 发现 {2} 个文件{3}", { 0: formatBytes(done), 1: formatBytes(total), 2: number('found'), 3: limited });
    }
    case 'dockerPrepare':
      if (status === 'success') {
        return view.flags.includes('persisted')
          ? tr("索引已保存 · {0} 项，下次直接打开", { 0: number('entries') })
          : tr("容器已就绪 · {0} 项（当前会话）", { 0: number('entries') });
      }
      return active ? tr("正在合并容器文件系统…") : '';
    case 'directoryExport':
      if (status === 'success') return tr("已导出 {0} 个文件{1}", { 0: number('files'), 1: faults });
      if (status === 'cancel') return tr("导出已取消（保留部分文件）");
      return active ? tr("已导出 {0} 个文件 · {1}", { 0: number('files'), 1: formatBytes(done) }) : '';
    case 'carveRecovery':
      if (status === 'success') return tr("已恢复 {0} 个文件{1}", { 0: number('files'), 1: faults });
      return active ? bytes : '';
    default:
      return '';
  }
}

/** 0–100; a success is complete, interrupted work keeps its last share. */
export function taskEntryPercent(entry: Pick<TaskEntry, 'status' | 'progress'>): number {
  if (entry.status === 'success') return 100;
  const amount = entry.progress;
  if (!amount) return 0;
  const share = amount.unit === 'percent' ? amount.done : amount.total ? amount.done / amount.total * 100 : 0;
  return Number.isFinite(share) ? Math.max(0, Math.min(100, Math.round(share))) : 0;
}

/** Running work whose amount is not known yet. */
export function taskEntryIndeterminate(entry: Pick<TaskEntry, 'status' | 'progress'>): boolean {
  const amount = entry.progress;
  return entry.status === 'running' && !(amount && (amount.unit === 'percent' || (amount.total ?? 0) > 0));
}

/** Seconds a task ran: the scheduler's figure for plugin jobs, wall clock otherwise. */
export function taskEntryElapsedSecs(entry: TaskEntry, now: number): number | null {
  if (entry.source === 'plugin') return entry.job.spendTimeSecs;
  const started = entry.startedAtMs ?? entry.createdAtMs;
  return Math.max(0, ((entry.finishedAtMs ?? now) - started) / 1000);
}

const BUILTIN_PANEL: Partial<Record<BackendTaskKind, TaskResultPanel>> = {
  globalSearch: 'search-results', timelineBuild: 'timeline', carveScan: 'carve', carveRecovery: 'carve',
  dockerPrepare: 'files', volumeSnapshot: 'files', sessionSnapshot: 'files',
  autoDetect: 'dashboard', encryptedVolumeUnlock: 'dashboard', imageHash: 'dashboard',
};

/** Native work leads back to its page; a plugin job once its run published records or warnings. */
export function taskEntryResultPanel(entry: TaskEntry): TaskResultPanel | null {
  if (entry.source === 'builtin') return BUILTIN_PANEL[entry.kind] ?? null;
  const published = (entry.job.result?.recordCount ?? 0) + (entry.job.result?.warningCount ?? 0);
  return entry.status === 'success' && published > 0 ? 'analysis' : null;
}

function wallClock(value: string | null): number | null {
  const parsed = parseTaskTime(value);
  return Number.isFinite(parsed) ? parsed : null;
}

/** A plugin job as the task center lists it (mirrors the backend's
 *  `task_center::job_entry`), for a command's reply before its event. */
export function pluginTaskEntry(job: SchedTaskRecord): PluginTaskEntry {
  const actions: TaskAction[] = [];
  if (canCancelStatus(job.status)) actions.push('cancel');
  if (canStartStatus(job.status)) actions.push('start');
  if (canRestartStatus(job.status)) actions.push('restart');
  if (isTerminalStatus(job.status)) actions.push('remove');
  return {
    id: `plugin:${job.id}`,
    imageId: job.info.imageId,
    status: job.status,
    cancelling: false,
    automatic: job.info.trigger === 'auto',
    createdAtMs: wallClock(job.createAt) ?? 0,
    startedAtMs: wallClock(job.startAt),
    finishedAtMs: isTerminalStatus(job.status) ? wallClock(job.updateAt) : null,
    progress: { unit: 'percent', done: Math.max(0, Math.min(100, job.progress)), total: 100 },
    error: job.info.error,
    actions,
    source: 'plugin',
    job,
  };
}
