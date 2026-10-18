
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, watch } from 'vue';
import { save } from '@tauri-apps/plugin-dialog';
import { timelineExport, type TimelineQuery } from '../../../lib/ipc/timeline';
import { notifyError, notifyExportSuccess } from '../../../composables/useToast';
export function useTimelineExport(options: { imageId: () => string; buildQuery: () => TimelineQuery; total: () => number }) {
  const exporting = ref(false);
  let generation = 0; let disposed = false;
  async function exportRows() {
    if (disposed || exporting.value || options.total() === 0) return;
    const imageId = options.imageId(); const query = options.buildQuery(); const captured = ++generation;
    exporting.value = true;
    try {
      const destination = await save({ defaultPath: tr("时间线-{0}.csv", { 0: imageId }), filters: [{ name: 'CSV', extensions: ['csv'] }] });
      if (!destination || disposed || captured !== generation) return;
      const result = await timelineExport({ imageId, query, destination });
      if (!disposed) notifyExportSuccess(result.output_path, tr("已导出 {0} 条记录", { 0: result.record_count.toLocaleString() }));
    } catch (error) { if (!disposed) notifyError(tr("导出时间线失败"), String(error)); }
    finally { if (!disposed && captured === generation) exporting.value = false; }
  }
  watch(options.imageId, () => { generation++; exporting.value = false; }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; generation++; });
  return { exporting, exportRows };
}
