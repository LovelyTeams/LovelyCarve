import type { ImageId, NodeId } from '../../../lib/ipc/ids';



export interface FileListNavigator {
  imageId: ImageId;
  openFileByNodeId: (nodeId: NodeId, offset?: number, length?: number) => Promise<void>;
  openFileBySource: (rootId: string, path: string) => Promise<void>;
}

/** A directory breadcrumb also remembers the position to restore on return. */
export interface FileNavigationFrame {
  id: string;
  name: string;
  scrollTop?: number;
}

export interface FileScrollRestore {
  scrollTop: number;
  childId: string | null;
  openDetail?: boolean;
}

export interface FileByteFocus {
  nodeId: NodeId;
  offset: number;
  length: number;
}
