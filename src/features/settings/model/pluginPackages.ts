// Labels for plugin packages and the plugin market: what a failure was, how
// an inspected package relates to the installed one, a market entry's state
// and a declared permission. Pure; translated through the given `tr`.
import { sourceText, type Translator } from '../../../i18n/translate';
import type { PackageChange, PackageRejectionReason, PluginPermission } from '../../../lib/ipc/plugins';
import type { MarketErrorReason, MarketStatus } from '../../../lib/ipc/pluginMarket';

/** A failure as the page states it: what went wrong, and the backend's own words. */
export interface PluginFailure {
  title: string;
  detail: string;
}

type FailureReason = PackageRejectionReason | MarketErrorReason;

function reasonTitle(reason: FailureReason, tr: Translator): string {
  switch (reason) {
    case 'noInstallRoot': return tr("没有插件安装目录");
    case 'invalidArchive': return tr("插件包文件无效");
    case 'invalidPackage': return tr("插件包内容无效");
    case 'noLibrary': return tr("插件包没有当前平台的库文件");
    case 'bundled': return tr("内置插件不能替换");
    case 'changed': return tr("插件包在检视后被修改");
    case 'libraryRejected': return tr("插件库未通过启动校验");
    case 'inUse': return tr("已安装的版本正在使用");
    case 'invalidSource': return tr("索引源无效");
    case 'unknownSource': return tr("索引源未配置");
    case 'fetchFailed': return tr("读取索引失败");
    case 'invalidIndex': return tr("索引格式无效");
    case 'unknownPackage': return tr("索引中没有该插件");
    case 'unknownVersion': return tr("索引中没有该版本");
    case 'unavailable': return tr("没有当前平台的插件包");
    case 'downloadFailed': return tr("下载失败");
    case 'integrity': return tr("下载的插件包与索引不符");
    case 'mismatch': return tr("下载的插件包不是索引列出的版本");
  }
}

const REASONS = new Set<string>([
  'noInstallRoot', 'invalidArchive', 'invalidPackage', 'noLibrary', 'bundled', 'changed', 'libraryRejected', 'inUse',
  'invalidSource', 'unknownSource', 'fetchFailed', 'invalidIndex', 'unknownPackage', 'unknownVersion', 'unavailable',
  'downloadFailed', 'integrity', 'mismatch',
]);

/** The `reason` a backend failure (`IpcCommandError.context`) carries, when
 *  it is one the page names. */
export function failureReason(cause: unknown): FailureReason | null {
  if (!cause || typeof cause !== 'object') return null;
  const context = (cause as { context?: unknown }).context;
  if (!context || typeof context !== 'object') return null;
  const reason = (context as { reason?: unknown }).reason;
  return typeof reason === 'string' && REASONS.has(reason) ? reason as FailureReason : null;
}

/** A failure of `action`: the reason's own title when the backend named one. */
export function pluginFailure(cause: unknown, action: string, tr: Translator = sourceText): PluginFailure {
  const reason = failureReason(cause);
  return { title: reason ? reasonTitle(reason, tr) : action, detail: String(cause) };
}

/** A blocked inspection's title. */
export function blockTitle(reason: PackageRejectionReason, tr: Translator = sourceText): string {
  return reasonTitle(reason, tr);
}

export function changeLabel(change: PackageChange, installed: string | null, tr: Translator = sourceText): string {
  switch (change) {
    case 'new': return tr("新安装");
    case 'upgrade': return tr("升级自 {0}", { 0: installed ?? '—' });
    case 'downgrade': return tr("降级自 {0}", { 0: installed ?? '—' });
    case 'reinstall': return tr("重新安装");
    case 'rebuild': return tr("同版本号的不同构建");
  }
}

/** Changes that deserve a second look before they replace what is installed. */
export function changeIsWarning(change: PackageChange): boolean {
  return change === 'downgrade' || change === 'rebuild';
}

export function marketStatusLabel(status: MarketStatus, tr: Translator = sourceText): string {
  switch (status) {
    case 'notInstalled': return tr("未安装");
    case 'installed': return tr("已安装");
    case 'updateAvailable': return tr("可更新");
    case 'installedNewer': return tr("已安装更新版本");
    case 'bundled': return tr("内置");
    case 'unavailable': return tr("当前平台不可用");
  }
}

export function permissionLabel(permission: PluginPermission, tr: Translator = sourceText): string {
  switch (permission) {
    case 'file_materialize': return tr("临时物化文件");
    case 'network': return tr("网络访问");
  }
}
