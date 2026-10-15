<template>
<aside class="task-pane">
      <header class="task-pane-header">
        <strong class="task-pane-title">{{ tr("搜索记录") }}</strong>
        <span class="history-count mono">{{ tasks.length }}</span>
        <button class="ui-icon-btn sm" :aria-label="tr('收起搜索记录')" :title="tr('收起搜索记录')" @click="emit('close')"><X :size="14" /></button>
      </header>
      <div class="task-scroll">
        <div v-if="loadingTasks" class="task-state">
          <Loader2 :size="15" class="spin" />
          <span>{{ tr("正在读取任务…") }}</span>
        </div>
        <div v-else-if="tasks.length === 0 && !historyError" class="task-state empty">
          <SearchX :size="18" />
          <span>{{ tr("还没有搜索任务") }}</span>
        </div>
        <ul v-else class="task-list">
          <li
            v-for="task in tasks"
            :key="task.search_id"
            :class="{ active: selectedSearchId === task.search_id }"
          >
            <button class="task-open" type="button" @click="emit('select', task)">
              <span class="task-status" :class="`status-${task.status}`">
                <Loader2 v-if="task.status === 'running'" :size="13" class="spin" />
                <CheckCircle2 v-else-if="task.status === 'success'" :size="13" />
                <CircleSlash v-else-if="task.status === 'cancelled'" :size="13" />
                <CircleAlert v-else :size="13" />
              </span>
              <span class="task-title" :title="taskFullTitle(task)">
                {{ taskDisplayTitle(task.keywords, task.regexes, tr) }}
              </span>
              <span class="task-count mono">
                {{ (runningInfoOf(task)?.hits ?? task.total_matches).toLocaleString() }}
              </span>
              <span class="task-meta">
                {{ formatTaskTime(task) }} · {{ statusLabel(task.status, tr) }}
                <template v-if="task.truncated"> {{ tr("· 不完整") }}</template>
              </span>
              <span
                v-if="task.status === 'running'"
                class="task-progress"
                :class="{ indeterminate: progressPercentOf(task) === null }"
              >
                <span
                  :style="progressPercentOf(task) === null ? undefined : { width: `${progressPercentOf(task)}%` }"
                ></span>
              </span>
            </button>
            <button
              class="task-delete"
              type="button"
              :title="tr('删除整次搜索（含命中）')"
              :aria-label="tr('删除搜索 {0}', { 0: taskDisplayTitle(task.keywords, task.regexes, tr) })"
              :disabled="deletingTaskId !== null"
              @click.stop="emit('delete', task)"
            >
              <Loader2 v-if="deletingTaskId === task.search_id" :size="12" class="spin" />
              <Trash2 v-else :size="12" />
            </button>
          </li>
        </ul>
      </div>
    </aside>

</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Loader2, X, SearchX, CheckCircle2, CircleSlash, CircleAlert, Trash2 } from 'lucide-vue-next';
import type { GlobalSearchHistoryEntry, GlobalSearchProgress } from '../../../lib/ipc/search';
import { taskDisplayTitle } from '../model/searchResults';
import { taskFullTitle, formatTaskTime, statusLabel, searchProgressPercent } from '../model/searchTaskView';
const props = defineProps<{
  tasks: GlobalSearchHistoryEntry[]; loadingTasks: boolean; historyError: string; selectedSearchId: string | null;
  deletingTaskId: string | null; progress: ReadonlyMap<string, GlobalSearchProgress>;
}>();
const emit = defineEmits<{
  close: []; select: [task: GlobalSearchHistoryEntry]; delete: [task: GlobalSearchHistoryEntry];
}>();
const runningInfoOf = (task: GlobalSearchHistoryEntry) => props.progress.get(task.search_id) ?? null;
const progressPercentOf = (task: GlobalSearchHistoryEntry) => task.status === 'running' ? searchProgressPercent(runningInfoOf(task)) : null;
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
