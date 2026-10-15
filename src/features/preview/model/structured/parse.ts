import { sourceText, type Translator } from '../../../../i18n/translate';
// 结构化文本（JSON / YAML / XML）→ 统一的有序树 PValue。
//
// 设计目标：一套 PValue 表示喂给 graph.ts / layout.ts，三种格式各自解析后归一。
// - JSON：原生 JSON.parse；.jsonc/.json5 先剥注释与尾逗号，失败再退回 YAML 宽松解析。
// - YAML：js-yaml（JSON 的近超集，能兜住不少 json5 写法）。
// - XML：浏览器原生 DOMParser（零依赖，正确处理命名空间 / 属性 / CDATA），
//        元素→object（属性记为 @name，子元素按标签分组，重复标签→array，文本→#text）。
//
// PValue 用「有序 entries」而非普通对象，是为了在节点图里稳定保留键的出现顺序。
import { load as yamlLoad } from 'js-yaml';

export type PFormat = 'json' | 'yaml' | 'xml';

/** 归一后的有序值树。primitive 直接带值，容器保留子项顺序。 */
export type PValue =
  | { t: 'object'; entries: [string, PValue][] }
  | { t: 'array'; items: PValue[] }
  | { t: 'string'; v: string }
  | { t: 'number'; v: number }
  | { t: 'boolean'; v: boolean }
  | { t: 'null' };

export interface ParseResult {
  value: PValue | null;
  format: PFormat;
  /** 解析失败时的可读原因；成功时为 undefined。 */
  error?: string;
}

/** 内容嗅探：无格式提示时按首个非空字符猜。'<'→xml，'{'/'['→json，否则 yaml。 */
export function detectFormat(text: string): PFormat {
  const s = text.trimStart();
  if (s.startsWith('<')) return 'xml';
  if (s.startsWith('{') || s.startsWith('[')) return 'json';
  return 'yaml';
}

export function parseStructured(text: string, formatHint?: PFormat, tr: Translator = sourceText): ParseResult {
  const format = formatHint ?? detectFormat(text);
  if (text.trim().length === 0) {
    return { value: null, format, error: 'empty' };
  }
  try {
    switch (format) {
      case 'json':
        return { value: parseJsonLike(text), format };
      case 'yaml':
        return { value: jsToPValue(yamlLoad(text)), format };
      case 'xml':
        return { value: parseXml(text, tr), format };
    }
  } catch (err) {
    return { value: null, format, error: errMessage(err) };
  }
}

// ---- JSON / JSONC / JSON5 ------------------------------------------------

function parseJsonLike(text: string): PValue {
  const stripped = stripJsonComments(text);
  try {
    return jsToPValue(JSON.parse(stripped));
  } catch {
    // json5 的单引号 / 裸键等 JSON.parse 啃不动——退回 YAML（JSON 的近超集）兜底。
    return jsToPValue(yamlLoad(stripped));
  }
}

/** 剥掉 // 行注释、块注释和尾逗号；字符串内的相同序列不动。 */
function stripJsonComments(src: string): string {
  let out = '';
  let inStr = false;
  let quote = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (inStr) {
      out += c;
      if (c === '\\') {
        out += next ?? '';
        i += 2;
        continue;
      }
      if (c === quote) inStr = false;
      i += 1;
      continue;
    }
    if (c === '"' || c === "'") {
      inStr = true;
      quote = c;
      out += c;
      i += 1;
      continue;
    }
    if (c === '/' && next === '/') {
      i += 2;
      while (i < src.length && src[i] !== '\n') i += 1;
      continue;
    }
    if (c === '/' && next === '*') {
      i += 2;
      while (i < src.length && !(src[i] === '*' && src[i + 1] === '/')) i += 1;
      i += 2;
      continue;
    }
    out += c;
    i += 1;
  }
  return out.replace(/,(\s*[\]}])/g, '$1');
}

// ---- XML -----------------------------------------------------------------

function parseXml(text: string, tr: Translator = sourceText): PValue {
  const doc = new DOMParser().parseFromString(text, 'application/xml');
  const perr = doc.querySelector('parsererror');
  if (perr) throw new Error(perr.textContent?.trim() || tr("XML 解析失败"));
  const root = doc.documentElement;
  if (!root) throw new Error(tr("XML 缺少根元素"));
  return { t: 'object', entries: [[root.nodeName, elementToPValue(root)]] };
}

function elementToPValue(el: Element): PValue {
  const entries: [string, PValue][] = [];

  for (const attr of Array.from(el.attributes)) {
    entries.push([`@${attr.name}`, { t: 'string', v: attr.value }]);
  }

  const textParts: string[] = [];
  for (const node of Array.from(el.childNodes)) {
    if (node.nodeType === 3 /* TEXT_NODE */ || node.nodeType === 4 /* CDATA */) {
      const t = (node.nodeValue ?? '').trim();
      if (t) textParts.push(t);
    }
  }

  const childEls = Array.from(el.children);
  // 纯叶子（无属性、无子元素）：值即文本。
  if (childEls.length === 0 && entries.length === 0) {
    return { t: 'string', v: textParts.join(' ') };
  }

  // 子元素按标签分组，保留首次出现顺序；同名多个 → array。
  const groups = new Map<string, Element[]>();
  for (const child of childEls) {
    const k = child.nodeName;
    const arr = groups.get(k);
    if (arr) arr.push(child);
    else groups.set(k, [child]);
  }
  for (const [tag, els] of groups) {
    entries.push([
      tag,
      els.length === 1 ? elementToPValue(els[0]) : { t: 'array', items: els.map(elementToPValue) },
    ]);
  }
  if (textParts.length) {
    entries.push(['#text', { t: 'string', v: textParts.join(' ') }]);
  }
  return { t: 'object', entries };
}

// ---- 原生 JS 值 → PValue --------------------------------------------------

function jsToPValue(v: unknown, ancestors: Set<object> = new Set()): PValue {
  if (v === null || v === undefined) return { t: 'null' };
  const ty = typeof v;
  if (ty === 'string') return { t: 'string', v: v as string };
  if (ty === 'number') return { t: 'number', v: v as number };
  if (ty === 'bigint') return { t: 'string', v: `${v as bigint}` };
  if (ty === 'boolean') return { t: 'boolean', v: v as boolean };
  if (Array.isArray(v)) {
    // YAML 锚点/别名可能造出循环引用——按「祖先路径」检测，命中即截断，
    // 既挡住无限递归又不误伤多处复用的同一子树（兄弟共享仍各自展开）。
    if (ancestors.has(v)) return { t: 'string', v: '[circular]' };
    ancestors.add(v);
    const items = v.map((x) => jsToPValue(x, ancestors));
    ancestors.delete(v);
    return { t: 'array', items };
  }
  if (ty === 'object') {
    // YAML 时间戳标签会产出 Date——转 ISO 字符串展示。
    if (v instanceof Date) return { t: 'string', v: v.toISOString() };
    const obj = v as object;
    if (ancestors.has(obj)) return { t: 'string', v: '[circular]' };
    ancestors.add(obj);
    const entries = Object.entries(v as Record<string, unknown>).map(
      ([k, val]) => [k, jsToPValue(val, ancestors)] as [string, PValue],
    );
    ancestors.delete(obj);
    return { t: 'object', entries };
  }
  return { t: 'string', v: String(v) };
}

function errMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
