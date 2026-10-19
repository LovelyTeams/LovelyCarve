import { describe, expect, it } from 'vitest';
import type { RecordColumn } from '../../../lib/ipc/analysisResults';
import { compatiblePropertyLayout, resolvePropertyLayout } from './analysisPresentation';

const field = (id: string, hidden = false): RecordColumn => ({ id, label: id, kind: 'text', hidden, key: false, timeline: null, choices: [] });
const columns = [field('name'), field('key'), field('value'), field('extra', true)];
const layout = { label: 'name', secondary: 'key', value: 'value' };

describe('analysis presentation roles', () => {
  it('resolves declared roles without depending on a plugin id or field naming convention', () => {
    expect(resolvePropertyLayout(layout, columns)).toEqual({ label: columns[0], secondary: columns[1], value: columns[2] });
    expect(resolvePropertyLayout({ value: 'timezone' }, [field('timezone')])).toEqual({ label: null, secondary: null, value: field('timezone') });
  });

  it('rejects layouts which would hide, duplicate, or misidentify visible values', () => {
    for (const invalid of [null, { ...layout, value: 'missing' }, { ...layout, value: 'extra' }, { ...layout, value: 'name' }, { value: 'value' }]) {
      expect(resolvePropertyLayout(invalid, columns)).toBeNull();
    }
  });

  it('accepts updated display metadata for older runs only if every visible id and kind still matches', () => {
    expect(compatiblePropertyLayout(columns, columns.toReversed(), layout)).toEqual(layout);
    expect(compatiblePropertyLayout(columns, [...columns, field('new')], layout)).toBeNull();
    expect(compatiblePropertyLayout(columns, columns.map(column => column.id === 'value' ? { ...column, kind: 'int' } : column), layout)).toBeNull();
  });
});
