import { describe, expect, it } from 'vitest';
import { FILE_ICON_NAMES } from '../../utils/fileIcons';
import { BUNDLED_FILE_ICONS, fileIconSvg } from './fileIconSvg';

const GEOMETRY_BLOCK = /<(mask|clipPath)\b[\s\S]*?<\/\1>/g;

describe('bundled file icons', () => {
  it('ships exactly the icons the lookup can produce', () => {
    expect([...BUNDLED_FILE_ICONS.keys()].sort()).toEqual([...FILE_ICON_NAMES].sort());
  });

  it('converts every icon into self-contained markup painted with currentColor', () => {
    for (const name of BUNDLED_FILE_ICONS.keys()) {
      const icon = fileIconSvg(name);
      const painted = icon.body.replace(GEOMETRY_BLOCK, '');
      // 16px 网格，个别图标（diff、roc、v）宽 17。
      expect({ name, viewBox: /^0 0 1[67] 16$/.test(icon.viewBox), fill: icon.fill }).toEqual({ name, viewBox: true, fill: 'none' });
      expect(icon.body, name).not.toMatch(/<(script|style|foreignObject|image|use|a)\b|\son[a-z]+=|javascript:|href="(?!#)/i);
      expect(painted.match(/\s(fill|stroke)="(?!none"|currentColor"|url\(#)[^"]*"/g), name).toBeNull();
      expect(painted.match(/(^|[;"])\s*(fill|stroke)\s*:\s*(?!currentColor|none)/g), name).toBeNull();
      expect(painted.match(/stop-color|gradient/gi), name).toBeNull();
      const ids = [...icon.body.matchAll(/\sid="([^"]+)"/g)].map(match => match[1]);
      expect(ids.filter(id => !id.startsWith(`lc-file-icon-${name}-`)), name).toEqual([]);
      const references = [...icon.body.matchAll(/url\(#([^)]+)\)/g)].map(match => match[1]);
      expect(references.filter(id => !ids.includes(id)), name).toEqual([]);
    }
  });

  it('falls back to the generic file icon for unknown names', () => {
    expect(fileIconSvg('no-such-icon')).toEqual(fileIconSvg('file'));
  });
});
