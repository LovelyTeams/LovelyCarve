
import { tr } from '../../../composables/useLocale';
import { computed, reactive } from 'vue';
import { copyText } from '../../../composables/clipboard';
import { notifySuccess } from '../../../composables/useToast';
import { tsvEscape } from '../../../utils/exportFormat';
import { formatTimestamp } from '../../../utils/time';
import { kindLabel, type DiskFsNode as FsNode } from '../model/model';
import type { HashAlgo } from '../../../lib/ipc/fs/derived';
import type { FileTag } from '../model/tags';
import type { useFileSelection } from './useFileSelection';

/** What the row menu's batch actions act on: every visible checked row when it
 *  was opened on a checked row, otherwise only the row it was opened on. */
export type FileMenuScope = 'checked' | 'row';

/** The row menu's targets drive its clipboard, export, analysis and tag actions alike. */
export function useFileActions(options: {
  imageId: () => string; rows: () => FsNode[]; node: () => FsNode | null; scope: () => FileMenuScope;
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
  const { checkedIds, checkedFiles, checkedDirs } = options.selection;
  const { fullPathOf } = options;

  const targets = computed<FsNode[]>(() => {
    const node = options.node();
    if (options.scope() === 'row') return node ? [node] : [];
    const out: FsNode[] = [];
    const seen = new Set<string>();
    for (const n of options.rows()) {
      if (!checkedIds.value.has(n.id) || seen.has(n.id)) continue;
      seen.add(n.id);
      out.push(n);
    }
    return out;
  });
  const targetFiles = computed(() => options.scope() === 'row' ? targets.value.filter(n => n.kind === 'file') : checkedFiles.value);
  const targetDirs = computed(() => options.scope() === 'row' ? targets.value.filter(n => n.kind === 'dir') : checkedDirs.value);
  /** Counts for the menu heading and labels. */
  const targetSummary = computed(() => ({
    count: targets.value.length,
    files: targetFiles.value.length,
    dirs: targetDirs.value.length,
    bytes: targetFiles.value.reduce((sum, n) => sum + n.size, 0),
  }));

  function copyCurrentPath() {
    void copyText(options.currentPath(), tr("当前目录完整路径"));
  }

  function onCopyNames() {
    const nodes = targets.value;
    options.closeMenu();
    if (nodes.length === 0) return;
    void copyText(nodes.map(n => n.name).join('\n'), tr("{0} 个名称", { 0: nodes.length }));
  }

  function onCopyPaths() {
    const nodes = targets.value;
    options.closeMenu();
    if (nodes.length === 0) return;
    void copyText(nodes.map(n => fullPathOf(n)).join('\n'), tr("{0} 条完整路径", { 0: nodes.length }));
  }

  // 证据级信息行：制表符分隔，多行带表头，可直接粘进 Excel / 笔录。检材文件名不可信，每个单元格
  // 经 tsvEscape 中和公式，名字里的制表符与换行显示为 ␉ ␊，拆不出新的单元格或行。
  function onCopyForensic() {
    const nodes = targets.value;
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

  const canExportFile = computed(() => targetFiles.value.length > 0);
  const canExportDir = computed(() => targetDirs.value.length > 0);
  const canHash = computed(() => targetFiles.value.length > 0);
  const canIdentify = computed(() => targetFiles.value.length > 0);
  /** 插件分析任务一次提交一个文件(任务表按 (function, 检材) 单实例去重)：
   *  右键的文件，且它就是唯一的目标。 */
  const canPluginTask = computed(() => options.node()?.kind === 'file' && targets.value.length === 1);

  const pluginTaskDialog = reactive<{ visible: boolean; node: FsNode | null }>({ visible: false, node: null });

  function onPluginTask() {
    const node = options.node();
    options.closeMenu();
    if (!node || node.kind !== 'file' || !canPluginTask.value) return;
    pluginTaskDialog.node = node;
    pluginTaskDialog.visible = true;
  }

  // 一个目标走单项导出，多个走批量导出。
  async function onExportFile() {
    const files = targetFiles.value;
    options.closeMenu();
    await options.exportFile(null, files);
  }

  async function onExportDir() {
    const directories = targetDirs.value;
    options.closeMenu();
    await options.exportDirectory(null, directories);
  }

  // 单文件与批量同一条后台通道：进度 toast 可取消，算完右下角通知并回填
  // 哈希列 / 详情面板；已算过的算法直接复用缓存，只补算缺的，不重复读盘。
  async function onComputeHash(algos: HashAlgo[]) {
    const files = targetFiles.value;
    options.closeMenu();
    if (files.length > 0) await options.digest(files, algos);
  }

  // 类型识别同理：结果回填「识别类型」列，已识别过的节点直接复用缓存。
  async function onIdentifyType() {
    const files = targetFiles.value;
    options.closeMenu();
    if (files.length > 0) await options.identify(files);
  }

  function onTagPick(tag: FileTag | null) {
    const nodes = targets.value;
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

  return { targetSummary, copyCurrentPath, onCopyNames, onCopyPaths, onCopyForensic,
    canExportFile, canExportDir, canHash, canIdentify, canPluginTask, pluginTaskDialog, onPluginTask,
    onExportFile, onExportDir, onComputeHash, onIdentifyType, onTagPick };
}
