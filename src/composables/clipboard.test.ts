import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { copyPending } from './clipboard';

const toast = vi.hoisted(() => ({ notifyError: vi.fn(), notifySuccess: vi.fn() }));
vi.mock('./useToast', () => toast);

const writeText = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  writeText.mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { clipboard: { writeText } });
});
afterEach(() => vi.unstubAllGlobals());

describe('copyPending', () => {
  it('copies the text once it is made', async () => {
    expect(await copyPending(Promise.resolve('lovelycarve://open?case=c'), '链接')).toBe(true);
    expect(writeText).toHaveBeenCalledExactlyOnceWith('lovelycarve://open?case=c');
    expect(toast.notifySuccess).toHaveBeenCalledExactlyOnceWith('已复制', '链接');
  });

  it('reports a text that could not be made like a failed copy', async () => {
    expect(await copyPending(Promise.reject(new Error('no active case')), '链接')).toBe(false);
    expect(writeText).not.toHaveBeenCalled();
    expect(toast.notifyError).toHaveBeenCalledExactlyOnceWith('复制失败', 'Error: no active case');
  });
});
