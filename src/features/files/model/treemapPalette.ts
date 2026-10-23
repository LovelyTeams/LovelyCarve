/** 树图配色：瓦片是同一种中性色，层级越深混入越多，嵌套不画边框也读得出；「时间」着色用主色，越久越淡。
 *  画布与图例共用这一份比例，画布把令牌解析成具体颜色后自行混合。 */

/** 各层瓦片混入中性色（`--text-secondary`）的比例（第 0 层到第 3 层，更深按第 3 层）。 */
export const DEPTH_MIX: readonly number[] = [0.2, 0.28, 0.36, 0.44];
/** 「时间」各档混入主色的比例，由新到旧。 */
export const AGE_MIX: readonly number[] = [0.58, 0.44, 0.31, 0.19, 0.09];
/** 时间未知的瓦片。 */
export const AGE_UNKNOWN_MIX = 0.1;

/** 图例色块：与画布同一套令牌，交给 CSS 混合。 */
const percent = (share: number) => `${Math.round(share * 100)}%`;

export function ageSwatch(bucket: number | null): string {
  if (bucket === null) return `color-mix(in srgb, var(--text-secondary) ${percent(AGE_UNKNOWN_MIX)}, var(--surface-secondary))`;
  return `color-mix(in srgb, var(--primary-color) ${percent(AGE_MIX[bucket] ?? AGE_MIX[AGE_MIX.length - 1])}, var(--surface-secondary))`;
}

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** 解析 `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa` 与 `rgb()` / `rgba()`；其它写法为 null。 */
export function parseColor(text: string): Rgba | null {
  const value = text.trim().toLowerCase();
  const hex = value.match(/^#([0-9a-f]{3,8})$/);
  if (hex) {
    const digits = hex[1];
    if (digits.length === 3 || digits.length === 4) {
      const [r, g, b, a = 'f'] = [...digits];
      return { r: parseInt(r + r, 16), g: parseInt(g + g, 16), b: parseInt(b + b, 16), a: parseInt(a + a, 16) / 255 };
    }
    if (digits.length === 6 || digits.length === 8) {
      return {
        r: parseInt(digits.slice(0, 2), 16),
        g: parseInt(digits.slice(2, 4), 16),
        b: parseInt(digits.slice(4, 6), 16),
        a: digits.length === 8 ? parseInt(digits.slice(6, 8), 16) / 255 : 1,
      };
    }
    return null;
  }
  const functional = value.match(/^rgba?\(([^)]+)\)$/);
  if (!functional) return null;
  const parts = functional[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  if (parts.length < 3 || parts.some(part => !Number.isFinite(part))) return null;
  return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
}

/** `color-mix(in srgb, from amount, to)`：按 sRGB 分量线性混合（带透明度时按预乘）。 */
export function mixColor(from: Rgba, to: Rgba, amount: number): Rgba {
  const weight = Math.min(Math.max(amount, 0), 1);
  const alpha = from.a * weight + to.a * (1 - weight);
  if (alpha <= 0) return { r: 0, g: 0, b: 0, a: 0 };
  const channel = (start: number, end: number) => (start * from.a * weight + end * to.a * (1 - weight)) / alpha;
  return { r: channel(from.r, to.r), g: channel(from.g, to.g), b: channel(from.b, to.b), a: alpha };
}

export function withAlpha(color: Rgba, alpha: number): Rgba {
  return { ...color, a: color.a * alpha };
}

export function cssColor(color: Rgba): string {
  const round = (value: number) => Math.round(Math.min(Math.max(value, 0), 255));
  return `rgba(${round(color.r)}, ${round(color.g)}, ${round(color.b)}, ${Math.min(Math.max(color.a, 0), 1).toFixed(3)})`;
}
