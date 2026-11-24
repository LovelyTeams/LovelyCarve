import { describe, expect, it } from 'vitest';
import type { SchedFunction, SchedFunctionOption } from '../../../lib/ipc/tasks';
import { analyzerOptionFields, functionOptions } from './analyzerOptions';

function option(id: string, kind: SchedFunctionOption['kind'], defaults: unknown, choices: string[] = []): SchedFunctionOption {
  return { id, kind, label: id, default: defaults, choices: choices.map(choice => ({ id: choice, label: choice })) };
}

function fn(analyzer: string, options: SchedFunctionOption[]): SchedFunction {
  return { function: `org.test/${analyzer}`, pluginId: 'org.test', analyzer, displayName: `功能 ${analyzer}`, options } as unknown as SchedFunction;
}

describe('analyzer option fields', () => {
  it('shares an option declared alike and keeps one declared differently per function', () => {
    const password = option('password', 'text', '');
    const scan = fn('scan', [password, option('mode', 'flag', false)]);
    const list = fn('list', [password, option('mode', 'text', '')]);
    expect(analyzerOptionFields([scan, list]).map(field => [field.key, field.functionName ?? null])).toEqual([
      ['password', null],
      ['scan\0mode', '功能 scan'],
      ['list\0mode', '功能 list'],
    ]);
  });

  it('sends each function the values of its own options that differ from the defaults', () => {
    const scan = fn('scan', [option('password', 'text', ''), option('mode', 'flag', false), option('items', 'multi', ['a'], ['a', 'b'])]);
    const list = fn('list', [option('password', 'text', ''), option('mode', 'text', '')]);
    const values = { password: 'secret', 'scan\0mode': true, 'list\0mode': '', items: ['a'] };
    expect(functionOptions(values, scan)).toEqual({ password: 'secret', mode: true });
    expect(functionOptions(values, list)).toEqual({ password: 'secret' });
    expect(functionOptions({ items: ['b', 'a'] }, scan)).toEqual({ items: ['b', 'a'] });
    expect(functionOptions({ password: '', items: ['a'] }, scan)).toBeNull();
    expect(functionOptions({}, fn('plain', []))).toBeNull();
  });
});
