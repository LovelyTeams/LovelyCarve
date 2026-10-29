// Tauri event boundary. The ONLY module allowed to import
// `@tauri-apps/api/event`; all `listen`/`emit` usage funnels through here so
// event names and subscription lifecycle have a single home.
//
// Enforcement: `rg "@tauri-apps/api/event" src --glob '!src/lib/ipc/**'` must
// stay empty.
import { listen, once, emit, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event';

// Subscribe to a backend event. Returns the unlisten handle the caller is
// responsible for invoking on teardown (kept identical to raw `listen` so
// existing onBeforeUnmount cleanup keeps working unchanged).
export function onEvent<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  return listen<T>(event, handler);
}

/** Subscribe before creating a native window, including its earliest close. */
export function onWindowDestroyed(label: string, handler: () => void): Promise<UnlistenFn> {
  return once('tauri://destroyed', handler, { target: { kind: 'Window', label } });
}

// Emit a custom app event (cross-window messaging, settings open, etc.).
export function emitEvent<T>(event: string, payload?: T): Promise<void> {
  return emit(event, payload);
}

export type { UnlistenFn };
