// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import AnalysisResultsNav from './AnalysisResultsNav.vue';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';

const state = vi.hoisted(() => ({ selectNode: vi.fn(), reload: vi.fn(), setImageId: vi.fn(), ensureFresh: vi.fn(), openPage: vi.fn() }));
const tree = ref<AnalysisResultNode[]>([]);
const selectedNode = ref<AnalysisResultNode | null>(null);
const treeLoading = ref(false);
const treeError = ref<string | null>(null);
vi.mock('../composables/useAnalysisResults', () => ({
  useAnalysisResults: () => ({ tree, selectedNode, treeLoading, treeError, ...state }),
}));
vi.mock('../composables/useAnalysisLabels', () => ({
  useAnalysisLabels: () => {
    const nodeLabel = (node: AnalysisResultNode) => node.label;
    return { nodeLabel, nodeLabels: { node: nodeLabel, category: (node: AnalysisResultNode) => node.typeName } };
  },
}));
let app: App | null = null;
let host: HTMLElement;
const imageId = ref('one');
function result(nodeId: string, imageId: string, label: string): AnalysisResultNode {
  return {
    nodeId, imageId, label, kind: 'result', pluginId: 'p', analyzer: 'linuxinfo', function: 'p/linuxinfo', artifactType: nodeId, typeName: 'baseinfo',
    sourceNodeId: null, targetLabel: 'Kali · part:2', runId: 'run', completedAt: '', childCount: 3, warningCount: 0, legacy: false, children: [],
  };
}
const users = result('users', 'one', '用户列表');
const logins = result('logins', 'one', '登录记录');
const run = { ...users, nodeId: 'linuxinfo', kind: 'function' as const, label: 'Linux 系统信息', warningCount: 2, children: [users, logins] };
beforeEach(() => {
  tree.value = [run, result('other', 'two', '其他检材结果')];
  selectedNode.value = users;
  treeLoading.value = false;
  treeError.value = null;
  vi.clearAllMocks();
  imageId.value = 'one';
  host = document.createElement('div');
  document.body.append(host);
  app = createApp({ render: () => h(AnalysisResultsNav, { imageId: imageId.value, onSelect: state.openPage }) });
  app.mount(host);
});
afterEach(() => { app?.unmount(); app = null; document.body.replaceChildren(); });

describe('AnalysisResultsNav', () => {
  it('shows the current evidence plugin results directly and selects the clicked result', () => {
    const items = [...host.querySelectorAll<HTMLButtonElement>('.result-item')];
    expect(items.map(item => item.querySelector('span')?.textContent)).toEqual(['用户列表', '登录记录']);
    expect(host.querySelector('header')).toBeNull();
    expect(items[0].getAttribute('aria-current')).toBe('true');
    items[1].click();
    expect(state.selectNode).toHaveBeenCalledWith(logins);
    expect(state.openPage).toHaveBeenCalledOnce();
  });
  it('names the run target, counts its warnings and opens its audit', () => {
    const heading = host.querySelector<HTMLElement>('.group-heading')!;
    expect(heading.querySelector('small')?.textContent).toBe('Kali · part:2');
    expect(heading.querySelector('.ui-badge.warn')?.textContent).toBe('2');
    host.querySelector<HTMLButtonElement>('.run-info')!.click();
    expect(state.selectNode).toHaveBeenCalledWith(run);
  });
  it('loads results independently of the right panel and refreshes the context when evidence changes', async () => {
    expect(state.setImageId).toHaveBeenCalledWith('one');
    expect(state.ensureFresh).toHaveBeenCalledOnce();
    expect(state.openPage).not.toHaveBeenCalled();
    imageId.value = 'two';
    await nextTick();
    expect(state.setImageId).toHaveBeenLastCalledWith('two');
    expect(state.ensureFresh).toHaveBeenCalledTimes(2);
    expect([...host.querySelectorAll('.result-item span')].map(item => item.textContent)).toEqual(['其他检材结果']);
  });
  it('narrows plugin result entries with its own search field', async () => {
    const input = host.querySelector<HTMLInputElement>('input')!;
    input.focus();
    expect(document.activeElement).toBe(input);
    input.value = '登录';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    expect([...host.querySelectorAll('.result-item span')].map(item => item.textContent)).toEqual(['登录记录']);
  });
  it('clears filtering with Escape and keeps keyboard focus in the directory', async () => {
    const input = host.querySelector<HTMLInputElement>('input')!;
    input.focus();
    input.value = '登录';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await nextTick();
    const parentEscape = vi.fn();
    host.addEventListener('keydown', parentEscape);
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    await nextTick();
    expect(input.value).toBe('');
    expect(host.querySelectorAll('.result-item')).toHaveLength(2);
    expect(document.activeElement).toBe(input);
    expect(parentEscape).not.toHaveBeenCalled();
  });
  it('announces directory loading, errors and an empty filter result', async () => {
    tree.value = [];
    treeLoading.value = true;
    await nextTick();
    expect(host.querySelector('[role="status"]')?.textContent).toContain('正在读取');
    treeLoading.value = false;
    treeError.value = '读取失败';
    await nextTick();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('读取失败');
    treeError.value = null;
    await nextTick();
    expect(host.querySelector('[role="status"]')?.textContent).toContain('暂无分析结果');
  });
});
