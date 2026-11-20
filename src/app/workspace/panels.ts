import { defineAsyncComponent, type AsyncComponentLoader } from 'vue';
import AsyncPanelLoading from '../../components/feedback/AsyncPanelLoading.vue';

function createAsyncPanel(loader: AsyncComponentLoader) {
  return defineAsyncComponent({
    loader,
    delay: 120,
    loadingComponent: AsyncPanelLoading,
  });
}

export const DiskFileList = createAsyncPanel(() => import('../../features/files/components/DiskFileList.vue'));
export const DiskImageInfo = createAsyncPanel(() => import('../../features/evidence/components/DiskImageInfo.vue'));
export const AnalysisResultsPanel = createAsyncPanel(() => import('../../features/analysis/components/AnalysisResultsPanel.vue'));
export const SearchResultsPanel = createAsyncPanel(() => import('../../features/search/components/SearchResultsPanel.vue'));
export const TimelinePanel = createAsyncPanel(() => import('../../features/timeline/components/TimelinePanel.vue'));
export const CarvePanel = createAsyncPanel(() => import('../../features/carving/components/CarvePanel.vue'));

export const KEPT_ALIVE_PANELS = new Set(['files', 'search-results', 'carve', 'timeline', 'analysis']);
