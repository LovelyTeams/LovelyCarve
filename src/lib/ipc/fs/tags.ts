import { invokeCmd } from '../client';
import type { ImageId, NodeId } from '../ids';



/** 读取案件目录中本镜像已持久化的文件标签（节点 id → 标签文案）。无案件 /
 *  尚未打标 / 存档损坏时返回空表。 */
export function fsTagsLoad(args: { imageId: ImageId }): Promise<Record<string, string>> {
  return invokeCmd<Record<string, string>>('fs_tags_load', args);
}

/** 只读取指定节点集合的标签。用于目录页/批次水合，避免整镜像标签表通过
 *  IPC 一次性返回；后端会限制单次节点数。 */
export function fsTagsLoadNodes(args: {
  imageId: ImageId;
  nodeIds: NodeId[];
}): Promise<Record<string, string>> {
  return invokeCmd<Record<string, string>>('fs_tags_load_nodes', args);
}

/** 增量写入标签变更：`upserts` 设置/覆盖节点标签，`deleteNodeIds` 清除节点
 *  标签。无案件时返回 false；写入失败才会 reject。 */
export function fsTagsApplyChanges(args: {
  imageId: ImageId;
  upserts: Record<string, string>;
  deleteNodeIds: string[];
}): Promise<boolean> {
  return invokeCmd<boolean>('fs_tags_apply_changes', args);
}

/** 把本镜像的完整标签表写进案件目录（整表替换——清除标签靠"不在表里"生效，
 *  空表会删掉存档文件）。保留给完整迁移/兼容场景；普通 UI 打标应优先使用
 *  `fsTagsApplyChanges`，避免不完整本地表覆盖案件标签。 */
export function fsTagsSave(args: {
  imageId: ImageId;
  entries: Record<string, string>;
}): Promise<boolean> {
  return invokeCmd<boolean>('fs_tags_save', args);
}
