import type { CaseInfo } from '../../../lib/ipc/cases';

import { shallowRef } from 'vue';

interface TaskContext {
  activeCase: () => Pick<CaseInfo, 'dir' | 'evidence'> | null;
  /** Starts the platform-detection report listener of the image-info page. */
  ensureDetection: () => void;
}
const context = shallowRef<TaskContext | null>(null);
/** Main-window wiring supplies read-only case and detection inputs, without owning their state. */
export function registerTaskContext(value: TaskContext): () => void {
  context.value = value;
  value.ensureDetection();
  return () => { if (context.value === value) context.value = null; };
}
export function taskCaseContext() { return context.value?.activeCase() ?? null; }
export function ensureTaskDetection() { context.value?.ensureDetection(); }
