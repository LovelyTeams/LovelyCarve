
import { tr } from '../../../composables/useLocale';
import { computed, onMounted, onScopeDispose, ref, watch } from 'vue';
import { carveScanList, carveScanRemove, carveScanStatus, carveScanPause, carveScanResume, carveStart, onCarveScanProgress, type CarveStartOptions, type CarveScanSnapshot } from '../../../lib/ipc/carve';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { confirm } from '../../../composables/useConfirm';
import { notifyError, notifyInfo } from '../../../composables/useToast';
import { isFinished } from '../model/carveView';
interface CarveHistoryOptions {
  imageId: () => string; recovering: () => boolean; onSelected: () => Promise<void>;
  onReset: () => void; onStarted: () => void; onMoreResults: () => void;
}
export function useCarveHistory(options: CarveHistoryOptions) {
  let disposed = false; let generation = 0; let historyRequest = 0; let unlisten: (() => void) | null = null;
  const historyVisible = ref(false);
  const scans = ref<CarveScanSnapshot[]>([]);
  const loadingHistory = ref(false);
  const historyError = ref('');
  const deletingScanId = ref<string | null>(null);
  const scan = ref<CarveScanSnapshot | null>(null);
  const starting = ref(false);
  function updateSnapshot(snapshot: CarveScanSnapshot) {
    if (disposed || snapshot.image_id !== options.imageId()) return;
    const index = scans.value.findIndex(task => task.scan_id === snapshot.scan_id);
    if (index >= 0) scans.value[index] = snapshot;
    else {
      scans.value.unshift(snapshot);
      scans.value.sort((left, right) => right.created_unix_ms - left.created_unix_ms);
      historyVisible.value = true;
    }
    if (scan.value?.scan_id === snapshot.scan_id) scan.value = snapshot;
  }
  async function selectScan(task: CarveScanSnapshot) {
    if (disposed || options.recovering() || scan.value?.scan_id === task.scan_id) return;
    scan.value = task;
    await options.onSelected();
  }
  async function refreshHistory(imageId: string, preferredId?: string) {
    if (disposed || imageId !== options.imageId()) return;
    const request = ++historyRequest;
    loadingHistory.value = true;
    historyError.value = '';
    try {
      const entries = await carveScanList({ imageId });
      if (disposed || request !== historyRequest || imageId !== options.imageId()) return;
      if (entries.some(entry => !scans.value.some(task => task.scan_id === entry.scan_id))) historyVisible.value = true;
      scans.value = entries;
      loadingHistory.value = false;
      const selected = entries.find(task => task.scan_id === (preferredId ?? scan.value?.scan_id)) ?? entries[0];
      if (selected) {
        if (scan.value?.scan_id === selected.scan_id) scan.value = selected;
        else await selectScan(selected);
      } else {
        options.onReset();
        scan.value = null;
      }
    } catch (error) {
      if (!disposed && request === historyRequest) historyError.value = String(error);
    } finally {
      if (!disposed && request === historyRequest) loadingHistory.value = false;
    }
  }
  async function deleteScan(task: CarveScanSnapshot) {
    if (!isFinished(task) || deletingScanId.value || options.recovering()) return;
    const imageId = options.imageId();
    const captured = generation;
    if (!await confirm({ title: tr("删除扫描记录"), message: tr("删除本次扫描及内存中的结果，已恢复到磁盘的文件不受影响。"), confirmText: tr("删除"), danger: true })) return;
    if (disposed || captured !== generation || options.recovering()) return;
    deletingScanId.value = task.scan_id;
    try {
      await carveScanRemove({ imageId, scanId: task.scan_id });
      if (!disposed && captured === generation) await refreshHistory(imageId);
    } catch (error) {
      if (!disposed && captured === generation) notifyError(tr("删除扫描记录失败"), String(error));
    } finally {
      if (!disposed && captured === generation) deletingScanId.value = null;
    }
  }

  const isActive = computed(() => ['queued', 'running', 'paused'].includes(scan.value?.status ?? ''));
  const hasActiveScan = computed(() => scans.value.some(task => !isFinished(task)));
  /** The running phase reads directory records; its total is known only while it reads boot sectors. */
  const filesystemPhase = computed(() => scan.value?.phase === 'filesystem' && !!scan.value.filesystem);
  const progressPercent = computed(() => {
    const current = scan.value;
    if (!current) return 0;
    if (filesystemPhase.value) {
      const { done, total } = current.filesystem!;
      return total ? Math.min(100, Math.round((done / total) * 1000) / 10) : 0;
    }
    if (current.bytes_total <= 0) return 0;
    return Math.min(100, Math.round((current.bytes_scanned / current.bytes_total) * 1000) / 10);
  });
  const progressHeadline = computed(() => {
    const current = scan.value;
    if (!current) return '';
    if (current.status === 'queued') return tr("等待扫描");
    if (filesystemPhase.value && (current.status === 'running' || current.status === 'paused')) {
      const prefix = current.status === 'paused' ? tr("扫描已暂停") : tr("正在扫描文件系统");
      return current.filesystem!.total
        ? `${prefix} · ${tr("引导扇区 {0}%", { 0: progressPercent.value.toFixed(1) })}`
        : `${prefix} · ${tr("已检查 {0} 条记录", { 0: current.filesystem!.done.toLocaleString() })}`;
    }
    if (current.status === 'running') return tr("签名雕刻中 · {0}%", { 0: progressPercent.value.toFixed(1) });
    if (current.status === 'paused') return tr("扫描已暂停 · {0}%", { 0: progressPercent.value.toFixed(1) });
    if (current.status === 'completed') return current.truncated ? tr("扫描已达到结果上限") : tr("扫描完成");
    if (current.status === 'cancelled') return tr("扫描已取消，已发现结果仍可恢复");
    return tr("扫描失败");
  });
  const scanStatusTone = computed(() => ({
    queued: 'info', running: 'info', paused: 'warn', completed: 'success', cancelled: '', failed: 'danger',
  }[scan.value?.status ?? 'completed'] ?? ''));

  async function startScan(configured: CarveStartOptions) {
    if (disposed || starting.value || hasActiveScan.value || options.recovering()) return;
    starting.value = true;
    const imageId = options.imageId();
    const captured = generation;
    try {
      const result = await carveStart({
        imageId,
        options: configured,
      });
      if (disposed || captured !== generation) return;
      const snapshot = await carveScanStatus({ scanId: result.scan_id });
      if (disposed || captured !== generation) return;
      updateSnapshot(snapshot);
      await selectScan(snapshot);
      if (disposed || captured !== generation) return;
      await refreshHistory(imageId, result.scan_id);
      if (!disposed && captured === generation) options.onStarted();
    } catch (error) {
      if (!disposed && captured === generation) notifyError(tr("无法开始恢复扫描"), String(error));
    } finally {
      if (!disposed && captured === generation) starting.value = false;
    }
  }

  async function cancelScan() {
    const taskId = scan.value?.task_id;
    if (!taskId || disposed) return;
    try {
      await taskCancel({ taskId });
      notifyInfo(tr("正在取消扫描"), tr("已提交取消请求，当前读取块完成后停止。"));
    } catch (error) {
      notifyError(tr("取消扫描失败"), String(error));
    }
  }

  async function pauseScan() {
    const scanId = scan.value?.scan_id;
    if (!scanId || disposed) return;
    const captured = generation;
    try {
      const snapshot = await carveScanPause({ scanId });
      if (!disposed && captured === generation) updateSnapshot(snapshot);
    } catch (error) {
      if (!disposed && captured === generation) notifyError(tr("暂停扫描失败"), String(error));
    }
  }

  async function resumeScan() {
    const scanId = scan.value?.scan_id;
    if (!scanId || disposed) return;
    const captured = generation;
    try {
      const snapshot = await carveScanResume({ scanId });
      if (!disposed && captured === generation) updateSnapshot(snapshot);
    } catch (error) {
      if (!disposed && captured === generation) notifyError(tr("继续扫描失败"), String(error));
    }
  }


  watch(options.imageId, imageId => {
    generation++; historyRequest++; scan.value = null; scans.value = []; historyVisible.value = false;
    starting.value = false; deletingScanId.value = null; loadingHistory.value = false;
    historyError.value = '';
    options.onReset(); void refreshHistory(imageId);
  }, { flush: 'sync' });
  onMounted(() => {
    void onCarveScanProgress(event => {
      if (disposed || event.payload.image_id !== options.imageId()) return;
      updateSnapshot(event.payload);
      if (!scan.value) void selectScan(event.payload);
      else if (scan.value.scan_id === event.payload.scan_id) options.onMoreResults();
    }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; })
      .catch(error => { if (!disposed) notifyError(tr("订阅扫描进度失败"), String(error)); });
  });
  onScopeDispose(() => { disposed = true; generation++; historyRequest++; unlisten?.(); unlisten = null; });
  return {
    historyVisible, scans, loadingHistory, historyError, deletingScanId, scan, starting, isActive, hasActiveScan, filesystemPhase,
    progressPercent, progressHeadline, scanStatusTone, refreshHistory, selectScan, deleteScan, startScan, cancelScan, pauseScan, resumeScan
  };
}
