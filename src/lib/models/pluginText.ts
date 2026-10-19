import { sourceText, type Translator } from '../../i18n/translate';
import type { PluginLocales } from '../ipc/plugins';

/** Keys of a plugin package's locale catalog (`locales/<locale>.toml`). */
export const pluginTextKey = {
  plugin: () => 'plugin.name',
  analyzer: (analyzer: string) => `analyzer.${analyzer}`,
  category: (category: string) => `category.${category}`,
  option: (analyzer: string, option: string) => `option.${analyzer}.${option}`,
  choice: (analyzer: string, option: string, choice: string) => `option.${analyzer}.${option}.${choice}`,
  record: (recordType: string) => `record.${recordType}`,
  field: (recordType: string, field: string) => `field.${recordType}.${field}`,
  fieldChoice: (recordType: string, field: string, choice: string) => `choice.${recordType}.${field}.${choice}`,
};

/** A label a plugin declared, in `locale`: the package's translation when it
 *  ships one, else the default it declared (Chinese). Plugin text never goes
 *  through the application catalog. */
export function pluginText(locales: PluginLocales | undefined, locale: string, key: string, fallback: string): string {
  if (!locales) return fallback;
  return locales[locale]?.[key] ?? locales[locale.split('-')[0]]?.[key] ?? fallback;
}

/** Looks up plugin labels by plugin id; plugins no longer installed keep their stored labels. */
export type PluginTextLookup = (pluginId: string, key: string, fallback: string) => string;

/** The identity lookup: every label as the plugin declared it. */
export const declaredPluginText: PluginTextLookup = (_pluginId, _key, fallback) => fallback;

/** Result categories many plugins share, named by the application. */
function sharedCategoryLabel(category: string, tr: Translator): string | null {
  switch (category) {
    case 'baseinfo': return tr("基本信息");
    case 'system': return tr("系统记录");
    case 'application': return tr("应用程序");
    case 'browserinfo': return tr("浏览器");
    case 'loginfo': return tr("日志");
    case 'databaseinfo': return tr("数据库");
    case 'fileanalyzer': return tr("文件分析");
    case 'filesystem': return tr("文件系统");
    case 'analysis': return tr("数据分析");
    case 'longtime': return tr("耗时任务");
    default: return null;
  }
}

/** A result category's name: the application's for the shared categories,
 *  else the plugin's (`category.<id>` in its locale catalog), else the id. */
export function categoryLabel(
  category: string,
  pluginId: string,
  text: PluginTextLookup = declaredPluginText,
  tr: Translator = sourceText,
): string {
  return sharedCategoryLabel(category, tr) ?? text(pluginId, pluginTextKey.category(category), category);
}
