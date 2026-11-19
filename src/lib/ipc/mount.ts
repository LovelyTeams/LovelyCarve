import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';



export interface DokanMountView {
  id: string;
  image_id: ImageId;
  root_node_id: string;
  mount_point: string;
  running: boolean;
  share: MountShareView | null;
}

export interface MountShareView {
  mount_id: string;
  share_name: string;
  local_path: string;
  unc_path: string;
  running: boolean;
}

export function fsMounts(): Promise<DokanMountView[]> {
  return invokeCmd<DokanMountView[]>('fs_mounts');
}

export function fsAvailableMountPoints(): Promise<string[]> {
  return invokeCmd<string[]>('fs_available_mount_points');
}

/** Return the first free drive letter accepted by the read-only mount layer. */
export function fsSuggestMountPoint(): Promise<string> {
  return invokeCmd<string>('fs_suggest_mount_point');
}

/** Create (or reuse) an empty subfolder `<root>/<imageId>/<name>` for a folder
 *  mount, and return its absolute path. `root` must already exist; the image-id
 *  layer groups one image's partitions together, under which the leaf is named
 *  from `name` (sanitized) with a numeric suffix on collision. */
export function fsCreateMountDir(args: { root: string; imageId: ImageId; name: string }): Promise<string> {
  return invokeCmd<string>('fs_create_mount_dir', args);
}

export function fsMountReadonly(args: {
  imageId: ImageId;
  nodeId: NodeId;
  mountPoint: string;
}): Promise<DokanMountView> {
  return invokeCmd<DokanMountView>('fs_mount_readonly', args);
}

export function fsUnmountReadonly(args: { mountId: string }): Promise<void> {
  return invokeCmd<void>('fs_unmount_readonly', args);
}

export function fsShareMount(args: {
  mountId: string;
  shareName?: string | null;
}): Promise<MountShareView> {
  return invokeCmd<MountShareView>('fs_share_mount', args);
}

export function fsUnshareMount(args: { mountId: string }): Promise<void> {
  return invokeCmd<void>('fs_unshare_mount', args);
}
