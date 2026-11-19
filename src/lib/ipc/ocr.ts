// 文字识别（OCR）IPC（原类型视图）。后端 `ocr_scan` 用 PP-OCRv6-tiny（检测 + 识别）经 oar-ocr
// 跑 OCR（复用已有的 ONNX Runtime，CUDA 优先 + CPU 回退）。单次识别暂无进度事件，但会进入后端
// TaskManager 的 CPU/GPU 资源限流。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** 一个识别出的文字区域。`points` 为原图像素坐标系下的多边形角点 `[x, y]`。 */
export interface OcrRegion {
  text: string;
  confidence: number;
  points: [number, number][];
}

/** 一次 OCR 结果。`width`/`height` 为解码出的原图尺寸，用于缩放检测框。 */
export interface OcrOutcome {
  regions: OcrRegion[];
  width: number;
  height: number;
  count: number;
}

/** 识别指定图片文件节点中的文字。后端引擎缓存，重复扫描很快。 */
export function ocrScan(args: { imageId: ImageId; nodeId: NodeId; taskId?: string }): Promise<OcrOutcome> {
  return invokeCmd<OcrOutcome>('ocr_scan', args);
}
