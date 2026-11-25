import { describe, expect, it } from 'vitest';
import { changeIsWarning, changeLabel, failureReason, marketStatusLabel, pluginFailure } from './pluginPackages';

describe('plugin package labels', () => {
  it('names a failure by the reason the backend gave, keeping its words', () => {
    const rejected = { message: 'the package has no library demo.dll', context: { reason: 'noLibrary' }, toString: () => 'the package has no library demo.dll' };
    expect(failureReason(rejected)).toBe('noLibrary');
    expect(pluginFailure(rejected, '安装失败')).toEqual({ title: '插件包没有当前平台的库文件', detail: 'the package has no library demo.dll' });
    // Unknown reasons and plain errors fall back to the action.
    expect(failureReason({ context: { reason: 'somethingElse' } })).toBeNull();
    expect(pluginFailure(new Error('磁盘已满'), '安装失败')).toEqual({ title: '安装失败', detail: 'Error: 磁盘已满' });
  });

  it('describes how a package relates to the installed one', () => {
    expect(changeLabel('upgrade', '1.0.0')).toBe('升级自 1.0.0');
    expect(changeLabel('downgrade', '2.0.0')).toBe('降级自 2.0.0');
    expect(changeLabel('new', null)).toBe('新安装');
    expect(['new', 'upgrade', 'downgrade', 'reinstall', 'rebuild'].filter(change => changeIsWarning(change as never)))
      .toEqual(['downgrade', 'rebuild']);
    expect(marketStatusLabel('updateAvailable')).toBe('可更新');
  });
});
