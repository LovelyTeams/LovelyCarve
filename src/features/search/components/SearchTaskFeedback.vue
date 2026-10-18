<template>
            <div v-if="selectedRunning" class="status-banner running" role="status">
              <Loader2 :size="13" class="spin" />
              <span>{{ tr("正在扫描 {0} / {1} 个对象，已命中 {2} 处", { 0: filesDoneLabel, 1: filesTotalLabel, 2: totalMatchesLabel }) }}</span>
              <div class="banner-progress" :class="{ indeterminate: progressPercent === null }" role="progressbar" :aria-label="tr('搜索进度')" :aria-valuenow="progressPercent ?? undefined" aria-valuemin="0" aria-valuemax="100">
                <span :style="progressPercent === null ? undefined : { width: `${progressPercent}%` }"></span>
              </div>
              <span v-if="progressPercent !== null" class="banner-percent mono">{{ progressPercent }}%</span>
            </div>
            <div
              v-if="selectedTask.truncated || selectedTask.read_errors > 0"
              class="status-banner warn"
              role="alert"
            >
              <TriangleAlert :size="13" />
              <span v-if="selectedTask.truncated">{{ tr("总命中已达上限，结果不完整") }}</span>
              <span v-if="selectedTask.read_errors > 0">
                {{ selectedTask.read_errors.toLocaleString() }} {{ tr("个对象读取失败已跳过") }}
              </span>
            </div>
            <div v-if="selectedTask.status === 'failed'" class="status-banner error" role="alert">
              <CircleAlert :size="13" />
              <span>{{ tr("搜索失败：{0}", { 0: selectedTask.error ?? tr("未知错误") }) }}</span>
            </div>
            <div v-if="selectedTask.status === 'cancelled'" class="status-banner warn" role="status">
              <CircleSlash :size="13" />
              <span>{{ tr("搜索已取消，以下为部分结果") }}</span>
            </div>

</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Loader2, TriangleAlert, CircleAlert, CircleSlash } from 'lucide-vue-next';
import type { GlobalSearchHistoryEntry } from '../../../lib/ipc/search';
defineProps<{
  selectedTask: GlobalSearchHistoryEntry; selectedRunning: boolean; progressPercent: number | null;
  filesDoneLabel: string; filesTotalLabel: string; totalMatchesLabel: string;
}>();
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
