
import { tr } from '../../../composables/useLocale';
import { fsDigest, fsIdentifyType, type FsTypeIdent, type HashAlgo, type StoredDigest } from '../../../lib/ipc/fs/derived';
import { pushPersistentToast } from '../../../composables/useToast';
import { broadcastPreview } from '../../preview/api';
import type { DiskFsNode as FsNode } from '../model/model';
import { HASH_ALGO_LABEL } from '../model/derivedData';
import type { useFileDerivedData } from './useFileDerivedData';

/** Batch calculation actions use the panel's existing cache and captured evidence. */
export function useFileDerivedActions(options: {
  imageId: () => string;
  data: ReturnType<typeof useFileDerivedData>;
  revealColumns: (keys: readonly string[]) => void;
}) {
  const { digestCache, ftypeCache, persistDigests, persistFtypes } = options.data;

  async function digestCheckedFiles(files: FsNode[], algos: HashAlgo[]) {
    let cancelled = false;
    const gen = options.data.generation();
    const imageId = options.imageId();
    const algoLabel = algos.map(a => HASH_ALGO_LABEL[a]).join('/');
    const toast = pushPersistentToast({
      kind: 'info',
      title: tr("计算 {0} 中…（{1} 个）", { 0: algoLabel, 1: files.length }),
      detail: tr("准备中"),
      action: {
        get label() { return tr("取消"); },
        onClick: () => {
          cancelled = true;
          toast.update({ action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
        },
      },
    });
    let ok = 0;
    let failed = 0;
    let lastErr = '';
    // 本轮新算出的条目，循环结束一次性落盘（取消/切镜像前算出的也是有效结果）。
    const computed: Record<string, StoredDigest> = {};
    for (const f of files) {
      if (cancelled) break;
      // 切镜像后节点 id 指向另一块盘的文件，立即停手，别把旧镜像的哈希
      // 写进新镜像的缓存。
      if (gen !== options.data.generation()) {
        cancelled = true;
        break;
      }
      const cached = digestCache.get(f.id);
      const missing = algos.filter(a => !cached?.[a]);
      if (missing.length === 0) {
        ok++;
        continue;
      }
      try {
        const d = await fsDigest({ imageId, nodeId: f.id, algos: missing });
        if (gen !== options.data.generation()) {
          cancelled = true;
          break;
        }
        const merged: StoredDigest = { ...digestCache.get(f.id) };
        for (const a of missing) {
          if (d[a]) merged[a] = d[a];
        }
        digestCache.set(f.id, merged);
        computed[f.id] = merged;
        broadcastPreview({ kind: 'digest', imageId, nodeId: f.id, digest: merged });
        ok++;
      } catch (err) {
        failed++;
        lastErr = String(err);
      }
      toast.update({ detail: `${ok + failed}/${files.length}${failed > 0 ? tr(" · 失败 {0}", { 0: failed }) : ''}` });
    }
    persistDigests(imageId, computed);
    const parts = [tr("{0} 个成功", { 0: ok })];
    if (failed > 0) parts.push(tr("失败 {0}（{1}）", { 0: failed, 1: lastErr }));
    if (cancelled) parts.push(tr("未计算 {0}", { 0: files.length - ok - failed }));
    toast.update({
      kind: failed > 0 ? 'error' : cancelled ? 'info' : 'success',
      title: cancelled ? tr("哈希计算已取消") : failed > 0 ? tr("哈希计算完成（部分失败）") : tr("哈希计算完成"),
      detail: tr("{0} · 结果已回填 {1} 列", { 0: parts.join(' · '), 1: algoLabel }),
      action: undefined,
    });
    toast.dismiss(failed > 0 ? 6000 : 3200);
    // 算完自动把对应哈希列亮出来，省得用户去设置列里找。
    if (ok > 0) {
      options.revealColumns(algos);
    }
  }

  async function identifyCheckedFiles(files: FsNode[]) {
    let cancelled = false;
    const gen = options.data.generation();
    const imageId = options.imageId();
    const toast = pushPersistentToast({
      kind: 'info',
      title: tr("识别文件类型中…（{0} 个）", { 0: files.length }),
      detail: tr("准备中"),
      action: {
        get label() { return tr("取消"); },
        onClick: () => {
          cancelled = true;
          toast.update({ action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
        },
      },
    });
    let ok = 0;
    let failed = 0;
    let lastErr = '';
    let lastResult: FsTypeIdent | null = null;
    // 本轮新识别出的条目，循环结束一次性落盘（取消/切镜像前识别出的也有效）。
    const identified: Record<string, FsTypeIdent> = {};
    for (const f of files) {
      if (cancelled) break;
      if (gen !== options.data.generation()) {
        cancelled = true;
        break;
      }
      const cached = ftypeCache.get(f.id);
      if (cached) {
        lastResult = cached;
        ok++;
        continue;
      }
      try {
        const t = await fsIdentifyType({ imageId, nodeId: f.id });
        if (gen !== options.data.generation()) {
          cancelled = true;
          break;
        }
        ftypeCache.set(f.id, t);
        lastResult = t;
        identified[f.id] = t;
        broadcastPreview({ kind: 'ident', imageId, nodeId: f.id, ident: t });
        ok++;
      } catch (err) {
        failed++;
        lastErr = String(err);
      }
      toast.update({ detail: `${ok + failed}/${files.length}${failed > 0 ? tr(" · 失败 {0}", { 0: failed }) : ''}` });
    }
    // 识别结果落盘（开案件后跨会话复用，省得重跑推理）；无案件是 no-op。
    persistFtypes(imageId, identified);
    // 单文件直接把结论写进通知，不用去表里找。
    const single = files.length === 1 && lastResult !== null;
    const parts = single
      ? [`${lastResult!.label}（${lastResult!.mime_type}）`]
      : [tr("{0} 个成功", { 0: ok })];
    if (failed > 0) parts.push(tr("失败 {0}（{1}）", { 0: failed, 1: lastErr }));
    if (cancelled) parts.push(tr("未识别 {0}", { 0: files.length - ok - failed }));
    toast.update({
      kind: failed > 0 ? 'error' : cancelled ? 'info' : 'success',
      title: cancelled ? tr("类型识别已取消") : failed > 0 ? tr("类型识别完成（部分失败）") : tr("类型识别完成"),
      detail: tr("{0} · 结果已回填「识别类型」列", { 0: parts.join(' · ') }),
      action: undefined,
    });
    toast.dismiss(failed > 0 ? 6000 : 3200);
    if (ok > 0) options.revealColumns(['ftype']);
  }

  return { digestCheckedFiles, identifyCheckedFiles };
}
