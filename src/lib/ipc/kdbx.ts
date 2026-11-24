// KeePass KDBX IPC for the original-type view. Types mirror the Serialize
// output of src-tauri/src/core/preview/kdbx_preview (snake_case fields).
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** A time as the database states it: an instant (`utc`, Unix seconds), a
 *  KDBX 3 text time without zone (`unzoned`), or text that is not a time. */
export interface KdbxTime {
  kind: 'utc' | 'unzoned' | 'invalid';
  seconds: number | null;
  text: string | null;
}

export interface KdbxKdf {
  name: string;
  uuid: string;
  rounds: number | null;
  /** AES-KDF seed or Argon2 salt, hex. */
  salt: string;
  /** Argon2 memory in bytes. */
  memory: number | null;
  iterations: number | null;
  parallelism: number | null;
  version: number | null;
  secret_key: boolean;
  associated_data: boolean;
}

export interface KdbxVariantItem {
  key: string;
  kind: string;
  value: string;
}

export interface KdbxHeaderField {
  id: number;
  name: string | null;
  offset: number;
  length: number;
}

export interface KdbxHeader {
  file_size: number;
  version: string;
  pre_release: boolean;
  cipher: string;
  cipher_uuid: string;
  compression: string;
  kdf: KdbxKdf;
  master_seed: string;
  encryption_iv: string;
  /** KDBX 3 only. */
  inner_stream: string | null;
  protected_stream_key: string | null;
  stream_start_bytes: string | null;
  comment: string | null;
  public_custom_data: KdbxVariantItem[];
  length: number;
  sha256: string;
  /** KDBX 4: whether the stored header SHA-256 matches. */
  sha256_valid: boolean | null;
  fields: KdbxHeaderField[];
  warnings: string[];
}

export interface KdbxTimes {
  creation: KdbxTime | null;
  last_modification: KdbxTime | null;
  last_access: KdbxTime | null;
  expiry: KdbxTime | null;
  expires: boolean;
  usage_count: number | null;
  location_changed: KdbxTime | null;
}

export interface KdbxCustomData {
  key: string;
  value: string;
  last_modification: KdbxTime | null;
}

export interface KdbxField {
  key: string;
  value: string;
  protected: boolean;
  /** Why a protected value could not be freed; `value` is then as stored. */
  unreadable: string | null;
  /** The value with KeePass field references resolved. */
  resolved: string | null;
}

export interface KdbxAttachment {
  name: string;
  binary_id: number | null;
  size: number | null;
  sha256: string | null;
  protected: boolean;
  unreadable: string | null;
}

export interface KdbxAutoType {
  enabled: boolean | null;
  obfuscation: number | null;
  default_sequence: string | null;
  associations: { window: string; sequence: string }[];
}

export interface KdbxEntry {
  uuid: string | null;
  icon_id: number | null;
  custom_icon: string | null;
  foreground_color: string | null;
  background_color: string | null;
  override_url: string | null;
  quality_check: boolean | null;
  tags: string[];
  previous_parent_group: string | null;
  times: KdbxTimes;
  fields: KdbxField[];
  attachments: KdbxAttachment[];
  auto_type: KdbxAutoType | null;
  custom_data: KdbxCustomData[];
  /** Earlier versions, oldest first. */
  history: KdbxEntry[];
}

export interface KdbxGroup {
  uuid: string | null;
  name: string;
  notes: string | null;
  icon_id: number | null;
  custom_icon: string | null;
  times: KdbxTimes;
  is_expanded: boolean | null;
  default_auto_type_sequence: string | null;
  enable_auto_type: boolean | null;
  enable_searching: boolean | null;
  last_top_visible_entry: string | null;
  previous_parent_group: string | null;
  tags: string[];
  custom_data: KdbxCustomData[];
  entries: KdbxEntry[];
  groups: KdbxGroup[];
}

export interface KdbxMeta {
  generator: string | null;
  settings_changed: KdbxTime | null;
  database_name: string | null;
  database_name_changed: KdbxTime | null;
  database_description: string | null;
  database_description_changed: KdbxTime | null;
  default_username: string | null;
  default_username_changed: KdbxTime | null;
  maintenance_history_days: number | null;
  color: string | null;
  master_key_changed: KdbxTime | null;
  master_key_change_rec: number | null;
  master_key_change_force: number | null;
  master_key_change_force_once: boolean | null;
  memory_protection: { title: boolean | null; username: boolean | null; password: boolean | null; url: boolean | null; notes: boolean | null };
  custom_icons: { uuid: string | null; name: string | null; size: number; last_modification: KdbxTime | null }[];
  recycle_bin_enabled: boolean | null;
  recycle_bin_uuid: string | null;
  recycle_bin_changed: KdbxTime | null;
  entry_templates_group: string | null;
  entry_templates_group_changed: KdbxTime | null;
  history_max_items: number | null;
  history_max_size: number | null;
  last_selected_group: string | null;
  last_top_visible_group: string | null;
  custom_data: KdbxCustomData[];
}

export interface KdbxBinary {
  id: number;
  size: number;
  protected: boolean;
  compressed: boolean;
  sha256: string;
  unreadable: string | null;
}

export interface KdbxDatabase {
  header: KdbxHeader;
  key: { password: 'set' | 'empty' | 'none'; keyfile: string | null; transformed_key: boolean };
  /** Reopens the database without the password and keyfile. */
  transformed_key: string;
  header_hash: 'matches' | 'mismatch' | 'absent';
  inner_stream: string;
  meta: KdbxMeta;
  root: KdbxGroup;
  recycle_bin: string | null;
  deleted_objects: { uuid: string | null; deletion_time: KdbxTime | null }[];
  binaries: KdbxBinary[];
  counts: { groups: number; entries: number; history: number; attachments: number };
  warnings: string[];
}

export type KdbxKeyfile =
  | { kind: 'host'; path: string }
  | { kind: 'evidence'; nodeId: NodeId };

/** An empty or absent password with a keyfile tries the keyfile alone, then
 *  an empty password with it. A transformed key (hex) replaces both. */
export interface KdbxCredentials {
  password?: string;
  keyfile?: KdbxKeyfile;
  transformedKey?: string;
}

export interface KdbxAttachmentRef {
  /** Entry UUID, 32 hex digits. */
  entry: string;
  /** History version, oldest first; omitted for the current entry. */
  history?: number;
  index: number;
}

export function fsKdbxHeader(args: { imageId: ImageId; nodeId: NodeId }): Promise<KdbxHeader> {
  return invokeCmd<KdbxHeader>('fs_kdbx_header', args);
}

/** Wrong credentials reject with code `invalidInput`. */
export function fsKdbxOpen(args: { imageId: ImageId; nodeId: NodeId; credentials: KdbxCredentials }): Promise<KdbxDatabase> {
  return invokeCmd<KdbxDatabase>('fs_kdbx_open', args);
}

/** Up to `maxBytes` (at most 1 MiB) of one attachment. */
export function fsKdbxAttachment(args: {
  imageId: ImageId;
  nodeId: NodeId;
  credentials: KdbxCredentials;
  attachment: KdbxAttachmentRef;
  maxBytes: number;
}): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_kdbx_attachment', args);
}
