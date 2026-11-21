/** 随包分发的单色 SVG 图标 → 内联 `<svg>` 的 viewBox、根填充与内容。
 *
 *  这类图标按「透明度遮罩 + 统一着色」绘制：fill / stroke 写的颜色（black、#DCE0E5、
 *  #C6CAD0、white …）只表示这里要画，不代表颜色。转换时把它们换成 currentColor，
 *  图标随 CSS `color` 着色；`<mask>` 与 `<clipPath>` 里的颜色属于几何（亮度遮罩），
 *  原样保留。缺 viewBox 的按 width / height 补齐；id 与 `url(#…)` / `href="#…"` 引用
 *  加上前缀，同一文档里不同图标的 id 不会互相引用。
 *
 *  只处理随包的可信资源（经 `?raw` 打包），不做通用 SVG 清洗。 */

export interface InlineSvgIcon {
  viewBox: string;
  /** 根元素的 fill：多数图标为 none，未声明时按 SVG 默认（黑色）换成 currentColor。 */
  fill: string;
  /** 根元素的子内容，已换色、已加 id 前缀。 */
  body: string;
}

const ROOT = /^\s*(?:<\?xml[^>]*>\s*)?<svg\b([^>]*)>([\s\S]*)<\/svg>\s*$/;
const ATTRIBUTE = /([\w:.-]+)\s*=\s*"([^"]*)"/g;
/** 根元素上与绘制无关、或由 InlineSvgIcon 单独承载的属性。 */
const ROOT_ONLY = new Set(['xmlns', 'xmlns:xlink', 'xml:space', 'version', 'width', 'height', 'viewBox', 'fill']);
/** 颜色语义属于几何的块：亮度遮罩、裁剪路径。不允许嵌套同名块（随包资源均满足）。 */
const GEOMETRY_BLOCK = /<(mask|clipPath)\b[\s\S]*?<\/\1>/g;
const PAINT = /(\s)(fill|stroke)="([^"]*)"/g;
const STYLE = /(\s)style="([^"]*)"/g;
const STYLE_PAINT = /(^|;)(\s*)(fill|stroke)(\s*):([^;]*)/g;

function paint(value: string): string {
  const trimmed = value.trim();
  return trimmed === 'none' || trimmed === 'transparent' || trimmed.startsWith('url(') ? trimmed : 'currentColor';
}

function recolor(markup: string): string {
  return markup
    .replace(PAINT, (_, space: string, name: string, value: string) => `${space}${name}="${paint(value)}"`)
    .replace(STYLE, (_, space: string, style: string) => {
      const painted = style.replace(STYLE_PAINT, (_match, lead: string, gap: string, name: string, colon: string, value: string) =>
        `${lead}${gap}${name}${colon}:${paint(value)}`);
      return `${space}style="${painted}"`;
    });
}

function recolorOutsideGeometry(markup: string): string {
  let out = '';
  let last = 0;
  for (const block of markup.matchAll(GEOMETRY_BLOCK)) {
    const start = block.index ?? 0;
    out += recolor(markup.slice(last, start)) + block[0];
    last = start + block[0].length;
  }
  return out + recolor(markup.slice(last));
}

function prefixIds(markup: string, prefix: string): string {
  return markup
    .replace(/(\s)id="([^"]+)"/g, (_, space: string, id: string) => `${space}id="${prefix}${id}"`)
    .replace(/url\(#([^)\s]+)\)/g, (_, id: string) => `url(#${prefix}${id})`)
    .replace(/(\s)((?:xlink:)?href)="#([^"]+)"/g, (_, space: string, name: string, id: string) => `${space}${name}="#${prefix}${id}"`);
}

/** 把一个图标源文件转换成可内联的内容。源文件不是单个 `<svg>` 根元素时抛错。 */
export function inlineSvgIcon(source: string, idPrefix: string): InlineSvgIcon {
  const root = ROOT.exec(source);
  if (!root) throw new Error('SVG icon source must be a single <svg> element');
  const attributes = new Map<string, string>();
  for (const [, name, value] of root[1].matchAll(ATTRIBUTE)) attributes.set(name, value);

  const width = Number.parseFloat(attributes.get('width') ?? '16');
  const height = Number.parseFloat(attributes.get('height') ?? '16');
  const viewBox = attributes.get('viewBox') ?? `0 0 ${width} ${height}`;
  const fill = paint(attributes.get('fill') ?? 'black');

  let body = recolorOutsideGeometry(prefixIds(root[2].trim(), idPrefix));
  // 根元素上的其他绘制属性（stroke、stroke-width…）落到一层 <g> 上，让子元素照常继承。
  const inherited = [...attributes].filter(([name]) => !ROOT_ONLY.has(name));
  if (inherited.length > 0) {
    const group = inherited.map(([name, value]) => ` ${name}="${value}"`).join('');
    body = `${recolor(`<g${group}>`)}${body}</g>`;
  }
  return { viewBox, fill, body };
}
