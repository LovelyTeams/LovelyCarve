// Sole entry point for Tauri command invocation.
//
// Every IPC call in the frontend funnels through `invokeCmd` so that command
// dispatch, error normalization, and any future middleware (tracing, request
// dedup, retry policy) live in exactly one place. This is the ONLY module
// allowed to import `@tauri-apps/api/core` — every other layer goes through
// the typed wrappers in the sibling `*.ts` domain files.
//
// Enforcement: `rg "@tauri-apps/api/core" src --glob '!src/lib/ipc/**'` must
// stay empty.
import { invoke } from '@tauri-apps/api/core';

export interface BackendAppError {
  code:
    | 'cancelled'
    | 'permissionDenied'
    | 'unsupportedFormat'
    | 'corruptData'
    | 'sourceChanged'
    | 'resourceLimit'
    | 'ffiUnavailable'
    | 'taskPanicked'
    | 'notFound'
    | 'invalidInput'
    | 'caseLocked'
    | 'io'
    | 'internal';
  message: string;
  context: unknown | null;
  retryable: boolean;
}

export class IpcCommandError extends Error {
  readonly code: BackendAppError['code'];
  readonly context: unknown | null;
  readonly retryable: boolean;

  constructor(payload: BackendAppError) {
    super(payload.message);
    this.name = 'IpcCommandError';
    this.code = payload.code;
    this.context = payload.context;
    this.retryable = payload.retryable;
  }

  override toString(): string {
    return this.message;
  }
}

function isBackendAppError(value: unknown): value is BackendAppError {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<BackendAppError>;
  return (
    typeof candidate.code === 'string'
    && typeof candidate.message === 'string'
    && typeof candidate.retryable === 'boolean'
    && Object.prototype.hasOwnProperty.call(candidate, 'context')
  );
}

// String errors pass through unchanged for legacy commands. Commands that have
// migrated to the structured backend shape become `IpcCommandError`, preserving
// existing `String(err)` UI while exposing stable `code` / `retryable` fields.
export function invokeCmd<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args).catch((error: unknown) => {
    if (isBackendAppError(error)) {
      throw new IpcCommandError(error);
    }
    throw error;
  });
}
