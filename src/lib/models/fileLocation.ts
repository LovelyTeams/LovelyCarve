import type { ImageId, NodeId } from '../ipc/ids';



export type FileLocationTarget = {
  imageId: ImageId;
  offset?: number;
  length?: number;
} & ({ nodeId: NodeId; rootId?: never; path?: never } | { rootId: string; path: string; nodeId?: never });
