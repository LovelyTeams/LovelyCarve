import { describe, expect, it } from 'vitest';
import english from './en.json';
import chinese from './zh-CN.json';
import { sourceText, translate } from './translate';

describe('message catalogs', () => {
  it('preserves interpolation parameters in every English translation', () => {
    const placeholders = (message: string) => [...message.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
    for (const [chinese, translated] of Object.entries(english)) {
      expect(placeholders(translated), chinese).toEqual(placeholders(chinese));
      expect(translated, chinese).not.toMatch(/\p{Script=Han}/u);
    }
  });

  it('distinguishes words that have different meanings in different contexts', () => {
    for (const key of Object.keys(chinese)) expect(Object.hasOwn(english, key), key).toBe(true);
    expect(translate('关闭', 'en')).toBe('Close');
    expect(translate('motion.off', 'en')).toBe('Off');
    expect(sourceText('motion.off')).toBe('关闭');
    expect(translate('日', 'en')).toBe('Sun');
    expect(translate('timeUnits.day', 'en')).toBe('Day');
  });

  it('interpolates once, leaving evidence text and unprovided parameters intact', () => {
    expect(translate('分区 {0}', 'en', { 0: '文件列表 {1}' })).toBe('Partition 文件列表 {1}');
    expect(sourceText('分区 {0}', { 0: 2 })).toBe('分区 2');
    expect(translate('分区 {0}', 'en')).toBe('Partition {0}');
    expect(translate('未登记的原始消息', 'en')).toBe('未登记的原始消息');
    expect(translate('__proto__', 'en')).toBe('__proto__');
    expect(translate('constructor', 'en')).toBe('constructor');
    expect(translate('分区 {0}', 'en', { 0: '<script>{0}</script>' })).toBe('Partition <script>{0}</script>');
  });
});
