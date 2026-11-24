import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiskFsNode } from '../model/model';
import { useFileActions, type FileMenuScope } from './useFileActions';
import { useFileSelection } from './useFileSelection';

const mocks = vi.hoisted(() => ({ caseLink: vi.fn(), copyPending: vi.fn() }));
vi.mock('../../../lib/ipc/cases', () => ({ caseLink: mocks.caseLink }));
vi.mock('../../../composables/clipboard', () => ({ copyPending: mocks.copyPending, copyText: vi.fn() }));

const file = (id: string) => ({ id, name: `${id}.txt`, kind: 'file', size: 1 }) as DiskFsNode;

function actions(target: DiskFsNode, scope: FileMenuScope, checked: string[] = []) {
  const rows = [file('a'), file('b')];
  const selection = useFileSelection({ rows: () => rows, marqueeJustEnded: () => false });
  selection.checkedIds.value = new Set(checked);
  const closeMenu = vi.fn();
  const result = useFileActions({
    imageId: () => 'image-1', rows: () => rows, node: () => target, scope: () => scope,
    currentPath: () => '/', fullPathOf: node => `/${node.name}`, selection, closeMenu,
    exportFile: vi.fn(), exportDirectory: vi.fn(), digest: vi.fn(), identify: vi.fn(),
    setTag: vi.fn(), revealTagColumn: vi.fn(),
  });
  return { ...result, closeMenu };
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.caseLink.mockResolvedValue('lovelycarve://open?case=c');
});

describe('file row links', () => {
  it('copies the link of the one row the menu acts on', () => {
    const { onCopyLink, closeMenu } = actions(file('a'), 'row');
    onCopyLink();
    expect(closeMenu).toHaveBeenCalled();
    expect(mocks.caseLink).toHaveBeenCalledExactlyOnceWith({ imageId: 'image-1', nodeId: 'a' });
    expect(mocks.copyPending).toHaveBeenCalledExactlyOnceWith(expect.any(Promise), '链接');
  });

  it('copies no link for several checked rows', () => {
    const { onCopyLink, closeMenu } = actions(file('a'), 'checked', ['a', 'b']);
    onCopyLink();
    expect(closeMenu).toHaveBeenCalled();
    expect(mocks.caseLink).not.toHaveBeenCalled();
    expect(mocks.copyPending).not.toHaveBeenCalled();
  });
});
