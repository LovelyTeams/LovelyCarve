// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import SidebarNav from './SidebarNav.vue';
import { buildImageNavGroups } from '../workspace/navigation';
import type { NavGroup } from '../workspace/types';

let app: App | null = null;
let host: HTMLElement;
const activeId = ref('files');
const groups = ref<NavGroup[]>([]);
const selections: string[] = [];

function mount() {
  host = document.createElement('div');
  document.body.append(host);
  app = createApp({ render: () => h(SidebarNav, {
    groups: groups.value,
    activeId: activeId.value,
    onSelect: (id: string) => { selections.push(id); activeId.value = id; },
  }, { context: ({ select }: { select: (id: string) => void }) => h('button', { onClick: () => select('analysis') }, '分析结果目录') }) });
  app.mount(host);
}
function category(label: string) {
  return [...host.querySelectorAll<HTMLButtonElement>('.category-button')].find(button => button.textContent?.trim() === label)!;
}
function pages() { return [...host.querySelectorAll('.page-label')].map(item => item.textContent); }
async function settle() { await nextTick(); await nextTick(); }

beforeEach(() => {
  localStorage.clear();
  activeId.value = 'files';
  groups.value = buildImageNavGroups();
  selections.length = 0;
});
afterEach(() => { app?.unmount(); app = null; document.body.replaceChildren(); });

describe('SidebarNav', () => {
  it('switches categories and restores the last page in each category', async () => {
    mount();
    expect(pages()).toEqual(['镜像信息', '文件列表']);
    category('工具').click();
    await settle();
    expect(activeId.value).toBe('search-results');
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    host.querySelector<HTMLButtonElement>('[title="时间线"]')!.click();
    await settle();
    category('镜像').click();
    await settle();
    expect(activeId.value).toBe('files');
    category('工具').click();
    await settle();
    expect(activeId.value).toBe('timeline');
    expect(selections).toEqual(['search-results', 'timeline', 'files', 'timeline']);
  });

  it('follows external page changes and displays analysis context in the middle column', async () => {
    mount();
    activeId.value = 'analysis';
    await settle();
    expect(category('分析').getAttribute('aria-pressed')).toBe('true');
    expect(pages()).toEqual([]);
    expect(host.querySelector('.page-sidebar h2')).toBeNull();
    expect(host.querySelector('.page-sidebar .sidebar-context')?.textContent).toBe('分析结果目录');
    activeId.value = 'files';
    await settle();
    expect(category('镜像').getAttribute('aria-pressed')).toBe('true');
    expect(host.querySelector('.sidebar-context')).toBeNull();
  });

  it('reopens the page column when selecting a category from the collapsed rail', async () => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', 'true');
    mount();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(true);
    category('分析').click();
    await settle();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(false);
    expect(localStorage.getItem('lovelycarve-sidebar-collapsed')).toBe('false');
    expect(pages()).toEqual([]);
  });

  it('falls back to an available tool when the remembered carving page is unsupported', async () => {
    activeId.value = 'carve';
    mount();
    activeId.value = 'files';
    groups.value = buildImageNavGroups({ supportsRawAnalysis: false });
    await settle();
    category('工具').click();
    await settle();
    expect(activeId.value).toBe('search-results');
    expect(pages()).toEqual(['全局搜索', '时间线']);
  });

  it('previews hovered categories without changing the page or the persisted collapsed state', async () => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', 'true');
    mount();
    const sidebar = host.querySelector<HTMLElement>('.sidebar-nav')!;
    const pageColumn = host.querySelector<HTMLElement>('.page-sidebar')!;
    expect(pageColumn.style.display).toBe('none');
    category('工具').dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(sidebar.classList.contains('hover-preview')).toBe(true);
    expect(pageColumn.style.display).not.toBe('none');
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    expect(category('工具').getAttribute('aria-expanded')).toBe('true');
    expect(category('镜像').getAttribute('aria-pressed')).toBe('true');
    expect(activeId.value).toBe('files');
    expect(selections).toEqual([]);
    expect(localStorage.getItem('lovelycarve-sidebar-collapsed')).toBe('true');
    category('工具').dispatchEvent(new Event('pointerleave'));
    pageColumn.dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(sidebar.classList.contains('hover-preview')).toBe(true);
    category('镜像').dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(pages()).toEqual(['镜像信息', '文件列表']);
    sidebar.dispatchEvent(new Event('pointerleave'));
    await settle();
    expect(pageColumn.style.display).toBe('none');
    expect(sidebar.classList.contains('hover-preview')).toBe(false);
  });

  it('opens a previewed child page and dismisses the floating column without pinning it', async () => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', 'true');
    mount();
    category('工具').dispatchEvent(new Event('pointerenter'));
    await settle();
    host.querySelector<HTMLButtonElement>('[title="时间线"]')!.click();
    await settle();
    expect(activeId.value).toBe('timeline');
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(false);
    expect(localStorage.getItem('lovelycarve-sidebar-collapsed')).toBe('true');
  });

  it.each([true, false])('previews the plugin result directory and opens its result with collapsed=%s', async collapsed => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', String(collapsed));
    mount();
    category('分析').dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(host.querySelector('.sidebar-context')?.textContent).toBe('分析结果目录');
    expect(pages()).toEqual([]);
    expect(activeId.value).toBe('files');
    host.querySelector<HTMLButtonElement>('.sidebar-context button')!.click();
    await settle();
    expect(activeId.value).toBe('analysis');
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(false);
    expect(localStorage.getItem('lovelycarve-sidebar-collapsed')).toBe(String(collapsed));
  });

  it('supports keyboard preview, keeps it while focusing children, and restores focus on Escape', async () => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', 'true');
    mount();
    category('工具').focus();
    await settle();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(true);
    const child = host.querySelector<HTMLButtonElement>('[title="时间线"]')!;
    child.focus();
    await settle();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(true);
    child.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    await settle();
    expect(document.activeElement).toBe(category('工具'));
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(false);
    category('镜像').focus();
    await settle();
    category('镜像').blur();
    await settle();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(false);
    expect(selections).toEqual([]);
  });

  it('previews another category in the expanded column and restores the active page category on leaving', async () => {
    mount();
    category('工具').dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    expect(activeId.value).toBe('files');
    expect(selections).toEqual([]);
    expect(category('镜像').getAttribute('aria-pressed')).toBe('true');
    expect(category('工具').getAttribute('aria-expanded')).toBe('true');
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(false);
    // Expanded previews use the existing column instead of the floating layout.
    expect(host.querySelector('.sidebar-nav')?.classList.contains('hover-preview')).toBe(false);
    category('工具').dispatchEvent(new Event('pointerleave'));
    host.querySelector('.page-sidebar')!.dispatchEvent(new Event('pointerenter'));
    await settle();
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    host.querySelector('.sidebar-nav')!.dispatchEvent(new Event('pointerleave'));
    await settle();
    expect(pages()).toEqual(['镜像信息', '文件列表']);
    expect(host.querySelector<HTMLElement>('.page-sidebar')!.style.display).not.toBe('none');
  });

  it('opens a hovered child in the expanded column and retains that category after leaving', async () => {
    mount();
    category('工具').dispatchEvent(new Event('pointerenter'));
    await settle();
    host.querySelector<HTMLButtonElement>('[title="时间线"]')!.click();
    await settle();
    expect(activeId.value).toBe('timeline');
    host.querySelector('.sidebar-nav')!.dispatchEvent(new Event('pointerleave'));
    await settle();
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(false);
  });

  it('dismisses a keyboard preview in the expanded column with Escape', async () => {
    mount();
    category('工具').focus();
    await settle();
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
    const child = host.querySelector<HTMLButtonElement>('[title="时间线"]')!;
    child.focus();
    child.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    await settle();
    expect(document.activeElement).toBe(category('工具'));
    expect(pages()).toEqual(['镜像信息', '文件列表']);
    expect(activeId.value).toBe('files');
  });

  it('keeps only the collapse action in the category rail and does not intercept slash', async () => {
    localStorage.setItem('lovelycarve-sidebar-collapsed', 'true');
    mount();
    const actions = [...host.querySelectorAll<HTMLButtonElement>('.rail-actions button')];
    expect(actions.map(button => button.getAttribute('aria-label'))).toEqual(['展开子类栏']);
    const slash = new KeyboardEvent('keydown', { key: '/', cancelable: true });
    window.dispatchEvent(slash);
    await settle();
    expect(slash.defaultPrevented).toBe(false);
    expect(host.querySelector('input')).toBeNull();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(true);
    actions[0].click();
    await settle();
    expect(host.querySelector('.sidebar-nav')?.classList.contains('collapsed')).toBe(false);
    expect(actions[0].getAttribute('aria-label')).toBe('收起子类栏');
    expect(pages()).toEqual(['镜像信息', '文件列表']);
    category('工具').click();
    await settle();
    expect(pages()).toEqual(['全局搜索', '时间线', '文件恢复']);
  });

  it('preserves dynamically added BCP categories and their counts', async () => {
    groups.value.push({ groupId: 'bcp', groupLabel: 'BCP 数据', groupIcon: 'Database', groupColor: 'var(--primary-color)',
      items: [{ id: 'bcp', label: '全部数据', icon: 'Database', badge: 1204, badgeType: 'info' }] });
    mount();
    category('BCP 数据').click();
    await settle();
    expect(activeId.value).toBe('bcp');
    expect(pages()).toEqual(['全部数据']);
    expect(host.querySelector('.page-button .ui-badge')?.textContent).toBe((1204).toLocaleString());
  });
});
