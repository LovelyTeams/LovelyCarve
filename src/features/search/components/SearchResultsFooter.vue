<template>
            <footer class="ui-footbar result-footer">
              <span class="result-summary footer-task" :title="`${taskFullTitle(selectedTask, tr)} · ${formatTaskTime(selectedTask)} · ${selectedTask.codepages.join(' / ')}`">
                <span
                  v-for="pattern in selectedPatterns.visible"
                  :key="`${pattern.regex ? 'r' : 'k'}:${pattern.text}`"
                  class="pattern-chip mono"
                  :class="{ regex: pattern.regex }"
                >{{ pattern.regex ? `/${pattern.text}/` : pattern.text }}</span>
                <span v-if="selectedPatterns.hidden > 0" class="pattern-more">+{{ selectedPatterns.hidden }}</span>
              </span>
              <span class="ui-badge sm status-badge" :class="searchStatusTone">{{ statusLabel(selectedTask.status, tr) }}</span>
              <span class="ui-footbar-stat">{{ tr("扫描") }} <strong>{{ scannedLabel }}</strong> {{ tr("个对象") }}</span>
              <span class="ui-footbar-stat">{{ tr("search.foundIn") }} <strong :key="matchedFileCountLabel" class="ui-bump">{{ matchedFileCountLabel }}</strong> {{ tr("个文件 ·") }} <strong :key="totalMatchesLabel" class="ui-bump">{{ totalMatchesLabel }}</strong> {{ tr("处") }}</span>
              <span v-if="selectedTask.finished_ms" class="ui-footbar-stat footer-elapsed">{{ tr("耗时") }} <strong>{{ formatElapsedLabel(selectedTask) }}</strong></span>
              <span class="ui-footbar-spacer"></span>
              <span v-if="loadingHits" class="footer-loading">
                <Loader2 :size="11" class="spin" /> {{ tr("加载中…") }}
              </span>
              <span class="footer-total mono">
                {{ hitsTotal.toLocaleString() }} {{ tr("条结果") }}
              </span>
            </footer>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Loader2 } from 'lucide-vue-next';
import type { GlobalSearchHistoryEntry } from '../../../lib/ipc/search';
import { taskFullTitle, formatTaskTime, formatElapsedLabel, statusLabel } from '../model/searchTaskView';
defineProps<{
  selectedTask: GlobalSearchHistoryEntry; selectedPatterns: { visible: { text: string; regex: boolean }[]; hidden: number };
  searchStatusTone: string; scannedLabel: string; matchedFileCountLabel: string; totalMatchesLabel: string;
  loadingHits: boolean; hitsTotal: number;
}>();
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
