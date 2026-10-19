// 分析结果浏览 IPC:结果树、分页表格、单条详情与运行审计。
// 节点名、列与字段类型都来自插件声明的记录结构,前端零硬编码;旧版本
// LovelyCarve 写下的结果(legacy)只读展示,没有字段类型与来源路径。
import { invokeCmd } from './client';
import type { BadRegion } from './fs/export';

export type AnalysisNodeKind = 'evidence' | 'class' | 'function' | 'result';

/** Rust `FieldKind`:值的显示、排序与检索方式。 */
export type RecordFieldKind =
  | 'text'
  | 'int'
  | 'float'
  | 'bool'
  | 'timestamp'
  | 'size'
  | 'duration'
  | 'path'
  | 'hash'
  | 'ip'
  | 'enum'
  | 'list'
  | 'json';

export interface RecordColumnChoice {
  id: string;
  label: string;
}

/** 记录类型的一列(插件声明的字段)。 */
export interface RecordColumn {
  id: string;
  label: string;
  kind: RecordFieldKind;
  /** 记录身份的一部分。 */
  key: boolean;
  /** 只在详情里显示,不占表格列。 */
  hidden: boolean;
  /** 时间戳字段进入时间线时的角色。 */
  timeline: string | null;
  choices: RecordColumnChoice[];
}

export interface AnalysisResultNode {
  nodeId: string;
  kind: AnalysisNodeKind;
  label: string;
  imageId: string;
  /** 结果分类(`baseinfo`、`vehicle`……)。 */
  typeName: string;
  pluginId: string;
  analyzer: string;
  /** `<plugin id>/<analyzer>`。 */
  function: string;
  /** 文件分析的输入文件节点。 */
  sourceNodeId: string | null;
  /** 运行的目标(系统名、文件名);整个检材为空。 */
  targetLabel: string;
  /** 结果项的记录类型。 */
  artifactType: string;
  /** 结果项与功能节点所属的已发布运行。 */
  runId: string;
  completedAt: string;
  childCount: number;
  /** 运行留下的告警数(功能节点)。 */
  warningCount: number;
  legacy: boolean;
  children: AnalysisResultNode[];
}

/** 记录的来源:卷内路径(`root` + `path`)或文件树节点,可带字节范围。 */
export interface RecordSource {
  /** 卷键(`part:2`)。 */
  root?: string;
  /** 卷内路径。 */
  path?: string;
  nodeId?: string;
  offset?: number;
  length?: number;
  /** 记录在来源里的位置(`第 12 行`)。 */
  note?: string;
}

export type RecordSeverity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export interface AnalysisRecord {
  recordId: string;
  runId: string;
  recordType: string;
  title: string;
  severity: RecordSeverity | null;
  /** 按列 id 的值。 */
  values: Record<string, unknown>;
  sources: RecordSource[];
  imageId: string;
  legacy: boolean;
}

/** Plugin-declared roles for one property per record; ids refer to visible columns. */
export interface RecordPropertyLayout {
  value: string;
  label?: string | null;
  secondary?: string | null;
}

export interface AnalysisRecordsPage {
  total: number;
  columns: RecordColumn[];
  properties?: RecordPropertyLayout | null;
  items: AnalysisRecord[];
}

export interface AnalysisRecordDetail {
  record: AnalysisRecord;
  columns: RecordColumn[];
}

export interface AnalysisRunTarget {
  kind: 'files' | 'system' | 'evidence';
  /** 系统 id、输入文件节点,整个检材为空。 */
  id: string;
  label: string;
}

/** 运行拿到的一个输入文件。 */
export interface AnalysisRunInput {
  /** 视图内路径(文件分析为文件名)。 */
  path: string;
  root: string | null;
  rootPath: string | null;
  nodeId: string | null;
  size: number;
}

export interface AnalysisRunWarning {
  code: string;
  message: string;
  path: string | null;
  nodeId: string | null;
}

export interface AnalysisRunRecordType {
  recordType: string;
  label: string;
  count: number;
}

/** 一次运行的审计:输入、选项、画像事实、告警与读不出而以零填充的源字节。 */
export interface AnalysisRunAudit {
  runId: string;
  state: 'published' | 'superseded';
  pluginId: string;
  pluginVersion: string;
  analyzer: string;
  analyzerName: string;
  target: AnalysisRunTarget;
  options: Record<string, unknown>;
  profile: Record<string, unknown>;
  inputs: AnalysisRunInput[];
  inputCount: number;
  schemaHash: string;
  summary: string | null;
  recordCount: number;
  recordTypes: AnalysisRunRecordType[];
  warningCount: number;
  warnings: AnalysisRunWarning[];
  badRegions: BadRegion[];
  syntheticBytes: number;
  startedAt: string;
  finishedAt: string | null;
  publishedAt: string | null;
}

/** 检材 → 分类 → 分析器运行 → 记录类型 整树(未开案件返回空)。 */
export function analysisResultTree(): Promise<AnalysisResultNode[]> {
  return invokeCmd<AnalysisResultNode[]>('analysis_result_tree');
}

/** 按选中结果项的 runId 服务端分页读取一种记录,翻页期间不切换运行。 */
export function analysisRecordsPage(args: {
  imageId: string;
  runId: string;
  recordType: string;
  legacy: boolean;
  keyword?: string;
  /** 列 id,或 `title`。 */
  sort?: string;
  descending?: boolean;
  skip: number;
  limit: number;
}): Promise<AnalysisRecordsPage> {
  return invokeCmd<AnalysisRecordsPage>('analysis_records_page', {
    imageId: args.imageId,
    runId: args.runId,
    recordType: args.recordType,
    legacy: args.legacy,
    keyword: args.keyword ?? null,
    sort: args.sort ?? null,
    descending: args.descending ?? false,
    skip: args.skip,
    limit: args.limit,
  });
}

/** 单条记录详情(表格行 → 右侧预览),带它所属类型的列。 */
export function analysisRecordDetail(args: {
  imageId: string;
  recordId: string;
}): Promise<AnalysisRecordDetail | null> {
  return invokeCmd<AnalysisRecordDetail | null>('analysis_record_detail', args);
}

/** 一次运行的审计(旧版结果没有审计,返回 null)。 */
export function analysisRunGet(args: {
  imageId: string;
  runId: string;
}): Promise<AnalysisRunAudit | null> {
  return invokeCmd<AnalysisRunAudit | null>('analysis_run_get', args);
}
