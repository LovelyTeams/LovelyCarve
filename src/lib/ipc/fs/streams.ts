import { invokeCmd } from '../client';
import type { ImageId, NodeId } from '../ids';

export interface NtfsDataStream {
  name: string;
  size: number;
  resident: boolean;
}

export function fsStreams(args: {
  imageId: ImageId;
  nodeId: NodeId;
}): Promise<NtfsDataStream[]> {
  return invokeCmd<NtfsDataStream[]>('fs_streams', args);
}

export function fsStreamPreview(args: {
  imageId: ImageId;
  nodeId: NodeId;
  streamName: string;
  maxBytes: number;
}): Promise<number[]> {
  return invokeCmd<number[]>('fs_stream_preview', args);
}

export function fsStreamExport(args: {
  imageId: ImageId;
  nodeId: NodeId;
  streamName: string;
  destination: string;
}): Promise<number> {
  return invokeCmd<number>('fs_stream_export', args);
}
