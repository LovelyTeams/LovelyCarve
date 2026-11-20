// Aggregate entry point for the IPC service layer. Import domain namespaces
// (`import { fs } from '../lib/ipc'`) or pull functions directly from the
// domain modules (`import { fsChildren } from '../lib/ipc/fs'`).
//
// Domain modules are intentionally split by concern rather than collapsed
// into one god file — add a new sibling module per backend domain.

export * as filedb from './filedb';
export * as search from './search';
export * as timeline from './timeline';
export * as cases from './cases';
export * as raid from './raid';
export * as plugins from './plugins';
export * as autodetect from './autodetect';
export { invokeCmd } from './client';
export { onEvent, emitEvent, type UnlistenFn } from './events';
export * from './carve';
