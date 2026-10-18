// 分析结果浏览 IPC:结果树、分页表格与单条详情。
// 节点名与列定义由插件提交,前端零硬编码。
import { invokeCmd } from './client';

export type AnalysisNodeKind = 'evidence' | 'class' | 'function' | 'result';

export interface AnalysisNodeColumn {
  key: string;
  label: string;
}

export interface AnalysisResultNode {
  nodeId: string;
  kind: AnalysisNodeKind;
  label: string;
  imageId: string;
  typeName: string;
  analyzer: string;
  function: string;
  sourceNodeId: string | null;
  artifactType: string;
  runId: string;
  completedAt: string;
  childCount: number;
  columns: AnalysisNodeColumn[];
  children: AnalysisResultNode[];
}

export interface AnalysisRecord {
  artifactId: string;
  runId: string;
  artifactType: string;
  title: string;
  value: unknown | null;
  attributes: Record<string, unknown>;
  /** Persisted run ownership and input file; older hosts may omit these. */
  imageId?: string;
  sourceNodeId?: string | null;
}

export interface AnalysisRecordsPage {
  total: number;
  items: AnalysisRecord[];
}

/** 检材 → 分类 → 功能 → 结果项 整树(未开案件返回空)。 */
export function analysisResultTree(): Promise<AnalysisResultNode[]> {
  return invokeCmd<AnalysisResultNode[]>('analysis_result_tree');
}

/** 按选中结果项的 runId 服务端分页读取记录。 */
export function analysisRecordsPage(args: {
  imageId: string;
  analyzer: string;
  runId: string;
  artifactType: string;
  keyword?: string;
  skip: number;
  limit: number;
}): Promise<AnalysisRecordsPage> {
  return invokeCmd<AnalysisRecordsPage>('analysis_records_page', {
    imageId: args.imageId,
    analyzer: args.analyzer,
    runId: args.runId,
    artifactType: args.artifactType,
    keyword: args.keyword ?? null,
    skip: args.skip,
    limit: args.limit,
  });
}

/** 单条记录详情(表格行 → 右侧预览)。结果按检材存放,带上记录所属检材。 */
export function analysisRecordDetail(args: {
  imageId: string;
  artifactId: string;
}): Promise<AnalysisRecord | null> {
  return invokeCmd<AnalysisRecord | null>('analysis_record_detail', args);
}
