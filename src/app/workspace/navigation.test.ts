import { describe, expect, it } from 'vitest';
import { buildImageNavGroups } from './navigation';

describe('buildImageNavGroups', () => {
  it('organizes the workspace into image, tools and analysis categories', () => {
    expect(buildImageNavGroups().map(group => ({
      label: group.groupLabel,
      pages: group.items.map(item => item.label),
    }))).toEqual([
      { label: '镜像', pages: ['镜像信息', '文件列表'] },
      { label: '工具', pages: ['全局搜索', '时间线', '文件恢复'] },
      { label: '分析', pages: ['分析结果'] },
    ]);
  });
  it('keeps the task hub out of full-page navigation', () => {
    expect(buildImageNavGroups().flatMap(group => group.items).map(item => item.id)).not.toContain('tasks');
  });

  it('shows file carving for raw disk-image sources', () => {
    const tools = buildImageNavGroups().find(group => group.groupId === 'tools');

    expect(tools?.items.map(item => item.id)).toContain('carve');
  });

  it('hides file carving for folder evidence', () => {
    const tools = buildImageNavGroups({ supportsRawAnalysis: false })
      .find(group => group.groupId === 'tools');

    expect(tools?.items.map(item => item.id)).toEqual(['search-results', 'timeline']);
  });
});
