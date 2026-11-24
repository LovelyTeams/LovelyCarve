// 运行前编辑的分析器参数（`[[analyzer.option]]`）：把一个插件各功能声明的参数排成一张表单，
// 再按功能取出提交给 sched_task_create 的值。纯模型，不含响应式状态。
import type { SchedFunction, SchedFunctionOption } from '../../../lib/ipc/tasks';

/** One field of the form. `key` holds its value: the option id when every
 *  function of the plugin that declares the id declares it alike, otherwise a
 *  key of its own function, labelled with that function's name. */
export interface AnalyzerOptionField {
  key: string;
  pluginId: string;
  analyzer: string;
  /** The function the field belongs to alone; absent for a shared field. */
  functionName?: string;
  option: SchedFunctionOption;
}

function shape(option: SchedFunctionOption): string {
  return JSON.stringify([option.kind, option.default ?? null, option.choices.map(choice => choice.id)]);
}

function ownKey(fn: SchedFunction, option: SchedFunctionOption): string {
  return `${fn.analyzer}\0${option.id}`;
}

/** The fields of `functions` (one plugin's), in declaration order. */
export function analyzerOptionFields(functions: readonly SchedFunction[]): AnalyzerOptionField[] {
  const shapes = new Map<string, Set<string>>();
  for (const fn of functions) {
    for (const option of fn.options) {
      const known = shapes.get(option.id) ?? new Set<string>();
      known.add(shape(option));
      shapes.set(option.id, known);
    }
  }
  const seen = new Set<string>();
  const fields: AnalyzerOptionField[] = [];
  for (const fn of functions) {
    for (const option of fn.options) {
      const shared = shapes.get(option.id)?.size === 1;
      const key = shared ? option.id : ownKey(fn, option);
      if (seen.has(key)) continue;
      seen.add(key);
      fields.push(shared
        ? { key, pluginId: fn.pluginId, analyzer: fn.analyzer, option }
        : { key, pluginId: fn.pluginId, analyzer: fn.analyzer, functionName: fn.displayName, option });
    }
  }
  return fields;
}

/** Whether `value` is what the analyzer runs with when the option is left out. */
function isDefault(option: SchedFunctionOption, value: unknown): boolean {
  switch (option.kind) {
    case 'flag':
      return value === (option.default === true);
    case 'text':
    case 'choice':
      return value === (typeof option.default === 'string' ? option.default : '');
    case 'multi': {
      const defaults = new Set(Array.isArray(option.default) ? option.default : []);
      return Array.isArray(value) && value.length === defaults.size && value.every(item => defaults.has(item));
    }
  }
}

/** The options to create `fn`'s task with: the form values of the options it
 *  declares that differ from their defaults, or `null` when none do. */
export function functionOptions(values: Readonly<Record<string, unknown>>, fn: SchedFunction): Record<string, unknown> | null {
  const options: Record<string, unknown> = {};
  for (const option of fn.options) {
    const key = Object.hasOwn(values, ownKey(fn, option)) ? ownKey(fn, option) : option.id;
    if (Object.hasOwn(values, key) && !isDefault(option, values[key])) options[option.id] = values[key];
  }
  return Object.keys(options).length ? options : null;
}
