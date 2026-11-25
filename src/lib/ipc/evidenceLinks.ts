// `lovelycarve://` links the desktop received (Rust `host::evidence_links`):
// links the app was started with, links other LovelyCarve processes handed
// over and, on macOS, links the system opened. They wait in a queue until the
// main window takes them; the backend announces every arrival.
import { invokeCmd } from './client';
import { onEvent, type UnlistenFn } from './events';
import type { ImageId, NodeId } from './ids';

/** The place a valid link names (snake_case on the wire). */
export interface EvidenceLinkPlace {
  case_id: string;
  image_id?: ImageId;
  node_id?: NodeId;
  root_id?: string;
  path?: string;
  offset?: number;
  length?: number;
}

/** The package identity from an official release-page link. */
export interface PluginInstallLink {
  pluginId: string;
  version: string;
}

/** One received link: a case location, a package install request, or an error. */
export interface ReceivedEvidenceLink {
  url: string;
  link: EvidenceLinkPlace | null;
  plugin_install?: PluginInstallLink;
  error: string | null;
}

/** Event announcing that links wait in the queue (matches the Rust constant). */
export const EVIDENCE_LINK_RECEIVED_EVENT = 'evidence-link-received';

/** Take every link waiting in the queue, oldest first. */
export function takeEvidenceLinks(): Promise<ReceivedEvidenceLink[]> {
  return invokeCmd<ReceivedEvidenceLink[]>('plugin:evidence-links|take');
}

export function onEvidenceLinkReceived(handler: () => void): Promise<UnlistenFn> {
  return onEvent<null>(EVIDENCE_LINK_RECEIVED_EVENT, () => handler());
}
