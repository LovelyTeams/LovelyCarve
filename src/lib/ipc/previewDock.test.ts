import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const bus = vi.hoisted(() => ({
  handlers: new Map<string, Set<(event: { payload: any }) => unknown>>(),
}));
vi.mock('./events', () => ({
  onEvent: vi.fn(async (name: string, handler: (event: { payload: any }) => unknown) => {
    const listeners = bus.handlers.get(name) ?? new Set();
    listeners.add(handler);
    bus.handlers.set(name, listeners);
    return () => listeners.delete(handler);
  }),
  emitEvent: vi.fn(async (name: string, payload: unknown) => {
    await Promise.all([...bus.handlers.get(name) ?? []].map(handler => handler({ payload })));
  }),
}));

import { onEvent, emitEvent } from './events';
import { onPreviewDockRequest, onPreviewSelection, onPreviewSelectionRequest, publishPreviewSelection, requestPreviewDock, requestPreviewSelection } from './previewDock';

beforeEach(() => { bus.handlers.clear(); vi.clearAllMocks(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe('preview docking handshake', () => {
  it('carries the current reading context in the acknowledged dock request', async () => {
    const owner = vi.fn(() => true);
    await onPreviewDockRequest('owner', owner);
    const readingState = { imageId: 'image', nodeId: 'file', tab: 'hex' as const, searchQuery: 'needle', encoding: 'utf-8', page: 3, pageSize: 2048, currentMatch: 1, scrollTop: 180, scrollLeft: 20, codeScrollTop: 0, codeScrollLeft: 0 };
    await requestPreviewDock('owner', 'preview', readingState);
    expect(owner).toHaveBeenCalledExactlyOnceWith('preview', readingState);
    expect(vi.getTimerCount()).toBe(0);
  });
  it('routes live selections and startup snapshots only to their originating preview', async () => {
    const own = vi.fn();
    const otherOwner = vi.fn();
    const otherPreview = vi.fn();
    const cleanup = await onPreviewSelection('owner', 'preview', own);
    await onPreviewSelection('other', 'preview', otherOwner);
    await onPreviewSelection('owner', 'other-preview', otherPreview);
    const selection = { ownerId: 'owner', previewId: 'preview', revision: 4, args: { imageId: 'image', nodeId: 'file', node: { id: 'file' }, title: 'file', fullPath: '/file' } };
    const snapshot = vi.fn(async previewId => { if (previewId === 'preview') await publishPreviewSelection(selection); });
    const unrelated = vi.fn(async () => {});
    await onPreviewSelectionRequest('owner', snapshot);
    await onPreviewSelectionRequest('other', unrelated);
    await requestPreviewSelection('owner', 'preview');
    expect(own).toHaveBeenCalledExactlyOnceWith(selection);
    expect(otherOwner).not.toHaveBeenCalled();
    expect(otherPreview).not.toHaveBeenCalled();
    expect(unrelated).not.toHaveBeenCalled();
    cleanup();
    await publishPreviewSelection({ ...selection, revision: 5 });
    expect(own).toHaveBeenCalledOnce();
  });
  it('restores only in the originating list and releases the response listener', async () => {
    const owner = vi.fn(() => true);
    const other = vi.fn(() => true);
    const cleanup = await onPreviewDockRequest('owner', owner);
    await onPreviewDockRequest('other', other);
    await requestPreviewDock('owner', 'file-a');
    expect(owner).toHaveBeenCalledExactlyOnceWith('file-a');
    expect(other).not.toHaveBeenCalled();
    expect(bus.handlers.get('file-preview-dock-response')?.size).toBe(0);
    cleanup();
    expect(bus.handlers.get('file-preview-dock-request')?.size).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('keeps the independent preview open when the evidence has changed', async () => {
    await onPreviewDockRequest('owner', () => false);
    await expect(requestPreviewDock('owner', 'file-a')).rejects.toThrow('已切换到其他检材');
    expect(bus.handlers.get('file-preview-dock-response')?.size).toBe(0);
  });

  it('ignores acknowledgements belonging to other requests and times out safely', async () => {
    const result = requestPreviewDock('closed-owner', 'file-a');
    const assertion = expect(result).rejects.toThrow('未响应');
    await emitEvent('file-preview-dock-response', { requestId: 'unrelated', accepted: true });
    await vi.advanceTimersByTimeAsync(3000);
    await assertion;
    expect(bus.handlers.get('file-preview-dock-response')?.size).toBe(0);
  });

  it('cleans up when event delivery fails', async () => {
    vi.mocked(emitEvent).mockRejectedValueOnce(new Error('event unavailable'));
    await expect(requestPreviewDock('owner', 'file-a')).rejects.toThrow('event unavailable');
    expect(vi.getTimerCount()).toBe(0);
    expect(bus.handlers.get('file-preview-dock-response')?.size).toBe(0);
  });

  it('cleans up when listener registration fails', async () => {
    vi.mocked(onEvent).mockRejectedValueOnce(new Error('listen unavailable'));
    await expect(requestPreviewDock('owner', 'file-a')).rejects.toThrow('listen unavailable');
    expect(vi.getTimerCount()).toBe(0);
  });
});
