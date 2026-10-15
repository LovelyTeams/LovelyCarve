
import { tr } from '../../../composables/useLocale';
import { onScopeDispose, ref, shallowRef } from 'vue';
import { timelineCount, type TimelineBucket, type TimelineQuery } from '../../../lib/ipc/timeline';
import { notifyError } from '../../../composables/useToast';
import { bucketRange } from '../model/timelineView';
export function useTimelineAggregation(options: { imageId: () => string; buildQuery: () => TimelineQuery; heatmapYear: () => number; tzHours: number }) {
  const BUCKET_CAP = 5000;
  let disposed = false;
  const heatmapBuckets = shallowRef<TimelineBucket[]>([]);
  const loadingHeatmap = ref(false);
  const heatmapError = ref('');
  let heatmapGeneration = 0;
  const buckets = shallowRef<TimelineBucket[]>([]);
  const bucketsTruncated = ref(false);
  const loadingBuckets = ref(false);
  let bucketsGeneration = 0;
  const bucketsError = ref('');

  async function loadBuckets() {
    const generation = ++bucketsGeneration;
    const query = options.buildQuery();
    loadingBuckets.value = true;
    bucketsError.value = '';
    try {
      const data = await timelineCount({ imageId: options.imageId(), query, limit: BUCKET_CAP });
      if (disposed || generation !== bucketsGeneration) return;
      buckets.value = data;
      bucketsTruncated.value = data.length >= BUCKET_CAP;
    } catch (error) {
      if (disposed || generation !== bucketsGeneration) return;
      bucketsError.value = tr("趋势加载失败，请重试");
      notifyError(tr("时间线聚合失败"), String(error));
      buckets.value = [];
    } finally {
      if (!disposed && generation === bucketsGeneration) loadingBuckets.value = false;
    }
  }

  // 年度热力图独立于日内钻取，始终展示当前非时间筛选下的全年分布。
  async function loadHeatmap() {
    const generation = ++heatmapGeneration;
    loadingHeatmap.value = true;
    heatmapError.value = '';
    try {
      const range = bucketRange(String(options.heatmapYear()), 'year', options.tzHours);
      const data = await timelineCount({ imageId: options.imageId(), query: { ...options.buildQuery(), ...range, time_unit: 'day' }, limit: 366 });
      if (!disposed && generation === heatmapGeneration) heatmapBuckets.value = data;
    } catch (error) {
      if (disposed || generation !== heatmapGeneration) return;
      heatmapBuckets.value = [];
      heatmapError.value = tr("热力图加载失败，请重试");
      notifyError(tr("读取活动热力图失败"), String(error));
    } finally {
      if (!disposed && generation === heatmapGeneration) loadingHeatmap.value = false;
    }
  }


  function reset() {
    bucketsGeneration++; heatmapGeneration++;
    buckets.value = []; heatmapBuckets.value = []; bucketsTruncated.value = false;
    loadingBuckets.value = false; loadingHeatmap.value = false; bucketsError.value = ''; heatmapError.value = '';
  }
  onScopeDispose(() => { disposed = true; reset(); });
  return { buckets, bucketsTruncated, loadingBuckets, bucketsError, heatmapBuckets, loadingHeatmap, heatmapError, loadBuckets, loadHeatmap, reset };
}
