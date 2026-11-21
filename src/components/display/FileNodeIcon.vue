<template>
  <svg
    class="file-node-icon"
    :class="kindClass"
    :width="size"
    :height="size"
    :viewBox="svg.viewBox"
    :fill="svg.fill"
    :data-icon="iconName"
    aria-hidden="true"
    focusable="false"
    v-html="svg.body"
  />
</template>

<script setup lang="ts">
// 文件/目录/分区节点的统一图标：目录树与文件列表共用这一份选型与配色，
// 两边不再各自维护。形状按文件名查图标表（utils/fileIcons），配色按扩展名
// 类别走主题 token；SVG 随包内联，绘制颜色统一换成 currentColor。
import { computed } from 'vue';
import { iconCategoryOf, type IconCategory } from '../../utils/fileCategory';
import { fileIconOf, NODE_ICON, type FileIconName } from '../../utils/fileIcons';
import { fileIconSvg } from './fileIconSvg';

const props = withDefaults(
  defineProps<{
    /** 节点 id：决定分区/逻辑卷与未分配空间等特殊形态。 */
    id: string;
    /** FsNode.kind：'dir' | 'file' | 'symlink' | 'unallocated'。 */
    kind: string;
    /** 文件名，按扩展名分类别。 */
    name: string;
    size?: number;
    /** 目录在树里已展开：画打开的文件夹。 */
    expanded?: boolean;
  }>(),
  { size: 14, expanded: false },
);

const isPartition = computed(() => /^part:\d+$/.test(props.id) || /^lvm:\d+:\d+$/.test(props.id));

// 普通 Linux 镜像中的 Docker Engine 虚拟视图：组节点与容器根使用容器
// 图标，容器内部仍按真实文件/目录类型渲染。
const isDockerGroup = computed(() => props.id === 'host-docker:');
const isDockerContainerRoot = computed(() => /^host-docker:[^:]+:$/.test(props.id));

// ReFS 文件快照伪文件（refsutil streamsnapshot）：用「历史/回拨」图标表达
// 它保存的是替换前的旧版本内容，名字带 `:`/`(快照)` 无从按扩展名识别。
const isSnapshot = computed(() => props.id.startsWith('refs-snap:'));

// NTFS 命名数据流伪文件（ADS）：用「分层」图标表达它是附在文件主体之外的
// 附加数据流，名字带 `:`/`(数据流)` 同样无从按扩展名识别。
const isAds = computed(() => props.id.startsWith('ntfs-ads:'));

// 未分配空间与分区间隙：虚线框表示不属于任何文件或分区的区域。
const isUnalloc = computed(
  () =>
    props.kind === 'unallocated' ||
    props.id.startsWith('diskgap:') ||
    props.id.startsWith('unalloc:'),
);

const category = computed<IconCategory>(() => {
  if (props.kind === 'dir') return 'other';
  const idx = props.name.lastIndexOf('.');
  const ext = idx <= 0 || idx === props.name.length - 1 ? '' : props.name.slice(idx + 1);
  return iconCategoryOf(ext.toLowerCase());
});

const iconName = computed<FileIconName>(() => {
  if (isDockerGroup.value || isDockerContainerRoot.value) return NODE_ICON.container;
  if (isSnapshot.value) return NODE_ICON.snapshot;
  if (isAds.value) return NODE_ICON.dataStream;
  if (isUnalloc.value) return NODE_ICON.unallocated;
  if (isPartition.value) return NODE_ICON.partition;
  if (props.kind === 'dir') return props.expanded ? NODE_ICON.folderOpen : NODE_ICON.folder;
  if (props.kind === 'symlink') return NODE_ICON.symlink;
  return fileIconOf(props.name, category.value);
});

const svg = computed(() => fileIconSvg(iconName.value));

const kindClass = computed(() => {
  if (isDockerGroup.value || isDockerContainerRoot.value) return 'kind-container';
  if (isSnapshot.value || isAds.value) return 'kind-snapshot';
  if (isPartition.value) return 'kind-dir';
  if (isUnalloc.value || props.kind !== 'file') return `kind-${props.kind}`;
  return `kind-file cat-${category.value}`;
});
</script>

<style scoped>
.file-node-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.file-node-icon.kind-dir {
  color: var(--brand-spark-core, var(--primary-color));
}

.file-node-icon.kind-container {
  color: var(--sev-info-fg);
}

/* ReFS 文件快照 / NTFS 命名数据流伪文件：用琥珀色图标标识。 */
.file-node-icon.kind-snapshot {
  color: var(--sev-snapshot-tag-fg);
}

/* 文件类别配色：全部走既有 token，三主题自动同步。 */
.file-node-icon.cat-image { color: var(--sev-success-fg); }
.file-node-icon.cat-video,
.file-node-icon.cat-audio,
.file-node-icon.cat-email { color: var(--sev-info-fg); }
.file-node-icon.cat-archive,
.file-node-icon.cat-database { color: var(--sev-warn-fg); }
.file-node-icon.cat-executable { color: var(--sev-critical-fg); }
.file-node-icon.cat-code { color: var(--accent-color); }
.file-node-icon.cat-pdf,
.file-node-icon.cat-document,
.file-node-icon.cat-spreadsheet { color: var(--primary-color); }
</style>
