// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, type App } from 'vue';
import { useSearchConfiguration } from '../composables/useSearchConfiguration';
import SearchCreateDialog from './SearchCreateDialog.vue';

vi.mock('../../../lib/ipc/fs/query', () => ({ fsChildren: async () => [{ id: 'part:1', name: '系统卷' }] }));
let app: App; let host: HTMLElement; let configuration: ReturnType<typeof useSearchConfiguration>;
const submitted = vi.fn();
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
async function mount() {
  host = document.createElement('div'); document.body.append(host);
  app = createApp(defineComponent({ setup() {
    configuration = useSearchConfiguration({ imageId: () => 'a', onSubmit: submitted });
    return () => h(SearchCreateDialog, { open: configuration.dialogVisible.value, draft: configuration.dialog, elements: configuration.elements, starting: configuration.starting.value, loadingPartitions: configuration.loadingPartitions.value, partitionOptions: configuration.partitionOptions.value, scopeSummary: configuration.scopeSummary.value, advancedSummary: configuration.advancedSummary.value, dialogError: configuration.dialogError.value, onClose: configuration.closeCreateDialog, onSubmit: configuration.submitCreateDialog, onToggleCodePage: configuration.toggleCodePage });
  } }));
  app.mount(host); await configuration.openCreateDialog(); await settle();
}
function popup() { return document.querySelector<HTMLElement>('[role="dialog"]')!; }
function button(text: string) { return [...popup().querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === text)!; }
function keyword(text: string) { const field = popup().querySelector<HTMLTextAreaElement>('.keyword-textarea')!; field.value = text; field.dispatchEvent(new Event('input', { bubbles: true })); return field; }
beforeEach(() => { submitted.mockReset(); });
afterEach(() => { app?.unmount(); document.body.replaceChildren(); });

describe('search dialog composition', () => {
  it('binds the real keyword focus and partition checkbox to serialized submission', async () => {
    await mount(); const field = keyword('password'); await settle(); expect(document.activeElement).toBe(field);
    const partition = popup().querySelector<HTMLInputElement>('input[value="part:1"]')!; partition.checked = true; partition.dispatchEvent(new Event('change', { bubbles: true })); await settle();
    button('开始搜索').click(); await settle();
    expect(submitted.mock.calls[0]?.[0]).toMatchObject({ keys: ['password'], scope: { partitions: ['part:1'], tags: [], tree_nodes: [] } });
    expect(configuration.dialogVisible.value).toBe(false);
  });
  it('keeps encoding validation and the pressed state of its controls', async () => {
    await mount(); keyword('password'); await settle();
    for (const page of ['UTF-8', 'GBK', 'UTF-16LE', 'UTF-16BE']) { button(page).click(); await settle(); }
    expect(button('开始搜索').disabled).toBe(true); expect(popup().textContent).toContain('至少选择一种编码');
    button('ASCII').click(); await settle(); expect(button('ASCII').getAttribute('aria-pressed')).toBe('true');
    button('开始搜索').click(); await settle(); expect(submitted.mock.calls[0]?.[0].codepages).toEqual(['ASCII']);
  });
  it('links both invalid limit fields to errors and exposes their advanced section', async () => {
    await mount(); keyword('password'); await settle();
    configuration.dialog.perFileCap = 0; configuration.dialog.globalCap = 1.5; await settle();
    const fields = [...popup().querySelectorAll<HTMLInputElement>('input[type="number"]')];
    for (const field of fields) {
      expect(field.getAttribute('aria-invalid')).toBe('true');
      expect(document.getElementById(field.getAttribute('aria-describedby')!)?.className).toBe('ui-field-error');
      expect(field.closest('details')?.open).toBe(true);
    }
    expect(button('开始搜索').disabled).toBe(true);
    configuration.dialog.perFileCap = 1; configuration.dialog.globalCap = 10; await settle();
    expect(fields.every(field => !field.hasAttribute('aria-invalid'))).toBe(true);
    expect(button('开始搜索').disabled).toBe(false);
  });
  it('restores the dialog opener after Escape', async () => {
    const opener = document.createElement('button'); document.body.append(opener); opener.focus();
    await mount();
    document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle(); expect(document.activeElement).toBe(opener);
  });
});
