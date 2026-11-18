// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import FilePreviewDock, { type FilePreviewPlacement } from './FilePreviewDock.vue';

let app: App | undefined;
let host: HTMLDivElement;
beforeEach(() => {
  localStorage.clear();
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => {
  app?.unmount();
  app = undefined;
  host.remove();
});

describe('file preview dock placement', () => {
  it('sizes along its placement axis within the room the host gives', async () => {
    const placement = ref<FilePreviewPlacement>('bottom');
    const limit = ref<number | undefined>(300);
    const locked = ref(false);
    const moves: FilePreviewPlacement[] = [];
    app = createApp({ render: () => h(FilePreviewDock, {
      opening: false, canDetach: true, placement: placement.value, limit: limit.value, canMove: true, moveLocked: locked.value,
      onMove: (next: FilePreviewPlacement) => moves.push(next),
    }) });
    app.mount(host);
    await nextTick();
    const dock = () => host.querySelector<HTMLElement>('.preview-dock')!;
    const divider = () => host.querySelector<HTMLElement>('[role="separator"]')!;
    expect([dock().className, dock().style.height, divider().getAttribute('aria-orientation'), divider().getAttribute('aria-valuemax')])
      .toEqual(['preview-dock bottom', '300px', 'horizontal', '300']);

    divider().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    await nextTick();
    expect(dock().style.height).toBe('276px');
    divider().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    await nextTick();
    expect([dock().style.height, localStorage.getItem('lovelycarve-preview-height')]).toEqual(['300px', '300']);

    host.querySelector<HTMLButtonElement>('[aria-label="移到右侧"]')!.click();
    expect(moves).toEqual(['right']);
    placement.value = 'right';
    limit.value = 400;
    await nextTick();
    expect([dock().className, dock().style.width, dock().style.height, divider().getAttribute('aria-orientation')])
      .toEqual(['preview-dock right', '400px', '', 'vertical']);

    // A workspace without room beside the list keeps the control visible but disabled.
    locked.value = true;
    await nextTick();
    expect(host.querySelector<HTMLButtonElement>('[aria-label="移到下方"]')!.disabled).toBe(true);
  });
});
