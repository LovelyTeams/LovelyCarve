// 二维码识别 IPC（原类型视图）。后端 `qr_scan` 跑 OpenCV WeChat 检测器 + 暴力
// 变换扫描，进度走 `qr-scan-progress` 事件，可用 `taskCancel({ taskId })` 取消。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** 一个识别出的二维码。`points` 为原图像素坐标系下的 4 个角点 `[x, y]`（可能为空）。 */
export interface QrFinding {
  text: string;
  points: [number, number][];
  transform: string;
}

/** 一次扫描结果。`width`/`height` 为解码出的原图尺寸，用于缩放检测框。 */
export interface QrScanOutcome {
  codes: QrFinding[];
  width: number;
  height: number;
  attempts: number;
  cancelled: boolean;
  detector: string;
}

/** `qr-scan-progress` 事件载荷（按 `task_id` 过滤）。 */
export interface QrScanProgress {
  task_id: string;
  attempts: number;
  total: number;
  found: number;
}

/** 扫描指定文件节点中的二维码。长任务：进度走 `qr-scan-progress`，
 *  可用 `taskCancel({ taskId })`（见 `lib/ipc/fs`）取消。`expectedCount` 达到即停。 */
export function qrScan(args: {
  imageId: ImageId;
  nodeId: NodeId;
  expectedCount: number;
  taskId: string;
}): Promise<QrScanOutcome> {
  return invokeCmd<QrScanOutcome>('qr_scan', args);
}
