<template>
    <aside class="task-pane">
      <header class="task-pane-header">
        <strong class="task-pane-title">{{ tr("雕刻记录") }}</strong><span class="history-count mono">{{ scans.length }}</span>
        <button class="ui-icon-btn sm" :aria-label="tr('收起雕刻记录')" :title="tr('收起雕刻记录')" @click="emit('close')"><X :size="14" /></button>
      </header>
      <div class="task-scroll">
        <div v-if="loadingHistory" class="task-state"><Loader2 :size="15" class="spin" /><span>{{ tr("正在读取任务…") }}</span></div>
        <div v-else-if="!scans.length && !historyError" class="task-state empty"><SearchX :size="18" /><span>{{ tr("还没有雕刻任务") }}</span></div>
        <ul v-else class="task-list">
          <li v-for="task in scans" :key="task.scan_id" :class="{ active: scan?.scan_id === task.scan_id }">
            <button class="task-open" :disabled="recovering" @click="emit('select', task)">
              <span class="task-status" :class="`status-${task.status === 'completed' ? 'success' : task.status}`">
                <Loader2 v-if="task.status === 'running' || task.status === 'queued'" :size="13" class="spin" />
                <CheckCircle2 v-else-if="task.status === 'completed'" :size="13" />
                <Pause v-else-if="task.status === 'paused'" :size="13" />
                <CircleSlash v-else-if="task.status === 'cancelled'" :size="13" /><CircleAlert v-else :size="13" />
              </span>
              <span class="task-title" :title="task.scope_label">{{ task.scope_label }}</span>
              <span class="task-count mono">{{ task.artifacts_found.toLocaleString() }}</span>
              <span class="task-meta">{{ formatScanTime(task, locale) }} · {{ statusLabel(task.status, tr) }}<template v-if="task.truncated"> {{ tr("· 不完整") }}</template></span>
              <span v-if="!isFinished(task)" class="task-progress"><span :style="{ width: `${scanPercent(task)}%` }"></span></span>
            </button>
            <button class="task-delete" :aria-label="tr('删除雕刻 {0}', { 0: task.scope_label })" :title="isFinished(task) ? tr('删除雕刻记录（不删除已恢复文件）') : tr('请先停止扫描')" :disabled="!isFinished(task) || recovering || deletingScanId !== null" @click.stop="emit('delete', task)"><Loader2 v-if="deletingScanId === task.scan_id" :size="12" class="spin" /><Trash2 v-else :size="12" /></button>
          </li>
        </ul>
      </div>
    </aside>
</template>
<script setup lang="ts">
const { locale } = useLocale();
import { tr, useLocale } from '../../../composables/useLocale';

import { X, Loader2, SearchX, CheckCircle2, Pause, CircleSlash, CircleAlert, Trash2 } from 'lucide-vue-next';
import type { CarveScanSnapshot } from '../../../lib/ipc/carve';
import { isFinished, scanPercent, statusLabel, formatScanTime } from '../model/carveView';
defineProps<{ scans: CarveScanSnapshot[]; loadingHistory: boolean; historyError: string; scan: CarveScanSnapshot | null; recovering: boolean; deletingScanId: string | null }>();
const emit = defineEmits<{ close: []; select: [scan: CarveScanSnapshot]; delete: [scan: CarveScanSnapshot] }>();
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
<style scoped src="../styles/carve-workspace.css"></style>
