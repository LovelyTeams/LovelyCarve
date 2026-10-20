import { invokeCmd } from '../client';
import type { ReadFaultReport } from './export';



/** 与后端 `core::hash::HashAlgo` 锁同步的算法字面量。 */
export type HashAlgo = 'md5' | 'sha1' | 'sha256' | 'sm3';

/** 单文件哈希结果：只有请求过的算法是非 null。 */
export interface FsDigest extends ReadFaultReport {
  md5: string | null;
  sha1: string | null;
  sha256: string | null;
  sm3: string | null;
  bytes_hashed: number;
  truncated: boolean;
}

/** 按需计算一个文件的内容哈希；`algos` 选要算的算法（结果只回填这些字段）。 */
export function fsDigest(args: { imageId: string; nodeId: string; algos: HashAlgo[] }): Promise<FsDigest> {
  return invokeCmd<FsDigest>('fs_digest', args);
}

/** AI 文件类型识别结果（后端 `core::magika_ident::TypeIdentReport`）。 */
export interface FsTypeIdent {
  /** magika 类型标签，如 `pdf` / `elf` / `python`。 */
  label: string;
  mime_type: string;
  /** 大类分组：document / executable / code / archive … */
  group: string;
  description: string;
  extensions: string[];
  is_text: boolean;
  /** 置信度 0~1；规则直判（空文件等）恒为 1。 */
  score: number;
  /** true = 模型推理；false = 规则直判。 */
  inferred: boolean;
  file_len: number;
}

/** 按文件内容识别类型（Google Magika 本地推理），与扩展名无关——适合
 *  发现改后缀伪装的文件。只采样文件头尾各 4 KiB，大文件也很快。 */
export function fsIdentifyType(args: { imageId: string; nodeId: string }): Promise<FsTypeIdent> {
  return invokeCmd<FsTypeIdent>('fs_identify_type', args);
}

/** 读取案件目录中本镜像已持久化的类型识别（节点 id → 识别结果）。无案件 /
 *  尚未识别 / 存档损坏时返回空表，浏览不受影响。 */
export function fsFtypeLoad(args: { imageId: string }): Promise<Record<string, FsTypeIdent>> {
  return invokeCmd<Record<string, FsTypeIdent>>('fs_ftype_load', args);
}

/** 只读取指定节点集合的类型识别结果。用于目录页/流式批次水合，避免整镜像
 *  类型识别表通过 IPC 一次性返回；后端会限制单次节点数。 */
export function fsFtypeLoadNodes(args: {
  imageId: string;
  nodeIds: string[];
}): Promise<Record<string, FsTypeIdent>> {
  return invokeCmd<Record<string, FsTypeIdent>>('fs_ftype_load_nodes', args);
}

/** 把新识别出的类型合并进案件目录的持久化存档（按节点整条替换——再识别会
 *  覆盖该节点，其他节点保留，跨批次/会话累积）。无案件时返回 false（结果只
 *  活在本会话）；写入失败才会 reject。 */
export function fsFtypePersist(args: {
  imageId: string;
  entries: Record<string, FsTypeIdent>;
}): Promise<boolean> {
  return invokeCmd<boolean>('fs_ftype_persist', args);
}

/** 持久化哈希条目：按节点 id 入库，算法字段缺省表示尚未计算（后端按字段
 *  合并，后算的算法不会冲掉先算的）。 */
export interface StoredDigest {
  md5?: string | null;
  sha1?: string | null;
  sha256?: string | null;
  sm3?: string | null;
}

/** 读取案件目录中本镜像已持久化的哈希（节点 id → 摘要）。无案件 / 尚未
 *  持久化 / 存档损坏时返回空表，浏览不受影响。 */
export function fsDigestLoad(args: { imageId: string }): Promise<Record<string, StoredDigest>> {
  return invokeCmd<Record<string, StoredDigest>>('fs_digest_load', args);
}

/** 只读取指定节点集合的哈希。用于目录页/流式批次水合，避免整镜像摘要表
 *  通过 IPC 一次性返回；后端会限制单次节点数。 */
export function fsDigestLoadNodes(args: {
  imageId: string;
  nodeIds: string[];
}): Promise<Record<string, StoredDigest>> {
  return invokeCmd<Record<string, StoredDigest>>('fs_digest_load_nodes', args);
}

/** 把新计算的哈希合并进案件目录的持久化存档。无案件时返回 false（结果只
 *  活在本会话）；写入失败才会 reject。 */
export function fsDigestPersist(args: {
  imageId: string;
  entries: Record<string, StoredDigest>;
}): Promise<boolean> {
  return invokeCmd<boolean>('fs_digest_persist', args);
}
