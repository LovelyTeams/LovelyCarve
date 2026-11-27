// 插件市场 IPC:官方索引的插件清单、下载并检视一个版本、释放下载。市场只读取插件
// 发布页的官方索引,没有其他索引源。下载结果经 `pluginInstall`(plugins.ts)安装,
// 安装后或取消时调 `pluginMarketRelease`。
import { invokeCmd } from './client';
import type { PackageInspection, PluginLocales, PluginPermission } from './plugins';

/** Rust `MarketStatus`:清单中的插件相对已安装版本的状态。 */
export type MarketStatus =
  | 'notInstalled'
  | 'installed'
  | 'updateAvailable'
  | 'installedNewer'
  | 'bundled'
  | 'unavailable';

/** Rust `MarketError::reason`(以及插件包拒绝原因)。 */
export type MarketErrorReason =
  | 'invalidSource'
  | 'fetchFailed'
  | 'invalidIndex'
  | 'unknownPackage'
  | 'unknownVersion'
  | 'unavailable'
  | 'downloadFailed'
  | 'integrity'
  | 'mismatch';

export interface MarketVersion {
  version: string;
  /** RFC 3339。 */
  published: string;
  permissions: PluginPermission[];
  prerelease: boolean;
  /** 当前平台的插件包;该版本没有时为 null。 */
  size: number | null;
  sha256: string | null;
}

export interface MarketPackage {
  id: string;
  name: string;
  description: string | null;
  publisher: string | null;
  license: string | null;
  /** `plugin.name` / `plugin.description` 的译文。 */
  locales: PluginLocales;
  /** 安装时取的版本:当前平台可安装的最高正式版,没有时取最高预发布版。 */
  latest: string | null;
  installed: string | null;
  status: MarketStatus;
  /** 新版本在前。 */
  versions: MarketVersion[];
}

export interface MarketCatalog {
  /** 读取的索引(官方索引的地址)。 */
  source: string;
  title: string;
  description: string | null;
  fetchedAt: string;
  /** 本机平台(`<os>-<arch>`)。 */
  platform: string;
  packages: MarketPackage[];
}

export interface MarketDownload {
  source: string;
  archive: string;
  sha256: string;
  inspection: PackageInspection;
}

/** 官方索引的插件清单;`refresh` 重新读取索引。 */
export function pluginMarketList(refresh = false): Promise<MarketCatalog> {
  return invokeCmd<MarketCatalog>('plugin_market_list', { refresh });
}

/** 从官方索引下载当前平台的插件包,按索引校验大小与 SHA-256,并检视它。 */
export function pluginMarketDownload(pluginId: string, version: string): Promise<MarketDownload> {
  return invokeCmd<MarketDownload>('plugin_market_download', { pluginId, version });
}

/** 删除安装过或放弃的下载。 */
export function pluginMarketRelease(sha256: string): Promise<void> {
  return invokeCmd<void>('plugin_market_release', { sha256 });
}
