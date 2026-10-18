
import { tr } from '../../../composables/useLocale';
import { computed, onActivated, onMounted, onScopeDispose, ref, shallowRef, watch } from 'vue';
import { trackedTimelineStart as timelineStart } from '../../tasks/api';
import { timelineStatus, onTimelineProgress, type TimelineStatusInfo } from '../../../lib/ipc/timeline';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { notifyError, notifyInfo } from '../../../composables/useToast';
export function useTimelineBuild(options: { imageId: () => string; onBuilt: () => void }) {
  let generation = 0;
  let statusGeneration = 0;
  let disposed = false;
  let unlisten: (() => void) | null = null;
  const statusInfo = shallowRef<TimelineStatusInfo | null>(null);
  const statusError = ref('');
  const building = ref(false);
  const progress = ref<{ done: number; total: number; rows: number } | null>(null);

  const statusChipClass = computed(() => {
    switch (statusInfo.value?.status) {
      case 'ready':
        return 'success';
      case 'running':
        return 'info';
      case 'stale':
        return 'warn';
      default:
        return '';
    }
  });

  const buildButtonLabel = computed(() => {
    if (building.value || statusInfo.value?.status === 'running') return tr("取消构建");
    switch (statusInfo.value?.status) {
      case 'buildRequired':
        return tr("创建时间线");
      case 'stale':
        return tr("更新时间线");
      default:
        return tr("重建时间线");
    }
  });

  async function refreshStatus(): Promise<TimelineStatusInfo | null> {
    if (disposed) return null;
    const imageId = options.imageId();
    const captured = generation;
    const request = ++statusGeneration;
    try {
      const info = await timelineStatus({ imageId });
      if (captured !== generation || request !== statusGeneration || disposed) return null;
      statusInfo.value = info;
      statusError.value = '';
      return info;
    } catch (error) {
      if (captured === generation && request === statusGeneration && !disposed) {
        notifyError(tr("读取时间线状态失败"), String(error));
        statusInfo.value = null;
        statusError.value = String(error);
      }
      return null;
    }
  }

  async function startBuild() {
    if (disposed || building.value) return;
    const imageId = options.imageId();
    const captured = generation;
    building.value = true;
    progress.value = { done: 0, total: 0, rows: 0 };
    try {
      const summary = await timelineStart({ imageId });
      if (captured !== generation || disposed) return;
      if (summary.status === 'cancelled') {
        notifyInfo(tr("时间线构建已取消"), tr("已写入的部分时间线数据会保留，可随时重建。"));
      } else {
        notifyInfo(
          tr("时间线构建完成"),
          tr("文件 {0} 条、插件事件 {1} 条", { 0: summary.file_rows.toLocaleString(), 1: summary.event_rows.toLocaleString() }) +
          (summary.skipped_invalid_time > 0
            ? tr("（跳过 {0} 条无有效时间戳的记录）", { 0: summary.skipped_invalid_time.toLocaleString() })
            : ''),
        );
      }
    } catch (error) {
      if (captured === generation && !disposed) notifyError(tr("时间线构建失败"), String(error));
    } finally {
      if (captured === generation && !disposed) {
        building.value = false;
        progress.value = null;
        await refreshStatus();
        if (captured === generation && !disposed) options.onBuilt();
      }
    }
  }

  async function buildButtonClicked() {
    if (disposed) return;
    if (building.value || statusInfo.value?.status === 'running') {
      const captured = generation;
      try {
        await taskCancel({ taskId: `timeline-build-${options.imageId()}` });
      } catch (error) {
        if (captured === generation && !disposed) notifyError(tr("取消时间线构建失败"), String(error));
      }
      return;
    }
    await startBuild();
  }


  function reset() {
    generation++; statusGeneration++; statusInfo.value = null; statusError.value = ''; building.value = false; progress.value = null;
  }
  watch(options.imageId, reset, { flush: 'sync' });
  onMounted(() => {
    void onTimelineProgress(event => {
      if (disposed || event.payload.image_id !== options.imageId()) return;
      progress.value = { done: event.payload.done, total: event.payload.total, rows: event.payload.rows };
    }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; })
      .catch(error => { if (!disposed) notifyError(tr("订阅时间线进度失败"), String(error)); });
  });
  onActivated(() => { if (statusInfo.value) void refreshStatus(); });
  onScopeDispose(() => { disposed = true; generation++; statusGeneration++; unlisten?.(); unlisten = null; });
  return { statusInfo, statusError, building, progress, statusChipClass, buildButtonLabel, refreshStatus, buildButtonClicked };
}
