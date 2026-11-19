// 跨窗口同步文件预览面板（DiskFileDetail）的「数据」状态。
//
// 独立预览窗（FilePreviewWindow）与主窗口是同源、不同 webview 的两个文档，
// 共享同一份 localStorage——主题切换就是借 `storage` 事件跨窗口广播的。预览面板
// 展示的两类缓存数据（Magika 识别的真实类型 ident、内容哈希 digest）走同一条
// 通道：任一窗口算出新结果后写入 KEY，其余窗口经 `storage` 事件收到并更新各自
// 的缓存 / 展示，避免「内嵌面板已识别 / 已算哈希、独立窗仍空白」这类不同步。
// 同文档不会收到自己的 `storage` 事件，故内嵌面板→列表缓存仍走原有的就地回灌，
// 本通道只负责跨窗口那一跳。注意：标签页 / 编码 / 搜索 / 滚动等纯 UI 状态刻意
// 不同步；拖出的面板跟随主列表的文件选择由 lib/ipc/previewDock 的 Tauri 事件单独传递。
import { onBeforeUnmount } from 'vue';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';
import type { ImageId, NodeId } from '../../../lib/ipc/ids';

const KEY = 'lovelycarve-preview-sync';

export type PreviewSyncMessage =
  | { kind: 'ident'; imageId: ImageId; nodeId: NodeId; ident: FsTypeIdent }
  | { kind: 'digest'; imageId: ImageId; nodeId: NodeId; digest: StoredDigest };

// 广播一条预览数据给其它窗口。附带 nonce，使同一文件重复算出相同结果时
// localStorage 的值仍会变化，从而照常触发 `storage` 事件。best-effort：隐私
// 模式 / 配额异常时静默忽略，不影响本窗口自身的流程。
export function broadcastPreview(msg: PreviewSyncMessage): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...msg, nonce: nextNonce() }));
  } catch {
    /* 同步是尽力而为，写不进就算了 */
  }
}

let nonceCounter = 0;
function nextNonce(): number {
  nonceCounter += 1;
  return nonceCounter;
}

// 订阅其它窗口广播的预览数据。返回的清理函数会在组件卸载时自动调用。
export function onPreviewBroadcast(handler: (msg: PreviewSyncMessage) => void): () => void {
  function onStorage(e: StorageEvent) {
    if (e.key !== KEY || !e.newValue) return;
    try {
      const m = JSON.parse(e.newValue) as Partial<PreviewSyncMessage>;
      if (!m.imageId || !m.nodeId) return;
      if (m.kind === 'ident' && m.ident) {
        handler({ kind: 'ident', imageId: m.imageId, nodeId: m.nodeId, ident: m.ident });
      } else if (m.kind === 'digest' && m.digest) {
        handler({ kind: 'digest', imageId: m.imageId, nodeId: m.nodeId, digest: m.digest });
      }
    } catch {
      /* 广播负载损坏：忽略，下一条照常处理 */
    }
  }
  window.addEventListener('storage', onStorage);
  const cleanup = () => window.removeEventListener('storage', onStorage);
  onBeforeUnmount(cleanup);
  return cleanup;
}
