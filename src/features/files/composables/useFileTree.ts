
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, reactive, ref, watch } from 'vue';
import { containerFsChildren as fsChildren } from './containerReads';
import { notifyError } from '../../../composables/useToast';
import { isHostDockerContainerRootId } from '../model/model';
import { fileTreePathFor, visibleFileTreeRows, type TreeNodeData, type TreeStackFrame } from '../model/fileTree';
import { useFileTreeMetadata } from './useFileTreeMetadata';
import type { Ref } from 'vue';
interface FileTreeOptions {
  imageId: () => string; pathStack: () => TreeStackFrame[]; caseId: () => string | null;
  hideEmptyDirs: Ref<boolean>; navigate: (stack: TreeStackFrame[]) => void; openFile: (stack: TreeStackFrame[], fileId: string) => void;
}
export function useFileTree(options: FileTreeOptions) {
  const TREE_FILE_CAP = 300;
  const MORE_PREFIX = '__tree_more__:';
  const hideEmptyDirs = options.hideEmptyDirs;
  let disposed = false; let navigationGeneration = 0;
  const nodes = reactive(new Map<string, TreeNodeData>());
  const childrenOf = reactive(new Map<string, string[]>());
  // id → 该目录整棵子树是否「无任何文件」（true=只剩空目录，可隐藏；false=子树里
  // 有文件，必显示）。仅含快照能判定的目录；未收录的 id 取不到值，过滤时退回按
  // childCount===0 的「直接为空」判断。随 hideEmptyDirs 开启时按需经后端回填。
  const emptySubtreeOf = reactive(new Map<string, boolean>());
  const rootIds = ref<string[]>([]);
  const rootLoaded = ref(false);

  // Bumped whenever the image swaps so in-flight count probes know to drop
  // their results instead of writing into the new image's tree state.
  let loadGeneration = 0;

  const activeId = computed(() => {
    const last = options.pathStack()[options.pathStack().length - 1];
    return last && last.id !== 'root' ? last.id : null;
  });

  // 在树里点选的文件叶子（区别于 activeId——后者由列表导航栈推导，只会是目录）。
  // 点文件时列表导航到其所在目录，activeId 落在父目录上，单独记下叶子 id 让被点
  // 的文件本身也保持高亮，符合资源管理器式预期。
  const selectedLeafId = ref<string | null>(null);


  const current = (generation: number) => !disposed && generation === loadGeneration;
  const { fetchDriveLetters, fetchChildCounts, fetchEmptySubtrees } = useFileTreeMetadata({
    imageId: options.imageId, current,
    publishCounts: counts => { for (const [id, count] of Object.entries(counts)) { const node = nodes.get(id); if (node) node.childCount = count; } },
    publishEmpty: values => { for (const [id, empty] of Object.entries(values)) emptySubtreeOf.set(id, empty); },
    publishLetters: letters => { for (const { id, letter } of letters) { const node = nodes.get(id); if (node) node.driveLetter = letter; } },
  });
  const visibleRows = computed(() => visibleFileTreeRows(nodes, childrenOf, emptySubtreeOf, rootIds.value, hideEmptyDirs.value));
  const pathFor = (id: string) => fileTreePathFor(nodes, id);
  const rootNodes = computed(() => rootIds.value);
  const dockerFilesystemLoading = computed(() =>
    Array.from(nodes.values()).some(
      node => node.loading && isHostDockerContainerRootId(node.id),
    ),
  );

  async function loadRoot() {
    loadGeneration += 1;
    const gen = loadGeneration;
    rootLoaded.value = false;
    nodes.clear();
    childrenOf.clear();
    emptySubtreeOf.clear();
    rootIds.value = [];
    if (!options.imageId()) return;
    try {
      const fetched = await fsChildren({
        imageId: options.imageId(),
        parentId: 'root',
      });
      if (!current(gen)) return;
      const ids: string[] = [];
      const dirIds: string[] = [];
      for (const n of fetched) {
        if (n.deleted) continue;
        ids.push(n.id);
        if (n.kind === 'dir') dirIds.push(n.id);
        nodes.set(n.id, {
          id: n.id,
          name: n.name,
          parentId: null,
          kind: n.kind,
          expanded: false,
          loaded: false,
          loading: false,
          childCount: null,
          driveLetter: null,
        });
      }
      rootIds.value = ids;
      // Eagerly count children of every root-level partition so the user
      // sees "(N)" next to each before they ever expand.
      void fetchChildCounts(dirIds, gen);
      // Windows 镜像：异步还原各卷原始盘符并叠加到分区根节点。放在根列表提交
      // 之后——SYSTEM 配置单元可达数十 MiB，绝不能阻塞首屏。
      void fetchDriveLetters(gen);
    } catch (error) {
      if (current(gen)) notifyError(tr("读取目录树失败"), String(error));
    } finally {
      if (current(gen)) rootLoaded.value = true;
    }
  }

  async function loadChildren(parentId: string) {
    const parent = nodes.get(parentId);
    if (!parent) return;
    if (parent.loaded || parent.loading) return;
    const gen = loadGeneration;
    parent.loading = true;
    try {
      const fetched = await fsChildren({
        imageId: options.imageId(),
        parentId,
      });
      if (!current(gen)) return;
      const ids: string[] = [];
      const dirIds: string[] = [];
      let fileCount = 0;
      let skippedFiles = 0;
      for (const n of fetched) {
        if (n.deleted) continue;
        if (n.kind !== 'dir') {
          fileCount += 1;
          if (fileCount > TREE_FILE_CAP) {
            skippedFiles += 1;
            continue;
          }
        } else {
          dirIds.push(n.id);
        }
        ids.push(n.id);
        const existing = nodes.get(n.id);
        if (existing) {
          // 同一节点 id 可能已在别处注册过（如旧快照在分区根重复列出的子卷
          // 根，或 syncToPath 补的占位节点）。保留其展开/已加载状态，但名称
          // 与类型以当前目录的真实列表为准——否则这里会渲染出过期的条目。
          existing.name = n.name;
          existing.kind = n.kind;
        } else {
          nodes.set(n.id, {
            id: n.id,
            name: n.name,
            parentId,
            kind: n.kind,
            expanded: false,
            loaded: false,
            loading: false,
            childCount: null,
            driveLetter: null,
          });
        }
      }
      if (skippedFiles > 0) {
        const moreId = `${MORE_PREFIX}${parentId}`;
        ids.push(moreId);
        nodes.set(moreId, {
          id: moreId,
          name: tr("… 其余 {0} 项（见右侧列表）", { 0: skippedFiles }),
          parentId,
          kind: 'file',
          expanded: false,
          loaded: true,
          loading: false,
          childCount: null,
          driveLetter: null,
        });
      }
      // Fire-and-forget background fetch of grandchild counts for each
      // newly-visible subdir, so the next level's "(N)" badges show up
      // without the user having to expand each one.
      void fetchChildCounts(dirIds, gen);
      // 「隐藏空目录」开着时，先判定这批子目录再公布 childrenOf。这样展开节点
      // 时只出现最终结果，不会先闪过随后被过滤掉的空目录。
      if (hideEmptyDirs.value) await fetchEmptySubtrees(dirIds, gen);
      if (!current(gen)) return;
      childrenOf.set(parentId, ids);
      parent.loaded = true;
    } catch (_err) {
      // 多半是上锁的 BitLocker 分区、不支持的文件系统，或一次瞬时 IPC 失败。
      // 故意不置 loaded / 不写 childrenOf：留着 loaded=false，下次展开或
      // syncToPath 路过时整体重试。曾经这里把节点冻结成「已加载且为空」，
      // 之后 syncToPath 只会往空表里补访问过的目录占位——表现为该目录在树里
      // 永远只有子目录、没有文件。
    } finally {
      parent.loading = false;
    }
  }

  async function toggleExpand(row: TreeNodeData) {
    if (disposed || row.kind !== 'dir') return;
    const captured = loadGeneration;
    if (!row.expanded) {
      if (!row.loaded) await loadChildren(row.id);
      if (!current(captured)) return;
      row.expanded = true;
      // 子目录此前可能在「隐藏空目录」关闭时就已加载过（loadChildren 会因 loaded
      // 而早退、不补判定）。展开时为还没判定的子目录补一次子树判定，确保「里面
      // 全是空目录」的子目录也能在这一层被隐去。fetchEmptySubtrees 自身幂等。
      if (hideEmptyDirs.value) {
        const kids = childrenOf.get(row.id);
        if (kids) {
          const need = kids.filter(id => {
            const child = nodes.get(id);
            return child?.kind === 'dir' && !emptySubtreeOf.has(id);
          });
          if (need.length > 0) await fetchEmptySubtrees(need, loadGeneration);
        }
      }
    } else {
      row.expanded = false;
    }
  }

  function onRowClick(row: TreeNodeData) {
    if (row.kind === 'dir') {
      selectedLeafId.value = null;
      options.navigate(pathFor(row.id));
      return;
    }
    // 叶子（文件 / 符号链接 / 未分配 / 合成的「…其余 N 项」）无法作为目录进入，
    // 一律先解析其所在目录的导航栈。
    const dirStack = row.parentId != null ? pathFor(row.parentId) : [{ id: 'root', name: '/' }];
    // 「…其余 N 项」是折叠占位、不是真实文件：只导航到所在目录，让右侧列表
    // 展开被折叠的条目。
    if (row.id.startsWith(MORE_PREFIX)) {
      selectedLeafId.value = null;
      options.navigate(dirStack);
      return;
    }
    // 真实文件：导航到所在目录并打开该文件的详情面板，与在右侧列表里单击该
    // 文件一致；同时高亮该叶子。
    selectedLeafId.value = row.id;
    options.openFile(dirStack, row.id);
  }

  // When the parent's path changes (user navigated via the list / breadcrumb),
  // walk down the new stack and expand each ancestor + load its children so
  // the tree mirrors where the list is.
  async function syncToPath() {
    if (disposed || !rootLoaded.value) return;
    const captured = loadGeneration;
    const navigation = ++navigationGeneration;
    const stack = [...options.pathStack()];
    for (let i = 1; i < stack.length; i++) {
      if (!current(captured) || navigation !== navigationGeneration) return;
      const frame = stack[i];
      if (!frame || frame.id === 'root') continue;
      // The frame may not be in the tree yet if the user descended via the
      // list without ever expanding here. Add it under its expected parent
      // (the previous frame in the stack) so subsequent navigation has a
      // home for it.
      if (!nodes.has(frame.id)) {
        const parentFrame = stack[i - 1];
        const parentId = parentFrame && parentFrame.id !== 'root' ? parentFrame.id : null;
        nodes.set(frame.id, {
          id: frame.id,
          name: frame.name,
          parentId,
          kind: 'dir',
          expanded: false,
          loaded: false,
          loading: false,
          childCount: null,
          driveLetter: null,
        });
        if (parentId) {
          const kids = childrenOf.get(parentId);
          if (kids && !kids.includes(frame.id)) {
            kids.push(frame.id);
          }
        } else if (!rootIds.value.includes(frame.id)) {
          rootIds.value.push(frame.id);
        }
      }
      const node = nodes.get(frame.id);
      if (!node) continue;
      // 展开路径上的每个目录，包括当前所在目录——让目录树跟随并展开到当前位置，
      // 当前目录在树里也展开其子目录，便于从树继续下钻（与右侧列表内容重叠是有意
      // 的，符合资源管理器式目录树的预期）。
      if (!node.loaded) await loadChildren(node.id);
      if (!current(captured) || navigation !== navigationGeneration) return;
      node.expanded = true;
    }
  }

  watch(
    () => options.imageId(),
    () => {
      void loadRoot().then(syncToPath);
    },
    { immediate: true, flush: 'sync' },
  );

  watch(
    () => options.pathStack(),
    () => {
      // 列表被导航到别的目录后，树里点选的文件已不在当前目录，撤销其高亮；
      // open-file 把列表导到该文件所在目录时父目录与 activeId 一致，保留高亮。
      if (selectedLeafId.value) {
        const leaf = nodes.get(selectedLeafId.value);
        if ((leaf?.parentId ?? null) !== activeId.value) selectedLeafId.value = null;
      }
      void syncToPath();
    },
    { deep: true },
  );

  // 镜像在无案件时装载（盘符已实时解出但未落盘），之后才打开/创建案件：重解析
  // 一次让后端把盘符持久化到案件目录——若已缓存则是一次廉价命中。镜像随案件
  // 打开而重载的常见路径由上面的 imageId watch 覆盖，这里只补「案件后到」的缺口。
  watch(
    options.caseId,
    caseId => {
      if (caseId && rootLoaded.value) void fetchDriveLetters(loadGeneration);
    },
  );

  // 打开「隐藏空目录」时，对当前可见的子级目录补齐判定，让过滤立刻生效——否则
  // 要等用户逐个展开时的后台查询才会把空目录隐去。子树判定（emptySubtreeOf）覆盖
  // 「里面全是空目录」的递归情形；childCount 作直接为空的兜底。回填后 `visibleRows`
  // 自动重算。已判定的目录不会触发任何额外 IPC。
  watch(hideEmptyDirs, on => {
    if (!on) return;
    const gen = loadGeneration;
    // 不能从 visibleRows 取目标：开关刚打开时，未知目录已按过滤规则暂时隐藏，
    // 再从过滤后的行里取会漏掉它们，导致它们永远没有机会完成判定。
    const dirs = Array.from(nodes.values()).filter(
      node => node.parentId !== null && node.kind === 'dir',
    );
    const needCount = dirs.filter(node => node.childCount == null).map(node => node.id);
    const needEmpty = dirs
      .filter(node => !isHostDockerContainerRootId(node.id) && !emptySubtreeOf.has(node.id))
      .map(node => node.id);
    if (needCount.length > 0) void fetchChildCounts(needCount, gen);
    if (needEmpty.length > 0) void fetchEmptySubtrees(needEmpty, gen);
  });

  onScopeDispose(() => { disposed = true; loadGeneration++; navigationGeneration++; });
  return { activeId, selectedLeafId, visibleRows, rootNodes, rootLoaded, dockerFilesystemLoading, onRowClick, toggleExpand };
}
