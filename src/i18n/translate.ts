import english from './en.json';
import chinese from './zh-CN.json';

export type Locale = 'zh-CN' | 'en';
export type MessageParams = Readonly<Record<string, unknown>>;
export type Translator = (message: string, params?: MessageParams) => string;

/** Default translator for pure presentation models and their tests. */
export const sourceText: Translator = (message, params) => translate(message, 'zh-CN', params);

/** Source messages are the Chinese catalog. Interpolation never evaluates HTML or code. */
export function translate(message: string, locale: Locale, params: MessageParams = {}): string {
  const catalog: Readonly<Record<string, string>> = locale === 'en' ? english : chinese;
  const translated = Object.prototype.hasOwnProperty.call(catalog, message)
    ? catalog[message] : message;
  return translated.replace(/\{(\w+)\}/g, (placeholder, key: string) =>
    Object.prototype.hasOwnProperty.call(params, key) ? String(params[key]) : placeholder);
}
