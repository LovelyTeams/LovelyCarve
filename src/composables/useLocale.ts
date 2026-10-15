import { readonly, ref, watch } from 'vue';
import { translate, type Locale, type MessageParams } from '../i18n/translate';

export type { Locale } from '../i18n/translate';
export const LOCALE_KEY = 'lovelycarve-locale';
export const DEFAULT_LOCALE: Locale = 'zh-CN';

function parseLocale(value: unknown): Locale {
  return value === 'en' ? 'en' : DEFAULT_LOCALE;
}

function storedLocale(): Locale {
  try { return parseLocale(localStorage.getItem(LOCALE_KEY)); }
  catch { return DEFAULT_LOCALE; }
}

const currentLocale = ref<Locale>(storedLocale());
let cleanup: (() => void) | null = null;

/** Use only for application messages, never evidence contents or plugin-provided data. */
export function tr(message: string, params?: MessageParams): string {
  return translate(message, currentLocale.value, params);
}

export function setLocale(locale: Locale): void {
  currentLocale.value = parseLocale(locale);
  try { localStorage.setItem(LOCALE_KEY, currentLocale.value); }
  catch { /* The preference still applies for the current window. */ }
}

/** Every webview applies and subscribes to the preference before mounting. */
export function initLocale(): () => void {
  if (cleanup) return cleanup;
  currentLocale.value = storedLocale();
  const stop = watch(currentLocale, locale => {
    document.documentElement.lang = locale;
  }, { immediate: true, flush: 'sync' });
  const onStorage = (event: StorageEvent) => {
    if (event.key !== LOCALE_KEY && event.key !== null) return;
    try { if (event.storageArea && event.storageArea !== localStorage) return; }
    catch { return; }
    currentLocale.value = parseLocale(event.newValue);
  };
  window.addEventListener('storage', onStorage);
  cleanup = () => {
    stop();
    window.removeEventListener('storage', onStorage);
    cleanup = null;
  };
  return cleanup;
}

export function useLocale() {
  return { locale: readonly(currentLocale), setLocale, tr };
}

/** Translate labels from application-owned option catalogs while retaining stable values. */
export function localizeOptions<T extends { label: string }>(options: readonly T[]): Array<Omit<T, 'label'> & { label: string }> {
  return options.map(option => ({ ...option, label: tr(option.label) }));
}
