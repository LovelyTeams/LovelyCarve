<template>
<footer class="ui-footbar list-footer">
  <template v-if="deepsearch">
    <span class="ui-footbar-stat">{{ matching ? tr("符合条件") : tr("共") }} <strong>{{ flatTotal.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
    <span v-if="loadedCount < flatTotal" class="ui-footbar-stat">{{ tr("已加载") }} <strong>{{ loadedCount.toLocaleString() }}</strong></span>
    <span
      v-if="flatTruncated"
      class="ui-badge sm warn"
      :title="indexOnly ? tr('已有索引覆盖不完整，仅显示已索引文件') : tr('实时遍历达安全上限，结果不完整')"
    ><AlertTriangle :size="11" />{{ tr("结果不完整") }}</span>
  </template>
  <template v-else>
    <span class="ui-footbar-stat">{{ matching ? tr("符合条件") : tr("共") }} <strong>{{ directoryTotal.toLocaleString() }}</strong> {{ tr("项") }}</span>
    <span v-if="directoryCursor" class="ui-footbar-stat">{{ tr("已加载") }} <strong>{{ loadedCount.toLocaleString() }}</strong></span>
    <span v-if="displayedCount !== loadedCount" class="ui-footbar-stat">{{ tr("显示") }} <strong>{{ displayedCount.toLocaleString() }}</strong></span>
  </template>
  <span v-if="checkedStats.count" class="checked-stat">
    <span class="ui-footbar-stat">{{ tr("已勾选") }} <strong :key="checkedStats.count" class="ui-bump">{{ checkedStats.count.toLocaleString() }}</strong> {{ tr("项") }} · {{ formatBytes(checkedStats.bytes) }}</span>
    <button class="ui-icon-btn sm" type="button" :title="tr('取消勾选 (Esc)')" :aria-label="tr('取消勾选')" @click="emit('clearChecked')"><X :size="14" /></button>
  </span>
  <span class="ui-footbar-spacer"></span>
  <span
    v-if="snapshotStatus"
    class="ui-badge sm snapshot-status"
    :class="snapshotTone"
    :title="snapshotStatus.state === 'building'
      ? tr('正在后台构建卷快照，目录加载将从实时遍历切换为快照')
      : snapshotStatus.state === 'stale'
        ? tr('源卷已变化，快照可能与当前镜像不一致，需重建')
        : tr('目录列表由卷快照直接返回，无需重新遍历文件系统')"
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
import { AlertTriangle, Camera, Loader2, X } from 'lucide-vue-next';
import { formatBytes } from '../model/model';

const props = defineProps<{
  indexOnly?: boolean;
  /** The backend applied the conditions, so its total counts matches, not every entry. */
  matching: boolean;
  deepsearch: boolean; flatTotal: number; flatTruncated: boolean;
  directoryTotal: number; directoryCursor: string | null;
  loadedCount: number; displayedCount: number;
  snapshotStatus: { state: 'building' | 'ready' | 'stale'; label: string } | null;
  checkedStats: { count: number; bytes: number };
}>();
const emit = defineEmits<{ clearChecked: [] }>();

const snapshotTone = computed(() => ({ building: 'info', ready: 'success', stale: 'warn' })[props.snapshotStatus?.state ?? 'ready']);
</script>

<style scoped>
/* ── 页脚：左侧条目数与勾选统计，右侧卷快照状态 ── */
.list-footer {
  padding-right: 12px;
}

.checked-stat {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-left: 4px;
  padding-left: 12px;
  border-left: 1px solid var(--border-color);
}

.checked-stat strong {
  color: var(--primary-color);
}

</style>
