// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { effectScope, ref } from 'vue';
import { useHexPreview } from './useHexPreview';

const scopes: ReturnType<typeof effectScope>[] = [];
afterEach(() => { for (const scope of scopes.splice(0)) scope.stop(); });

function setup() {
  const scope = effectScope();
  scopes.push(scope);
  const hex = scope.run(() => useHexPreview({ bytes: ref(new Uint8Array(64)), page: ref(1), pageSize: ref(64), searchQuery: ref('') }))!;
  return hex;
}

/** A byte cell in the hex column, or a character cell in a text column. */
function cell(offset: number, charset?: string): HTMLElement {
  const column = document.createElement('span');
  column.className = charset ? 'hex-text-col' : 'hex-bytes';
  if (charset) column.dataset.cs = charset;
  const element = document.createElement('span');
  element.dataset.off = String(offset);
  column.append(element);
  return element;
}

function event(type: string, target: Element, init: MouseEventInit = {}): MouseEvent {
  const value = new MouseEvent(type, { button: type === 'contextmenu' ? 2 : 0, cancelable: true, ...init });
  Object.defineProperty(value, 'target', { value: target });
  return value;
}

describe('hex selection', () => {
  it('extends the selection to a Shift-pressed byte', () => {
    const hex = setup();
    hex.onHexMouseDown(event('mousedown', cell(4)));
    hex.onHexDragEnd();
    hex.onHexMouseDown(event('mousedown', cell(9), { shiftKey: true }));
    hex.onHexDragEnd();
    expect([hex.selMin.value, hex.selMax.value, hex.selCount.value]).toEqual([4, 9, 6]);
  });

  it('keeps the selection under a right press inside it and selects the byte outside it', () => {
    const hex = setup();
    hex.onHexMouseDown(event('mousedown', cell(4)));
    hex.onHexMouseDown(event('mousedown', cell(9), { shiftKey: true }));
    hex.onHexDragEnd();
    expect(hex.prepareHexContextMenu(event('contextmenu', cell(6)))).toEqual({ charset: null });
    expect([hex.selMin.value, hex.selMax.value]).toEqual([4, 9]);
    expect(hex.prepareHexContextMenu(event('contextmenu', cell(20, 'utf-8')))?.charset?.id).toBe('utf-8');
    expect([hex.selMin.value, hex.selMax.value]).toEqual([20, 20]);
  });

  it('offers no menu when nothing is selected and no byte is under the pointer', () => {
    const hex = setup();
    expect(hex.prepareHexContextMenu(event('contextmenu', document.createElement('div')))).toBeNull();
    expect(hex.hasSelection.value).toBe(false);
  });
});
