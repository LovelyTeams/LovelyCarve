
import { tr } from '../../../composables/useLocale';
import { computed, reactive } from 'vue';
import { notifyError, notifySuccess } from '../../../composables/useToast';
import { tsvEscape } from '../../../utils/exportFormat';
import { formatTimestamp } from '../../../utils/time';
import { kindLabel, type DiskFsNode as FsNode } from '../model/model';
import type { HashAlgo } from '../../../lib/ipc/fs/derived';
import type { FileTag } from '../model/tags';
import type { useFileSelection } from './useFileSelection';

/** Visible selection determines clipboard/export/analysis/tag targets consistently. */
export function useFileActions(options: {
  imageId: () => string; rows: () => FsNode[]; node: () => FsNode | null;
  currentPath: () => string; fullPathOf: (node: FsNode) => string;
  selection: ReturnType<typeof useFileSelection>;
  closeMenu: () => void;
  exportFile: (node: FsNode | null, files: FsNode[]) => Promise<void>;
  exportDirectory: (node: FsNode | null, directories: FsNode[]) => Promise<void>;
  digest: (files: FsNode[], algorithms: HashAlgo[]) => Promise<void>;
  identify: (files: FsNode[]) => Promise<void>;
  setTag: (imageId: string, ids: string[], tag: FileTag | null) => void;
  revealTagColumn: () => void;
}) {
  const { checkedIds, checkedStats, checkedFiles, checkedDirs } = options.selection;
  const { fullPathOf } = options;
  function copyMenuNodes(): FsNode[] {
    const out: FsNode[] = [];
    const seen = new Set<string>();
    for (const n of options.rows()) {
      if (!checkedIds.value.has(n.id) || seen.has(n.id)) continue;
      seen.add(n.id);
      out.push(n);
    }
    const node = options.node();
    if (out.length === 0 && node) out.push(node);
    return out;
  }

  const copyCount = computed(() => {
    const checked = checkedStats.value.count;
    return checked > 0 ? checked : options.node() ? 1 : 0;
  });

  async function copyText(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      notifySuccess(tr("已复制"), what);
    } catch (err) {
      notifyError(tr("复制失败"), String(err));
    }
  }

  function copyCurrentPath() {
    void copyText(options.currentPath(), tr("当前目录完整路径"));
  }

  function onCopyNames() {
    const nodes = copyMenuNodes();
    options.closeMenu();
    if (nodes.length === 0) return;
    void copyText(nodes.map(n => n.name).join('\n'), tr("{0} 个名称", { 0: nodes.length }));
  }

  function onCopyPaths() {
    const nodes = copyMenuNodes();
    options.closeMenu();
    if (nodes.length === 0) return;
    void copyText(nodes.map(n => fullPathOf(n)).join('\n'), tr("{0} 条完整路径", { 0: nodes.length }));
  }

  // 证据级信息行：制表符分隔，多行带表头，可直接粘进 Excel / 笔录。检材文件名不可信，每个单元格
  // 经 tsvEscape 中和公式，名字里的制表符与换行显示为 ␉ ␊，拆不出新的单元格或行。
  function onCopyForensic() {
    const nodes = copyMenuNodes();
    options.closeMenu();
    if (nodes.length === 0) return;
    const header = tr("全路径\t类型\t大小（字节）\t创建时间\t修改时间\t访问时间\t元数据地址\t已删除");
    const lines = nodes.map(n =>
      [
        fullPathOf(n),
        kindLabel(n, tr),
        n.kind === 'dir' ? '' : String(n.size),
        formatTimestamp(n.timestamps?.created || n.timestamps?.birth),
        formatTimestamp(n.timestamps?.modified),
        formatTimestamp(n.timestamps?.accessed),
        String(n.inode ?? ''),
        n.deleted ? tr("是") : tr("否"),
      ].map(tsvEscape).join('\t'),
    );
    void copyText([header, ...lines].join('\n'), tr("{0} 行取证信息", { 0: nodes.length }));
  }

  const canExportFile = computed(() => checkedFiles.value.length > 0 || options.node()?.kind === 'file');
  const canExportDir = computed(() => checkedDirs.value.length > 0 || options.node()?.kind === 'dir');
  const canHash = computed(() => checkedFiles.value.length > 0 || options.node()?.kind === 'file');
  const canIdentify = computed(() => checkedFiles.value.length > 0 || options.node()?.kind === 'file');
  /** 插件分析任务按光标行的单个文件提交(任务表按 (function, 检材) 单实例去重)。 */
  const canPluginTask = computed(() => options.node()?.kind === 'file');

  const pluginTaskDialog = reactive<{ visible: boolean; node: FsNode | null }>({ visible: false, node: null });

  function onPluginTask() {
    const node = options.node();
    options.closeMenu();
    if (!node || node.kind !== 'file') return;
    pluginTaskDialog.node = node;
    pluginTaskDialog.visible = true;
  }

  async function onExportFile() {
    const node = options.node();
    options.closeMenu();
    await options.exportFile(node, checkedFiles.value);
  }

  async function onExportDir() {
    const node = options.node();
    options.closeMenu();
    await options.exportDirectory(node, checkedDirs.value);
  }

  async function onComputeHash(algos: HashAlgo[]) {
    const node = options.node();
    options.closeMenu();
    // 有勾选按勾选集，没有按光标行；单文件与批量同一条后台通道——进度 toast
    // 可取消，算完右下角通知并回填哈希列/详情面板，不再弹阻塞式弹窗。
    const files = checkedFiles.value;
    if (files.length > 0) {
      await options.digest(files, algos);
      return;
    }
    if (!node || node.kind !== 'file') return;
    await options.digest([node], algos);
  }

  // 批量哈希：逐个 fsDigest 勾选文件（带进度 toast 与取消），按所选算法回填
  // 对应哈希列。节点已算过的算法直接复用缓存，只补算缺的，不重复读盘。

  async function onIdentifyType() {
    const node = options.node();
    options.closeMenu();
    // 目标选择与哈希一致：有勾选按勾选集，没有按光标行。
    const files = checkedFiles.value;
    if (files.length > 0) {
      await options.identify(files);
      return;
    }
    if (!node || node.kind !== 'file') return;
    await options.identify([node]);
  }

  // 批量类型识别：逐个 fsIdentifyType（带进度 toast 与取消），结果回填
  // 「识别类型」列。已识别过的节点直接复用缓存。复用 hashGeneration 做
  // 镜像切换的代际守卫——清缓存的时机和哈希完全一致。

  function onTagPick(tag: FileTag | null) {
    const nodes = copyMenuNodes();
    options.closeMenu();
    if (nodes.length === 0) return;
    options.setTag(
      options.imageId(),
      nodes.map(n => n.id),
      tag,
    );
    if (tag) {
      options.revealTagColumn();
      notifySuccess(tr("已添加标签"), tr("{0} 项 → {1}", { 0: nodes.length, 1: tag }));
    } else {
      notifySuccess(tr("已清除标签"), tr("{0} 项", { 0: nodes.length }));
    }
  }

  return { copyCount, copyCurrentPath, onCopyNames, onCopyPaths, onCopyForensic,
    canExportFile, canExportDir, canHash, canIdentify, canPluginTask, pluginTaskDialog, onPluginTask,
    onExportFile, onExportDir, onComputeHash, onIdentifyType, onTagPick };
}
