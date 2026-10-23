<template>
<footer class="ui-footbar treemap-footer">
  <template v-if="view">
    <span class="ui-footbar-stat">{{ tr("合计") }} <strong>{{ formatBytes(view.root.bytes) }}</strong></span>
    <span class="ui-footbar-stat"><strong>{{ view.root.files.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
    <span class="ui-footbar-stat"><strong>{{ view.root.dirs.toLocaleString() }}</strong> {{ tr("个目录") }}</span>
    <span v-if="includeDeleted" class="ui-badge sm danger" :title="tr('合计包含已删除的文件')">{{ tr("含已删除") }}</span>
    <span
      v-if="view.truncated"
      class="ui-badge sm warn"
      :title="tr('卷快照达到条目上限而截断，合计可能偏小')"
    ><AlertTriangle :size="11" />{{ tr("索引截断") }}</span>
  </template>
  <span class="ui-footbar-spacer"></span>
  <span v-if="view" class="ui-footbar-stat" :title="tr('大小为文件逻辑大小，与列表一致；稀疏与压缩文件的实际占用可能更小')">{{ tr("逻辑大小 · 卷快照 {0}", { 0: snapshotTime }) }}</span>
  <span
    v-if="snapshotStatus"
    class="ui-badge sm"
    :class="snapshotTone"
    :title="snapshotStatus.state === 'building' ? tr('卷快照构建完成后，树图自动显示新的索引') : snapshotStatus.state === 'stale' ? tr('源卷已变化，快照可能与当前镜像不一致，需重建') : tr('树图由卷快照的索引汇总')"
  >
    <Loader2 v-if="snapshotStatus.state === 'building'" :size="11" class="spin" />
    <Camera v-else :size="11" />
    <span>{{ snapshotStatus.label }}</span>
  </span>
</footer>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { AlertTriangle, Camera, Loader2 } from 'lucide-vue-next';
import type { FsSizeTreeView } from '../../../lib/ipc/fs/sizeTree';
import { formatTimestamp } from '../../../utils/time';
import { formatBytes } from '../model/model';

const props = defineProps<{
  view: FsSizeTreeView | null;
  includeDeleted: boolean;
  snapshotStatus: { state: 'building' | 'ready' | 'stale'; label: string } | null;
}>();

const snapshotTime = computed(() => formatTimestamp(props.view?.builtAt));
const snapshotTone = computed(() => ({ building: 'info', ready: 'success', stale: 'warn' })[props.snapshotStatus?.state ?? 'ready']);
</script>

<style scoped>
.treemap-footer {
  padding-right: 12px;
}
</style>
