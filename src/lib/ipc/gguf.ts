// GGUF（llama.cpp / ggml 大模型权重容器）解析 IPC：原类型视图里 .gguf 文件的
// 结构化报告。类型镜像 src-tauri/src/core/gguf.rs 的 Serialize 输出（snake_case）。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** 按 ggml 张量类型聚合的分布项。 */
export interface GgufTypeCount {
  type_name: string;
  count: number;
  parameters: number;
}

/** 单条元数据 KV。数组值的 value 为空串，看 array_len / array_preview。 */
export interface GgufMetaEntry {
  key: string;
  /** 值类型可读名：标量如 string / u32 / f32；数组如 string[] / f32[]。 */
  type_name: string;
  value: string;
  /** 非 null 表示该值是数组，数字为元素总数。 */
  array_len: number | null;
  /** 数组前若干个元素的展示字符串（后端按上限截断）。 */
  array_preview: string[] | null;
}

/** 单个张量的元信息（不含权重数据本体）。 */
export interface GgufTensorEntry {
  name: string;
  /** ggml 类型名，如 BF16 / Q4_K；未识别为 "Unknown(id)"。 */
  type_name: string;
  dimensions: number[];
  /** 元素数 = 各维度之积。 */
  parameters: number;
  /** 相对张量数据区起点的偏移。 */
  offset: number;
}

export interface GgufReport {
  version: number;
  /** 张量总数（文件头声明值，可能 > tensors.length，见 tensors_truncated）。 */
  tensor_count: number;
  metadata_kv_count: number;
  file_size: number;
  /** 张量数据对齐（general.alignment 或默认 32）。 */
  alignment: number;
  /** 张量数据区起点（张量信息区结束位置按 alignment 上取整）。 */
  tensor_data_offset: number;
  /** 全部张量元素数之和——即模型参数量。 */
  total_parameters: number;
  architecture: string | null;
  name: string | null;
  /** general.file_type（量化方案 ID）。 */
  file_type: number | null;
  /** file_type 的可读量化名，如 Q4_K_M / BF16。 */
  quantization: string | null;
  tensor_types: GgufTypeCount[];
  metadata: GgufMetaEntry[];
  tensors: GgufTensorEntry[];
  /** 张量明细是否因上限被截断。 */
  tensors_truncated: boolean;
  parse_warnings: string[];
}

/** 解析镜像内文件的 GGUF 结构。非 GGUF 文件 reject，错误信息可直接展示。 */
export function fsGgufPreview(args: { imageId: ImageId; nodeId: NodeId }): Promise<GgufReport> {
  return invokeCmd<GgufReport>('fs_gguf_preview', args);
}
