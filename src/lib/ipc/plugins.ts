import { invokeCmd } from './client';
import { onEvent, type UnlistenFn } from './events';

export type PluginScanStatus = 'added' | 'unchanged' | 'removed' | 'updatePending' | 'error' | 'duplicate' | 'missingRoot';
export interface PluginScanEntry {
  directory: string;
  pluginId: string | null;
  version: string | null;
  status: PluginScanStatus;
  message: string;
}
export interface PluginScanReport {
  generation: number;
  roots: string[];
  entries: PluginScanEntry[];
}

export function pluginRescan(): Promise<PluginScanReport> {
  return invokeCmd<PluginScanReport>('plugin_rescan');
}

export function pluginScanReport(): Promise<PluginScanReport> {
  return invokeCmd<PluginScanReport>('plugin_scan_report');
}

export function onPluginCatalogChanged(handler: (generation: number) => void): Promise<UnlistenFn> {
  return onEvent<number>('plugin-catalog-changed', ({ payload }) => handler(payload));
}

export type PluginPermission = 'file_materialize' | 'emit_artifacts' | 'network';
export type PluginRuntimeState = 'configured' | 'starting' | 'running' | 'crashed';
/** `process`: inside lc-plugin-host; `inProcess`: loaded into LovelyCarve. */
export type PluginIsolation = 'process' | 'inProcess';

export interface PluginCapabilityInfo {
  kind: 'filesystem' | 'fileAnalyzer' | 'evidenceAnalyzer';
  name: string;
  priority: number;
  extensions: string[];
  mimeTypes: string[];
}

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  directory: string;
  /** Plugin library file (DLL / .so / .dylib). */
  library: string;
  isolation: PluginIsolation;
  fingerprint: string;
  state: PluginRuntimeState;
  error: string | null;
  permissions: PluginPermission[];
  capabilities: PluginCapabilityInfo[];
}

export interface PluginAnalysisArtifact {
  artifactType: string;
  title: string;
  value: unknown | null;
  attributes: Record<string, unknown>;
}

export interface PluginAnalysisTimelineEvent {
  timestamp: string;
  eventType: string;
  description: string;
  attributes: Record<string, unknown>;
}

export interface PluginAnalysisFinding {
  severity: 'info' | 'low' | 'medium' | 'high' | 'critical';
  title: string;
  description: string;
  attributes: Record<string, unknown>;
}

export interface PluginAnalysisResult {
  summary: string | null;
  artifacts: PluginAnalysisArtifact[];
  timeline: PluginAnalysisTimelineEvent[];
  findings: PluginAnalysisFinding[];
}

export interface PluginAnalysisRun {
  runId: string | null;
  persisted: boolean;
  pluginId: string;
  pluginVersion: string;
  analyzer: string;
  imageId: string;
  nodeId: string;
  fileName: string;
  startedAt: string;
  completedAt: string;
  result: PluginAnalysisResult;
}

export type StoredPluginAnalysisRun = Omit<PluginAnalysisRun, 'persisted'> & {
  runId: string;
};

export function pluginList(): Promise<PluginInfo[]> {
  return invokeCmd<PluginInfo[]>('plugin_list');
}

export function pluginRestart(pluginId: string): Promise<void> {
  return invokeCmd<void>('plugin_restart', { pluginId });
}

export function pluginAnalyzeFile(
  pluginId: string,
  analyzer: string,
  imageId: string,
  nodeId: string,
  fileName: string,
): Promise<PluginAnalysisRun> {
  return invokeCmd<PluginAnalysisRun>('plugin_analyze_file', {
    pluginId,
    analyzer,
    imageId,
    nodeId,
    fileName,
  });
}

export function pluginAnalysisList(
  imageId: string,
  nodeId: string,
): Promise<StoredPluginAnalysisRun[]> {
  return invokeCmd<StoredPluginAnalysisRun[]>('plugin_analysis_list', { imageId, nodeId });
}
