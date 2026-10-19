// 插件任务调度 IPC。Typed wrappers over the `sched_*` backend commands.
// Interfaces mirror the Rust serde structs in `core::extensions::task_scheduler`
// (camelCase on the wire). 插件任务与内置任务一起由任务中心列出
// (`taskList` / `task-entry`,见 `taskRuntime.ts`)。
//
// 生命周期:created → queued → running → success | failed | cancel;offline 为
// 可恢复的中间态。任务不能暂停:`pause` 只出现在旧案件的任务行里,这样的
// 任务可以开始(从头再跑)或取消。任务行持久化在案件台账 `case.db` 的
// `task` 表,重开案件后仍可见;进程内只保留运行句柄。时间戳是 RFC 3339 UTC。
import { invokeCmd } from './client';
import type { AnalyzerScope, AnalyzerTrigger } from './plugins';

/** Rust `SchedTaskStatus`(serde lowercase)。 */
export type SchedTaskStatus =
  | 'created'
  | 'queued'
  | 'running'
  | 'success'
  | 'failed'
  | 'cancel'
  | 'pause'
  | 'offline';

/** Rust `TaskTrigger`(serde snake_case)。 */
export type SchedTaskTrigger = AnalyzerTrigger;

/** 任务的目标与选项。同一 (function, 检材, 目标) 是同一个作业。 */
export interface SchedTaskParams {
  /** 文件作用域:分析的文件节点。 */
  nodeId: string | null;
  fileName: string | null;
  /** 系统作用域:识别出的系统(`linux@part:2`)。 */
  systemId: string | null;
  /** 目标的展示名(系统名或文件名)。 */
  targetLabel: string | null;
  /** 分析器选项,按选项 id。 */
  options: Record<string, unknown>;
}

export interface SchedTaskInfo {
  pluginId: string;
  analyzer: string;
  imageId: string;
  trigger: SchedTaskTrigger;
  parentTid: number | null;
  error: string | null;
}

export interface SchedTaskResult {
  summary: string | null;
  recordCount: number;
  /** 进入时间线的时间点数。 */
  timelineCount: number;
  /** 运行留下的告警数(运行审计里可查看)。 */
  warningCount: number;
  /** 读不出、以零填充的源字节数。 */
  syntheticBytes: number;
  runId: string | null;
}

export interface SchedTaskRecord {
  id: number;
  /** function 名 `<plugin id>/<analyzer>`。 */
  name: string;
  displayName: string;
  typeName: string;
  /** 台账检材主键。 */
  evidenceId: number;
  /** 0–100。 */
  progress: number;
  params: SchedTaskParams;
  result: SchedTaskResult | null;
  dependence: string;
  status: SchedTaskStatus;
  info: SchedTaskInfo;
  canPause: boolean;
  singleInstance: boolean;
  internal: boolean;
  spendTimeSecs: number | null;
  remainTimeSecs: number | null;
  startAt: string | null;
  createAt: string;
  updateAt: string;
}

/** Rust `OptionKind`:多选(空 = 全部)、单选、开关、文本。 */
export type SchedOptionKind = 'multi' | 'choice' | 'flag' | 'text';

export interface SchedFunctionChoice {
  id: string;
  label: string;
}

/** 运行前可选的一个分析器选项。 */
export interface SchedFunctionOption {
  id: string;
  kind: SchedOptionKind;
  label: string;
  choices: SchedFunctionChoice[];
  /** 默认值:multi 为 id 数组,choice / text 为字符串,flag 为布尔值。 */
  default: unknown;
}

export interface SchedFunction {
  /** `<plugin id>/<analyzer>`。 */
  function: string;
  pluginId: string;
  pluginName: string;
  pluginVersion: string;
  analyzer: string;
  displayName: string;
  /** 结果树分类(`baseinfo`、`vehicle`……)。 */
  typeName: string;
  scope: AnalyzerScope;
  /** 系统作用域适用的操作系统族;空 = 全部。 */
  platforms: string[];
  trigger: SchedTaskTrigger;
  internal: boolean;
  singleInstance: boolean;
  /** 成功运行发布的画像事实。 */
  provides: string[];
  /** 自动运行等待的画像事实。 */
  requires: string[];
  /** 文件作用域接受的扩展名与 MIME。 */
  extensions: string[];
  mimeTypes: string[];
  /** 系统 / 检材作用域声明的输入路径模式。 */
  inputs: string[];
  options: SchedFunctionOption[];
}

/** 识别出的系统挂载的一个卷。 */
export interface SystemMount {
  /** 视图里的位置:`""` 为根,`home`、`boot/efi`、`D:`。 */
  path: string;
  /** 卷键。 */
  root: string;
  node: string;
  nodePath: string;
  /** `root`、`fstab` 或 `drive_letter`。 */
  source: string;
  listed: boolean;
}

/** 检材上识别出的操作系统:系统作用域分析的目标。 */
export interface DetectedSystem {
  /** 检材内稳定:`linux@part:2`、`linux@part:3/@`。 */
  id: string;
  /** `linux`、`windows`、`macos`…… */
  family: string;
  product: string;
  version: string | null;
  /** `Kali GNU/Linux 2024.1 · part:2`。 */
  label: string;
  /** 系统卷的卷键。 */
  root: string;
  mounts: SystemMount[];
}

/** Rust `SchedNoticeKind`(serde camelCase)。 */
export type SchedNoticeKind = 'taskOffline' | 'taskTimeout' | 'allTasksFinished' | 'postActionFailed';

export interface SchedNotice {
  kind: SchedNoticeKind;
  taskId: number | null;
  evidenceId: number | null;
  message: string;
}

export interface SchedTaskCreateResponse {
  task: SchedTaskRecord;
  /** false:同一作业已有未结束的任务,返回的是它。 */
  created: boolean;
}

export function schedTaskFunctions(): Promise<SchedFunction[]> {
  return invokeCmd<SchedFunction[]>('sched_task_functions');
}

/** 已加载检材上识别出的操作系统(首次调用读取检材识别)。 */
export function schedSystemsList(imageId: string): Promise<DetectedSystem[]> {
  return invokeCmd<DetectedSystem[]>('sched_systems_list', { imageId });
}

/** 建任务:系统作用域给 `systemId`,文件作用域给 `nodeId`(与 `fileName`),
 *  `options` 按分析器的选项 id;缺省的选项取默认值。 */
export function schedTaskCreate(args: {
  function: string;
  imageId: string;
  nodeId?: string | null;
  fileName?: string | null;
  systemId?: string | null;
  options?: Record<string, unknown> | null;
}): Promise<SchedTaskCreateResponse> {
  return invokeCmd<SchedTaskCreateResponse>('sched_task_create', {
    function: args.function,
    imageId: args.imageId,
    nodeId: args.nodeId ?? null,
    fileName: args.fileName ?? null,
    systemId: args.systemId ?? null,
    options: args.options ?? null,
  });
}

export function schedTaskCancel(taskId: number): Promise<SchedTaskRecord> {
  return invokeCmd<SchedTaskRecord>('sched_task_cancel', { taskId });
}

/** 开始离线任务或旧案件里暂停的任务(从头再跑)。 */
export function schedTaskStart(taskId: number): Promise<SchedTaskRecord> {
  return invokeCmd<SchedTaskRecord>('sched_task_start', { taskId });
}

export function schedTaskRestart(taskId: number): Promise<SchedTaskRecord> {
  return invokeCmd<SchedTaskRecord>('sched_task_restart', { taskId });
}

export function schedTaskRemove(taskId: number): Promise<SchedTaskRecord> {
  return invokeCmd<SchedTaskRecord>('sched_task_remove', { taskId });
}

export function schedEvidencePostActionsSet(args: {
  imageId: string;
  actionsJson: string | null;
}): Promise<void> {
  return invokeCmd<void>('sched_evidence_post_actions_set', args);
}
