import { fsReadLarge } from '../../../lib/ipc/fs/read';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { fsMediaTemp, fsMediaRelease, newPreviewTaskId } from '../../../lib/ipc/media';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';

/** One viewer's cancellable request and cache lease, including late responses. */
export function createPreviewRequest() {
  const taskId = newPreviewTaskId();
  let cancelled = false;
  let materialized = false;
  const release = () => fsMediaRelease(taskId).catch(error => console.warn('释放预览缓存失败', error));
  return {
    taskId,
    cancel() {
      cancelled = true;
      void taskCancel({ taskId }).catch(error => console.warn('取消预览失败', error));
      if (materialized) void release();
    },
    async read(args: { imageId: ImageId; nodeId: NodeId }) {
      const result = await fsReadLarge({ ...args, taskId });
      if (cancelled) return null;
      return result;
    },
    async materialize(args: { imageId: ImageId; nodeId: NodeId; ext?: string }) {
      const path = await fsMediaTemp({ ...args, taskId });
      materialized = true;
      if (cancelled) { await release(); return null; }
      return path;
    },
  };
}
