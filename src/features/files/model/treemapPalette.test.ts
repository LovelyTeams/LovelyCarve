import { describe, expect, it } from 'vitest';
import { ageSwatch, cssColor, mixColor, parseColor } from './treemapPalette';

describe('treemap palette', () => {
  it('reads the color notations the theme tokens use', () => {
    expect(parseColor('#3b82f6')).toEqual({ r: 59, g: 130, b: 246, a: 1 });
    expect(parseColor('#fff')).toEqual({ r: 255, g: 255, b: 255, a: 1 });
    expect(parseColor('#00000080')).toEqual({ r: 0, g: 0, b: 0, a: 128 / 255 });
    expect(parseColor('rgba(239, 68, 68, 0.10)')).toEqual({ r: 239, g: 68, b: 68, a: 0.1 });
    expect(parseColor('rgb(1 2 3 / 0.5)')).toEqual({ r: 1, g: 2, b: 3, a: 0.5 });
    expect(parseColor('color-mix(in srgb, red, blue)')).toBeNull();
  });

  it('mixes like color-mix in sRGB', () => {
    const blue = { r: 0, g: 0, b: 255, a: 1 };
    const white = { r: 255, g: 255, b: 255, a: 1 };
    expect(mixColor(blue, white, 0.2)).toEqual({ r: 204, g: 204, b: 255, a: 1 });
    expect(cssColor(mixColor(blue, white, 0.5))).toBe('rgba(128, 128, 255, 1.000)');
    // A transparent end contributes no color, only opacity.
    expect(cssColor(mixColor(blue, { r: 0, g: 0, b: 0, a: 0 }, 0.4))).toBe('rgba(0, 0, 255, 0.400)');
  });

  it('gives legend swatches the same tokens the canvas paints with', () => {
    expect(ageSwatch(0)).toBe('color-mix(in srgb, var(--primary-color) 58%, var(--surface-secondary))');
    expect(ageSwatch(null)).toBe('color-mix(in srgb, var(--text-secondary) 10%, var(--surface-secondary))');
  });
});
