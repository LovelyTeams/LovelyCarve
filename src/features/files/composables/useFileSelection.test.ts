// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { ref } from 'vue';
import { useFileSelection } from './useFileSelection';
import type { DiskFsNode } from '../model/model';

function node(id: string, kind: 'file' | 'dir' = 'file', rowKey = id): DiskFsNode {
  return { id, rowKey, name: id, kind, size: 10, deleted: false } as DiskFsNode;
}
function click(selection: ReturnType<typeof useFileSelection>, id: string, checked: boolean, shiftKey = false) {
  const input = document.createElement('input');
  input.type = 'checkbox'; input.checked = checked;
  const event = new MouseEvent('click', { shiftKey });
  Object.defineProperty(event, 'target', { value: input });
  selection.onCheckClick(event, id);
  return input;
}

describe('file selection identities and operation scope', () => {
  it('keeps a hard-link cursor distinct while counting/exporting its node once', () => {
    const first = node('file', 'file', 'left');
    const second = node('file', 'file', 'right');
    const selection = useFileSelection({ rows: () => [first, second], marqueeJustEnded: () => false });
    selection.selectedKey.value = second.rowKey!;
    selection.toggleAll();
    expect(selection.selectedKey.value).toBe('right');
    expect(selection.checkedFiles.value).toEqual([first]);
    expect(selection.checkedStats.value).toEqual({ count: 1, bytes: 10 });
    expect(selection.headerChecked.value).toBe(true);
  });
  it('excludes filtered-out nodes from operations and preserves their checks when toggling visible rows', () => {
    const rows = ref([node('hidden'), node('visible'), node('directory', 'dir')]);
    const selection = useFileSelection({ rows: () => rows.value, marqueeJustEnded: () => false });
    selection.toggleAll();
    rows.value = rows.value.slice(1);
    expect(selection.checkedFiles.value.map(item => item.id)).toEqual(['visible']);
    expect(selection.checkedDirs.value.map(item => item.id)).toEqual(['directory']);
    expect(selection.checkedStats.value).toEqual({ count: 2, bytes: 10 });
    selection.toggleAll();
    expect(selection.checkedIds.value).toEqual(new Set(['hidden']));
    expect(selection.headerIndeterminate.value).toBe(false);
    selection.toggleAll();
    expect(selection.checkedIds.value).toEqual(new Set(['hidden', 'visible', 'directory']));
  });
  it('resolves a Shift anchor in the current sorted/filtered projection', () => {
    const rows = ref(['a', 'b', 'c', 'd'].map(id => node(id)));
    const selection = useFileSelection({ rows: () => rows.value, marqueeJustEnded: () => false });
    click(selection, 'b', true);
    rows.value = ['d', 'b', 'c'].map(id => node(id));
    click(selection, 'd', true, true);
    expect(selection.checkedIds.value).toEqual(new Set(['b', 'd']));
    click(selection, 'c', false, true);
    expect(selection.checkedIds.value).toEqual(new Set());
  });
  it('falls back to the clicked row if a filtered-out anchor no longer exists', () => {
    const rows = ref([node('a'), node('b')]);
    const selection = useFileSelection({ rows: () => rows.value, marqueeJustEnded: () => false });
    click(selection, 'a', true);
    rows.value = [node('b'), node('c')];
    click(selection, 'c', true, true);
    expect(selection.checkedIds.value).toEqual(new Set(['a', 'c']));
  });
  it('flips one row with Ctrl+click and checks a range from that anchor with Shift+click', () => {
    const rows = ['a', 'b', 'c', 'd'].map(id => node(id));
    const selection = useFileSelection({ rows: () => rows, marqueeJustEnded: () => false });
    selection.toggleChecked('b');
    selection.checkRange('d', 'a');
    expect(selection.checkedIds.value).toEqual(new Set(['b', 'c', 'd']));
    selection.toggleChecked('c');
    expect(selection.checkedIds.value).toEqual(new Set(['b', 'd']));
  });
  it('starts a Shift range at the cursor row when nothing was checked by hand, keeping other checks', () => {
    const rows = ['a', 'b', 'c', 'd'].map(id => node(id));
    const selection = useFileSelection({ rows: () => rows, marqueeJustEnded: () => false });
    selection.checkedIds.value = new Set(['d']);
    selection.checkRange('c', 'a');
    expect(selection.checkedIds.value).toEqual(new Set(['a', 'b', 'c', 'd']));
    selection.clearChecked();
    selection.checkRange('b', 'missing');
    expect(selection.checkedIds.value).toEqual(new Set(['b']));
  });
  it('restores a native checkbox after a marquee ghost click without altering selection', () => {
    const selection = useFileSelection({ rows: () => [node('a')], marqueeJustEnded: () => true });
    selection.checkedIds.value = new Set(['a']);
    expect(click(selection, 'a', false).checked).toBe(true);
    expect(selection.checkedIds.value).toEqual(new Set(['a']));
  });
});
