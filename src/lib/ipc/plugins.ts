// 插件包 IPC:列出、重启共享实例、检视与安装(.lcpkg)、卸载、读取说明。安装
// 分两步:先 `pluginInspect` 读出插件包是什么,再带上它报告的 SHA-256 调
// `pluginInstall`。分析器不直接调用,一律经任务调度(`tasks.ts`)运行。
import { invokeCmd } from './client';
import type { RecordColumn, RecordPropertyLayout } from './analysisResults';

export type PluginPermission = 'file_materialize' | 'network';
/** 共享实例(文件系统浏览)的状态。 */
export type PluginRuntimeState = 'configured' | 'starting' | 'running' | 'crashed';
/** Rust `AnalyzerScope`:一次运行看的是一个识别出的系统、整个检材,还是选中的文件。 */
export type AnalyzerScope = 'system' | 'evidence' | 'files';
/** Rust `TaskTrigger`:检材上线或事实发布时自动建任务,或只由用户发起。 */
export type AnalyzerTrigger = 'auto' | 'manually';

export interface PluginAnalyzerInfo {
  id: string;
  name: string;
  scope: AnalyzerScope;
  /** 系统作用域适用的操作系统族;空 = 全部。 */
  platforms: string[];
  category: string;
  trigger: AnalyzerTrigger;
  provides: string[];
  requires: string[];
}

export interface PluginRecordTypeInfo {
  recordType: string;
  label: string;
  fields: RecordColumn[];
  properties?: RecordPropertyLayout | null;
}

/** 语言包:语言 → 键 → 译文(键如 `analyzer.users.name`、`record.linux.user`)。 */
export type PluginLocales = Record<string, Record<string, string>>;

export interface PluginPackageInfo {
  id: string;
  name: string;
  version: string;
  description: string | null;
  publisher: string | null;
  license: string | null;
  /** 插件包带 README.md(`pluginReadme`)。 */
  hasReadme: boolean;
  directory: string;
  /** 当前平台的库文件。 */
  library: string;
  libraryPresent: boolean;
  /** 用户安装(可卸载),不是随程序附带。 */
  removable: boolean;
  memoryLimit: number;
  fingerprint: string;
  state: PluginRuntimeState;
  error: string | null;
  permissions: PluginPermission[];
  /** 构建库所用的 SDK 版本(来自 schema.json)。 */
  sdk: string | null;
  schemaHash: string | null;
  analyzers: PluginAnalyzerInfo[];
  filesystems: string[];
  records: PluginRecordTypeInfo[];
  locales: PluginLocales;
}

/** 加载失败的插件包与原因。 */
export interface RejectedPluginPackage {
  directory: string;
  error: string;
}

export interface PluginListResponse {
  packages: PluginPackageInfo[];
  rejected: RejectedPluginPackage[];
  /** 用户安装的插件包所在目录。 */
  installDirectory: string | null;
}

export interface InstalledPluginPackage {
  pluginId: string;
  version: string;
  /** 被替换的旧版本。 */
  replaced: string | null;
}

/** Rust `PackageChange`:插件包相对已安装版本的变化。 */
export type PackageChange = 'new' | 'upgrade' | 'downgrade' | 'reinstall' | 'rebuild';

/** Rust `PackageRejection::reason`:插件包不能检视或安装的原因。 */
export type PackageRejectionReason =
  | 'noInstallRoot'
  | 'invalidArchive'
  | 'invalidPackage'
  | 'noLibrary'
  | 'bundled'
  | 'changed'
  | 'libraryRejected'
  | 'inUse';

export interface InspectedAnalyzer {
  id: string;
  name: string;
  scope: AnalyzerScope;
}

/** 检视出的插件包为何不能安装。 */
export interface InstallBlock {
  reason: PackageRejectionReason;
  message: string;
}

/** 插件包将安装什么(只读出,不运行其中任何代码)。 */
export interface PackageInspection {
  archive: string;
  /** 插件包文件的 SHA-256;安装时要求同一份字节。 */
  sha256: string;
  size: number;
  pluginId: string;
  name: string;
  version: string;
  description: string | null;
  publisher: string | null;
  license: string | null;
  permissions: PluginPermission[];
  memoryLimit: number;
  analyzers: InspectedAnalyzer[];
  filesystems: string[];
  recordTypes: number;
  sdk: string | null;
  schemaHash: string | null;
  locales: PluginLocales;
  /** 当前平台的库文件(包内相对路径)。 */
  library: string;
  /** 库文件的 SHA-256;插件包没有当前平台的库时为 null。 */
  librarySha256: string | null;
  installed: { version: string; bundled: boolean; librarySha256: string | null } | null;
  change: PackageChange;
  blocked: InstallBlock | null;
}

/** 插件包的文件扩展名。 */
export const PLUGIN_PACKAGE_EXTENSION = 'lcpkg';

export function pluginList(): Promise<PluginListResponse> {
  return invokeCmd<PluginListResponse>('plugin_list');
}

export function pluginRestart(pluginId: string): Promise<void> {
  return invokeCmd<void>('plugin_restart', { pluginId });
}

/** 检视 `.lcpkg` 插件包:标识、声明、摘要与已安装版本的关系。 */
export function pluginInspect(archive: string): Promise<PackageInspection> {
  return invokeCmd<PackageInspection>('plugin_inspect', { archive });
}

/**
 * 安装检视过的 `.lcpkg`(`sha256` 取检视结果);库先在插件进程中通过启动校验,
 * 才替换已安装的同 ID 版本。内置插件不能替换。
 */
export function pluginInstall(archive: string, sha256: string): Promise<InstalledPluginPackage> {
  return invokeCmd<InstalledPluginPackage>('plugin_install', { archive, sha256 });
}

/** 插件包的 README.md;没有时为 null。 */
export function pluginReadme(pluginId: string): Promise<string | null> {
  return invokeCmd<string | null>('plugin_readme', { pluginId });
}

/** 卸载用户安装的插件包;已发布的分析结果保留在案件里。 */
export function pluginUninstall(pluginId: string): Promise<void> {
  return invokeCmd<void>('plugin_uninstall', { pluginId });
}
